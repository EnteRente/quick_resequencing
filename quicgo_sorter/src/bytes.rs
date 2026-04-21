//! Types and traits for working with bytes.
//! With a focus on zero copy, memory pooling, single threading, and cache locality.
//!
//! Inspired by https://crates.io/crates/bytes
//!

use std::{mem, slice};
use std::ops::{Deref, DerefMut, RangeBounds};
use std::ptr::without_provenance;
use std::rc::Rc;
use crate::bytes::ByteData::{Dynamic, Static};

/// A cheaply cloneable and sliceable chunk of contiguous memory.
#[derive(Debug, Eq, PartialEq)]
pub struct Bytes {
    ptr: *const u8,
    len: usize,
    data: ByteData
}

impl Deref for Bytes {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl DerefMut for Bytes {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.as_mut_slice()
    }
}

impl Bytes {

    /// Creates a new empty `Bytes`.
    pub fn empty() -> Self {
        const EMPTY: &[u8] = &[];
        EMPTY.into()
    }

    /// Creates a new `Bytes` with only length, without allocating data.
    /// This is useful for benchmarking where actual payload is not needed.
    /// The buffer maintains all invariants (length calculations) but has no backing data.
    pub fn from_len(len: usize) -> Self {
        const EMPTY: &[u8] = &[];
        Bytes {
            ptr: EMPTY.as_ptr(),
            len,
            data: Static(EMPTY),
        }
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self.ptr, self.len) }
    }

    #[inline]
    fn as_mut_slice(&self) -> &mut [u8] {
        unsafe { slice::from_raw_parts_mut(self.ptr.cast_mut(), self.len) }
    }

    /// Returns true if the `Bytes` has a length of 0.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Returns a slice of self for the provided range.
    ///
    /// This will increment the reference count for the underlying memory and
    /// return a new `Bytes` handle set to the slice.
    ///
    /// This operation is `O(1)`.
    ///
    /// # Panics
    ///
    /// Requires that `begin <= end` and `end <= self.len()`, otherwise slicing
    /// will panic.
    pub fn slice(&self, range: impl RangeBounds<usize>) -> Bytes {
        use core::ops::Bound;

        let len = self.len();

        let begin = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n.checked_add(1).expect("out of range"),
            Bound::Unbounded => 0,
        };

        let end = match range.end_bound() {
            Bound::Included(&n) => n.checked_add(1).expect("out of range"),
            Bound::Excluded(&n) => n,
            Bound::Unbounded => len,
        };

        assert!(
            begin <= end,
            "range start must not be greater than end: {:?} <= {:?}",
            begin,
            end,
        );
        assert!(
            end <= len,
            "range end out of bounds: {:?} <= {:?}",
            end,
            len,
        );

        if end == begin {
            return Bytes::empty();
        }

        let mut ret = self.clone();

        ret.len = end - begin;
        ret.ptr = unsafe { ret.ptr.add(begin) };

        ret
    }

    /// Advance start of buffer by `cnt`.
    /// This reduces the size by `cnt`.
    #[inline]
    pub fn advance(&mut self, cnt: usize) {
        assert!(
            cnt <= self.len(),
            "cannot advance past `remaining`: {:?} <= {:?}",
            cnt,
            self.len(),
        );

        unsafe {
            self.inc_start(cnt);
        }
    }

    #[inline]
    unsafe fn inc_start(&mut self, by: usize) {
        // should already be asserted, but debug assert for tests
        debug_assert!(self.len >= by, "internal: inc_start out of bounds");
        self.len -= by;
        self.ptr = self.ptr.add(by);
    }

    /// Creates a new `Bytes` with length zero and the given pointer as the address.
    fn new_empty_with_ptr(ptr: *const u8) -> Self {
        debug_assert!(!ptr.is_null());

        // Detach this pointer's provenance from whichever allocation it came from, and reattach it
        // to the provenance of the fake ZST [u8;0] at the same address.
        let ptr = without_provenance(ptr as usize);

        Bytes {
            ptr,
            len: 0,
            data: Static(&[]),
        }
    }


    /// Splits the bytes into two at the given index.
    ///
    /// Afterwards `self` contains elements `[at, len)`, and the returned
    /// `Bytes` contains elements `[0, at)`.
    ///
    /// This is an `O(1)` operation that just increases the reference count and
    /// sets a few indices.
    ///
    /// # Panics
    ///
    /// Panics if `at > len`.
    #[must_use = "consider Bytes::advance if you don't need the other half"]
    pub fn split_to(&mut self, at: usize) -> Self {
        if at == self.len() {
            let end_ptr = self.ptr.wrapping_add(at);
            return mem::replace(self, Bytes::new_empty_with_ptr(end_ptr));
        }

        if at == 0 {
            return Bytes::new_empty_with_ptr(self.ptr);
        }

        assert!(
            at <= self.len(),
            "split_to out of bounds: {:?} <= {:?}",
            at,
            self.len(),
        );

        let mut ret = self.clone();

        unsafe { self.inc_start(at) };

        ret.len = at;
        ret
    }

    /// Splits the bytes into two at the given index.
    ///
    /// Afterwards `self` contains elements `[0, at)`, and the returned `Bytes`
    /// contains elements `[at, len)`. It's guaranteed that the memory does not
    /// move, that is, the address of `self` does not change, and the address of
    /// the returned slice is `at` bytes after that.
    ///
    /// This is an `O(1)` operation that just increases the reference count and
    /// sets a few indices.
    ///
    /// # Panics
    ///
    /// Panics if `at > len`.
    #[must_use = "consider Bytes::truncate if you don't need the other half"]
    pub fn split_off(&mut self, at: usize) -> Self {
        if at == self.len() {
            return Bytes::new_empty_with_ptr(self.ptr.wrapping_add(at));
        }

        if at == 0 {
            return mem::replace(self, Bytes::new_empty_with_ptr(self.ptr));
        }

        assert!(
            at <= self.len(),
            "split_off out of bounds: {:?} <= {:?}",
            at,
            self.len(),
        );

        let mut ret = self.clone();

        self.len = at;

        unsafe { ret.inc_start(at) };

        ret
    }
}

