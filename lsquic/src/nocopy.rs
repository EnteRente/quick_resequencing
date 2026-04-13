use std::mem::size_of;

use crate::frame::{DataFrameSnapshot, InsFrame, StreamFrame, StreamId};
use crate::list::{FrameHandle, IntrusiveFrameList};

// Port of lsquic's nocopy receive-side data-in implementation.
//
// Upstream reference:
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_nocopy.c
//
// Closely mirrored C paths:
// - `insert_frame()`
// - `nocopy_di_get_frame()`
// - `nocopy_di_frame_done()`
// - `nocopy_di_readable_bytes()`
// - `check_efficiency()`
//
// The major structural adaptation is replacing the original macro-based TAILQ
// with an explicit Rust intrusive doubly linked list backed by heap nodes.
pub const EFF_CHECK_THRESH_LOW: usize = 10;
pub const EFF_CHECK_THRESH_HIGH: usize = 1000;
pub const EFF_FAR_TRAVERSE_COUNT: usize = 4;
pub const EFF_MAX_HOLES: usize = 5;
pub const EFF_TINY_FRAME_SZ: u64 = 64;

#[derive(Debug)]
pub struct NoCopyDataIn {
    frames_in: IntrusiveFrameList,
    byteage: u64,
    fin_off: u64,
    stream_id: StreamId,
    n_frames: usize,
    n_holes: usize,
    cons_far: usize,
    fin_set: bool,
    fin_reached: bool,
    switch_impl: bool,
}

impl NoCopyDataIn {
    pub fn new(stream_id: StreamId) -> Self {
        Self {
            frames_in: IntrusiveFrameList::new(),
            byteage: 0,
            fin_off: 0,
            stream_id,
            n_frames: 0,
            n_holes: 0,
            cons_far: 0,
            fin_set: false,
            fin_reached: false,
            switch_impl: false,
        }
    }

    #[inline]
    pub fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    #[inline]
    pub fn byteage(&self) -> u64 {
        self.byteage
    }

    #[inline]
    pub fn wants_switch(&self) -> bool {
        self.switch_impl
    }

    #[inline]
    pub fn clear_switch(&mut self) {
        self.switch_impl = false;
    }

