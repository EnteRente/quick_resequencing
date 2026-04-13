use std::marker::PhantomData;
use std::ptr::NonNull;

use crate::frame::StreamFrame;

// Rust adaptation for lsquic's intrusive TAILQ used by `di_nocopy`.
//
// Upstream reference:
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_nocopy.c
//
// The C code keeps `TAILQ_ENTRY(stream_frame)` links inside the queued frame
// object. This Rust version now does the same logically: the boxed
// `StreamFrame` itself carries intrusive `prev` / `next` links.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) struct FrameHandle(NonNull<StreamFrame>);

#[derive(Debug, Default)]
pub(crate) struct IntrusiveFrameList {
    head: Option<NonNull<StreamFrame>>,
    tail: Option<NonNull<StreamFrame>>,
    len: usize,
    owns_nodes: PhantomData<Box<StreamFrame>>,
}

impl IntrusiveFrameList {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn front(&self) -> Option<FrameHandle> {
        self.head.map(FrameHandle)
    }

    #[inline]
    pub(crate) fn back(&self) -> Option<FrameHandle> {
        self.tail.map(FrameHandle)
    }

    #[inline]
    pub(crate) fn next(&self, handle: FrameHandle) -> Option<FrameHandle> {

        unsafe { handle.0.as_ref().next.map(FrameHandle) }
    }

    #[inline]
    pub(crate) fn prev(&self, handle: FrameHandle) -> Option<FrameHandle> {
        unsafe { handle.0.as_ref().prev.map(FrameHandle) }
    }

    #[inline]
    pub(crate) fn get(&self, handle: FrameHandle) -> &StreamFrame {
        unsafe { handle.0.as_ref() }
    }

    pub(crate) fn push_front(&mut self, frame: StreamFrame) -> FrameHandle {
        let mut boxed = Box::new(frame);
        boxed.prev = None;
        boxed.next = self.head;
        let handle = FrameHandle(NonNull::from(boxed.as_mut()));

        if let Some(mut old_head) = self.head {
            unsafe { old_head.as_mut().prev = Some(handle.0) };
        } else {
            self.tail = Some(handle.0);
        }

        self.head = Some(handle.0);
        self.len += 1;
        let _ = Box::into_raw(boxed);
        handle
    }

    pub(crate) fn insert_after(&mut self, mut after: FrameHandle, frame: StreamFrame) -> FrameHandle {
        let next = unsafe { after.0.as_ref().next };
        let mut boxed = Box::new(frame);
        boxed.prev = Some(after.0);
        boxed.next = next;
        let handle = FrameHandle(NonNull::from(boxed.as_mut()));

        unsafe { after.0.as_mut().next = Some(handle.0) };

        if let Some(mut next) = next {
            unsafe { next.as_mut().prev = Some(handle.0) };
        } else {
            self.tail = Some(handle.0);
        }

        self.len += 1;
        let _ = Box::into_raw(boxed);
        handle
    }

    pub(crate) fn remove(&mut self, handle: FrameHandle) -> Box<StreamFrame> {
        let (prev, next) = unsafe {
            let node = handle.0.as_ref();
            (node.prev, node.next)
        };

        if let Some(mut prev) = prev {
            unsafe { prev.as_mut().next = next };
        } else {
            self.head = next;
        }

        if let Some(mut next) = next {
            unsafe { next.as_mut().prev = prev };
        } else {
            self.tail = prev;
        }

        self.len -= 1;

        let mut boxed = unsafe { Box::from_raw(handle.0.as_ptr()) };
        boxed.prev = None;
        boxed.next = None;
        boxed
    }

    pub(crate) fn pop_front(&mut self) -> Option<Box<StreamFrame>> {
        let head = self.front()?;
        Some(self.remove(head))
    }
}

impl Drop for IntrusiveFrameList {
    fn drop(&mut self) {
        while self.pop_front().is_some() {}
    }
}