impl Default for Bytes {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Eq, PartialEq)]
enum ByteData {
    Static(&'static [u8]),
    Dynamic(Rc<Vec<u8>>),
}

impl ByteData {
    pub fn clone(&self) -> ByteData {
        match self {
            Static(v) => Static(v),
            Dynamic(v) => Dynamic(v.clone()),
        }
    }
}

impl Clone for Bytes {
    fn clone(&self) -> Self {
        Bytes {
            ptr: self.ptr,
            len: self.len,
            data: self.data.clone(),
        }
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(v: Vec<u8>) -> Self {
        Bytes {
            ptr: v.as_ptr(),
            len: v.len(),
            data: Dynamic(Rc::new(v)),
        }
    }
}

impl From<&'static [u8]> for Bytes {
    fn from(v: &'static [u8]) -> Self {
        Bytes {
            ptr: v.as_ptr(),
            len: v.len(),
            data: Static(v),
        }
    }
}

impl<const S: usize> From<&'static [u8; S]> for Bytes {
    fn from(v: &'static [u8; S]) -> Self {
        Bytes {
            ptr: v.as_ptr(),
            len: v.len(),
            data: Static(v),
        }
    }
}



/// Like the `Into` trait but with copying memory
pub trait IntoCopy<T> {
    /// Converts this type into the (usually inferred) input type by copying memory.
    fn copy_into(&self) -> T;
}

impl IntoCopy<Bytes> for &[u8] {
    fn copy_into(&self) -> Bytes {
        self.to_vec().into()
    }
}

impl IntoCopy<Bytes> for &mut [u8] {
    fn copy_into(&self) -> Bytes {
        self.to_vec().into()
    }
}