    pub fn insert_frame(&mut self, new_frame: StreamFrame, read_offset: u64) -> InsFrame {
        // Mirrors `insert_frame()` plus the post-insert efficiency-triggered
        // `DI_SWITCH_IMPL` request from lsquic_di_nocopy.c.
        let (ins, count) = self.insert_frame_impl(new_frame, read_offset);
        if matches!(ins, InsFrame::Ok) && self.check_efficiency(count) {
            self.switch_impl = true;
        }
        ins
    }

    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool, read_offset: u64) -> InsFrame {
        self.insert_frame(StreamFrame::new_len_only(self.stream_id, offset, fin, len), read_offset)
    }

    pub fn get_frame(&self, read_offset: u64) -> Option<DataFrameSnapshot> {
        // Mirrors `nocopy_di_get_frame()`: only the head frame is readable and
        // only when its current read offset matches the stream read offset.
        let frame = self.frames_in.front().map(|head| self.frames_in.get(head))?;
        if frame.data_frame.read_offset() == read_offset {
            Some(frame.data_frame)
        } else {
            None
        }
    }

    pub fn copy_current_data(&self, read_offset: u64, out: &mut [u8]) -> Option<usize> {
        let handle = self.frames_in.front()?;
        let frame = self.frames_in.get(handle);
        if frame.data_frame.read_offset() != read_offset {
            return None;
        }

        let len = frame.data_frame.readable_len().min(out.len() as u64) as usize;
        if frame.has_payload() {
            let start = frame.data_frame.read_off as usize;
            let src = &frame.data()[start..];
            out[..len].copy_from_slice(&src[..len]);
        } else {
            out[..len].fill(0);
        }
        Some(len)
    }

    pub fn frame_done(&mut self, data_frame: &DataFrameSnapshot) {
        // Mirrors `nocopy_di_frame_done()`.
        assert_eq!(data_frame.read_off, data_frame.size);

        let Some(handle) = self.frames_in.front() else {
            return;
        };

        let frame = self.frames_in.remove(handle);
        let first = self.frames_in.front().map(|next| self.frames_in.get(next));

        if let Some(first) = first {
            self.n_holes -=
                usize::from(frame.data_frame.offset + frame.data_frame.size as u64 != first.data_frame.offset);
        }

        self.n_frames -= 1;
        self.byteage -= frame.data_frame.size as u64;

        if frame.data_frame.fin {
            self.fin_reached = true;
        }
    }

    #[inline]
    pub fn empty(&self) -> bool {
        self.frames_in.is_empty()
    }

    pub fn pop_front_frame(&mut self) -> Option<Box<StreamFrame>> {
        let frame = self.frames_in.pop_front()?;
        self.n_frames -= 1;
        self.byteage -= frame.data_frame.size as u64;
        Some(frame)
    }

    pub fn readable_bytes(&self, mut read_offset: u64) -> u64 {
        let starting_offset = read_offset;
        let mut current = self.frames_in.front();

        while let Some(handle) = current {
            let frame = self.frames_in.get(handle);
            if frame.data_frame.read_offset() == read_offset {
                read_offset += frame.data_frame.end_offset() - frame.data_frame.read_offset();
            } else if read_offset > starting_offset {
                break;
            }

            current = self.frames_in.next(handle);
        }

        read_offset - starting_offset
    }

    pub fn mem_used(&self) -> usize {
        size_of::<Self>()
            + self
                .iter_handles()
                .map(|handle| size_of::<StreamFrame>() + self.frames_in.get(handle).packet_len())
                .sum::<usize>()
    }

    fn iter_handles(&self) -> impl Iterator<Item = FrameHandle> + '_ {
        std::iter::successors(self.frames_in.front(), |handle| self.frames_in.next(*handle))
    }

    fn insert_frame_impl(&mut self, new_frame: StreamFrame, read_offset: u64) -> (InsFrame, usize) {
        // Direct control-flow port of lsquic_di_nocopy.c: validate against the
        // current read offset and known FIN, find left/right neighbors, then use
        // the same 2-bit neighbor-selection cases to decide OK/DUP/OVERLAP/ERR.
        let new_off = new_frame.data_frame.offset;
        let new_end = new_frame.data_frame.end_offset();
        let new_size = new_frame.data_frame.size;
        let new_fin = new_frame.data_frame.fin;

        if read_offset > new_end {
            return if new_fin {
                (InsFrame::Err, 0)
            } else {
                (InsFrame::Dup, 0)
            };
        }

        if self.fin_set {
            if new_fin && new_end != self.fin_off {
                return (InsFrame::Err, 0);
            }

            if new_end > self.fin_off {
                return (InsFrame::Err, 0);
            }

            if read_offset == new_end {
                return (InsFrame::Dup, 0);
            }
        } else if read_offset == new_end && !new_fin {
            return (InsFrame::Dup, 0);
        }

        let mut next = self.frames_in.back();
        let mut prev = None;
        let mut count = 0;

        if let Some(mut next_handle) = next {
            if new_off < self.frames_in.get(next_handle).data_frame.offset {
                count = 1;
                prev = self.frames_in.prev(next_handle);

                while let Some(prev_handle) = prev {
                    if new_off >= self.frames_in.get(prev_handle).data_frame.offset {
                        break;
                    }

                    count += 1;
                    next_handle = prev_handle;
                    next = Some(next_handle);
                    prev = self.frames_in.prev(prev_handle);
                }
            }
        }

        if prev.is_none()
            && next.is_some()
            && new_off >= self.frames_in.get(next.expect("checked as some")).data_frame.offset
        {
            let next_handle = next.expect("checked as some");
            prev = Some(next_handle);
            next = self.frames_in.next(next_handle);
        }

        let select = ((prev.is_some() as u8) << 1) | next.is_some() as u8;

        match select {
            0 => {
                if read_offset == new_end {
                    if new_size != 0 {
                        if new_fin && !(self.fin_reached && read_offset == self.fin_off) {
                            return (InsFrame::Overlap, count);
                        }

                        return (InsFrame::Dup, count);
                    } else if !new_fin || (self.fin_reached && read_offset == self.fin_off) {
                        return (InsFrame::Dup, count);
                    }
                } else if read_offset > new_off {
                    return (InsFrame::Overlap, count);
                }

                self.frames_in.push_front(new_frame);
            },

            1 => {
                let next_frame = self.frames_in.get(next.expect("only right neighbor"));
                if new_end > next_frame.data_frame.offset {
                    return (InsFrame::Overlap, count);
                } else if read_offset > new_off {
                    return (InsFrame::Overlap, count);
                }

                self.n_holes += usize::from(new_end != next_frame.data_frame.offset);
                self.frames_in.push_front(new_frame);
            },

            2 | 3 => {
                let prev_handle = prev.expect("left neighbor exists");
                let prev_frame = self.frames_in.get(prev_handle);

                if prev_frame.data_frame.offset == new_off && prev_frame.data_frame.size == new_size {
                    if !prev_frame.data_frame.fin && new_fin {
                        return (InsFrame::Overlap, count);
                    }

                    return (InsFrame::Dup, count);
                }

                if prev_frame.data_frame.end_offset() > new_off {
                    return (InsFrame::Overlap, count);
                }

                if let Some(next_handle) = next {
                    let next_frame = self.frames_in.get(next_handle);
                    if new_end > next_frame.data_frame.offset {
                        return (InsFrame::Overlap, count);
                    } else if read_offset > new_off {
                        return (InsFrame::Overlap, count);
                    }
                }

                self.n_holes += usize::from(prev_frame.data_frame.end_offset() != new_off);

                if let Some(next_handle) = next {
                    let next_frame = self.frames_in.get(next_handle);
                    self.n_holes += usize::from(new_end != next_frame.data_frame.offset);
                    self.n_holes -= 1;
                }

                self.frames_in.insert_after(prev_handle, new_frame);
            },

            _ => unreachable!("select is a 2-bit value"),
        }

        if new_fin {
            self.fin_set = true;
            self.fin_off = new_end;
        }

        self.n_frames += 1;
        self.byteage += new_size as u64;
        (InsFrame::Ok, count)
    }

    fn check_efficiency(&mut self, count: usize) -> bool {
        // Mirrors `check_efficiency()` in lsquic_di_nocopy.c.
        if self.n_frames <= EFF_CHECK_THRESH_LOW {
            self.cons_far = 0;
            return false;
        }

        if self.n_frames > EFF_CHECK_THRESH_HIGH {
            return true;
        }

        if count >= self.n_frames / 2 {
            self.cons_far += 1;
            if self.cons_far > EFF_FAR_TRAVERSE_COUNT {
                return true;
            }
        } else {
            self.cons_far = 0;
        }

        if self.n_holes > EFF_MAX_HOLES {
            return true;
        }

        self.byteage / EFF_TINY_FRAME_SZ < self.n_frames as u64
    }
}
