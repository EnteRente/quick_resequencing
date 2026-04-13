use std::alloc::{self, Layout};
use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{self, NonNull};
use std::slice;

use crate::frame::{DataFrameSnapshot, InsFrame, StreamFrame, StreamId};

// Port of lsquic's hash-based receive-side data-in implementation.
//
// Upstream reference:
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_hash.c
//
// Closely mirrored C paths:
// - `lsquic_data_in_hash_insert_data_frame()`
// - `hash_di_get_frame()`
// - `hash_di_frame_done()`
// - `hash_di_readable_bytes()`
// - `hash_grow()`
// - `hash_insert()`
// - `hash_find()`
// - `hash_remove()`
// - `new_block()`
// - `block_write()`
// - `has_bytes_after()`
// - `n_avail_bytes()`
//
// This version now follows the upstream structure much more closely:
// - `data_block` is an intrusive node with embedded TAILQ links
// - `HashDataIn` owns a flat array of per-bucket queue heads
// - bucket count and bucket selection use direct Rust translations of the
//   original `N_BUCKETS` and `BUCKNO` C macros
// - `db_data` is modeled as raw storage via `MaybeUninit`, which avoids the
//   eager zero-initialization of untouched block bytes
// - bucket chains now follow the original `TAILQ_HEAD` / `TAILQ_ENTRY`
//   pointer shape (`first` + `last` and `next` + `prev_ptr`) instead of a
//   simpler head/tail + prev/next adaptation

macro_rules! N_BUCKETS {
    ($n_bits:expr) => {
        1usize << (($n_bits) as usize)
    };
}

macro_rules! BUCKNO {
    ($n_bits:expr, $off:expr) => {
        (((($off) / DB_DATA_SIZE as u64) as usize) & (N_BUCKETS!($n_bits) - 1))
    };
}

pub const N_DB_SETS: usize = 57;
// 3616 on 64-bit machines
pub const DB_DATA_SIZE: usize =
    0x1000 - 2 * size_of::<usize>() - size_of::<u64>() - N_DB_SETS * size_of::<u64>();

const _: () = assert!(N_DB_SETS * 64 >= DB_DATA_SIZE);
const _: () = assert!(N_DB_SETS * 64 - 64 <= DB_DATA_SIZE);

#[repr(C)]
#[derive(Debug)]
struct DataBlock {
    db_next: Option<NonNull<DataBlock>>,
    db_prev: *mut Option<NonNull<DataBlock>>,
    db_off: u64,
    db_set: [u64; N_DB_SETS],
    db_data: MaybeUninit<[u8; DB_DATA_SIZE]>,
}

#[repr(C)]
#[derive(Debug)]
struct DblockHead {
    first: Option<NonNull<DataBlock>>,
    last: *mut Option<NonNull<DataBlock>>,
}

impl DblockHead {
    #[inline]
    fn first(&self) -> Option<NonNull<DataBlock>> {
        self.first
    }

    unsafe fn insert_tail(&mut self, mut block: NonNull<DataBlock>) {
        unsafe {
            block.as_mut().db_next = None;
            block.as_mut().db_prev = self.last;
            *self.last = Some(block);
            self.last = ptr::addr_of_mut!(block.as_mut().db_next);
        }
    }

    unsafe fn remove(&mut self, mut block: NonNull<DataBlock>) {
        let (prev_ptr, next) = unsafe {
            let block_ref = block.as_ref();
            (block_ref.db_prev, block_ref.db_next)
        };

        if let Some(mut next) = next {
            unsafe { next.as_mut().db_prev = prev_ptr };
        } else {
            self.last = prev_ptr;
        }

        unsafe {
            *prev_ptr = next;
            block.as_mut().db_prev = ptr::null_mut();
            block.as_mut().db_next = None;
        }
    }
}

#[derive(Debug)]
pub struct HashDataIn {
    fin_off: u64,
    buckets: NonNull<DblockHead>,
    last_block: Option<NonNull<DataBlock>>,
    current_frame: DataFrameSnapshot,
    stream_id: StreamId,
    count: usize,
    nbits: u8,
    fin: bool,
    synthetic_data: bool,
    switch_impl: bool,
    owns_memory: PhantomData<(DataBlock, DblockHead)>,
}

impl HashDataIn {
    pub fn new(stream_id: StreamId, byteage: u64) -> Self {
        let nbits = if byteage >= DB_DATA_SIZE as u64 {
            my_log2((byteage / DB_DATA_SIZE as u64) as u32) + 2
        } else {
            3
        };
        let bucket_count = N_BUCKETS!(nbits);
        let buckets =
            alloc_buckets(bucket_count).unwrap_or_else(|| alloc::handle_alloc_error(buckets_layout(bucket_count)));

        Self {
            fin_off: 0,
            buckets,
            last_block: None,
            current_frame: DataFrameSnapshot {
                offset: 0,
                read_off: 0,
                size: 0,
                fin: false,
            },
            stream_id,
            count: 0,
            nbits,
            fin: false,
            synthetic_data: false,
            switch_impl: false,
            owns_memory: PhantomData,
        }
    }

    #[inline]
    pub fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    #[inline]
    pub fn wants_switch(&self) -> bool {
        self.switch_impl
    }

    #[inline]
    pub fn clear_switch(&mut self) {
        self.switch_impl = false;
    }

    pub fn insert_frame(&mut self, frame: StreamFrame, read_offset: u64) -> InsFrame {
        if frame.has_payload() {
            self.insert_data_frame(&frame.data_frame, frame.data(), read_offset)
        } else {
            self.insert_len_data_frame(&frame.data_frame, read_offset)
        }
    }

    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool, read_offset: u64) -> InsFrame {
        assert!(len <= u16::MAX as usize);
        let data_frame = DataFrameSnapshot {
            offset,
            read_off: 0,
            size: len as u16,
            fin,
        };
        self.insert_len_data_frame(&data_frame, read_offset)
    }

    pub fn insert_data_frame(
        &mut self,
        data_frame: &DataFrameSnapshot,
        data: &[u8],
        read_offset: u64,
    ) -> InsFrame {
        assert!(
            !self.synthetic_data || self.count == 0,
            "cannot mix payload-backed and length-only hash inserts"
        );
        if self.count == 0 {
            self.synthetic_data = false;
        }
        self.insert_data_frame_inner(data_frame, Some(data), read_offset)
    }

    pub(crate) fn insert_len_data_frame(
        &mut self,
        data_frame: &DataFrameSnapshot,
        read_offset: u64,
    ) -> InsFrame {
        assert!(
            self.synthetic_data || self.count == 0,
            "cannot mix payload-backed and length-only hash inserts"
        );
        self.synthetic_data = true;
        self.insert_data_frame_inner(data_frame, None, read_offset)
    }

    fn insert_data_frame_inner(
        &mut self,
        data_frame: &DataFrameSnapshot,
        data: Option<&[u8]>,
        read_offset: u64,
    ) -> InsFrame {
        // Mirrors `lsquic_data_in_hash_insert_data_frame()`.
        let frame_end = data_frame.offset + data_frame.size as u64;

        if frame_end < read_offset {
            return if data_frame.fin {
                InsFrame::Err
            } else {
                InsFrame::Dup
            };
        }

        if self.fin && ((data_frame.fin && frame_end != self.fin_off) || frame_end > self.fin_off)
        {
            return InsFrame::Err;
        }

        let (mut size, mut off, mut data_offset) = if data_frame.offset < read_offset {
            let diff = read_offset - data_frame.offset;
            assert!(diff <= data_frame.size as u64);
            (
                data_frame.size as usize - diff as usize,
                data_frame.offset + diff,
                diff as usize,
            )
        } else {
            (data_frame.size as usize, data_frame.offset, 0)
        };

        let mut key = off - (off % DB_DATA_SIZE as u64);
        let mut block = None;
        let mut first = true;

        while first || size > 0 {
            first = false;

            let mut block_ptr = match self.hash_find(key) {
                Some(block) => block,
                None => match self.new_block(key) {
                    Some(block) => block,
                    None => return InsFrame::Err,
                },
            };

            let written = unsafe {
                match data {
                    Some(data) => block_write(
                        block_ptr.as_mut(),
                        (off % DB_DATA_SIZE as u64) as usize,
                        &data[data_offset..],
                        size,
                    ),
                    None => block_mark(
                        block_ptr.as_mut(),
                        (off % DB_DATA_SIZE as u64) as usize,
                        size,
                    ),
                }
            };

            size -= written;
            off += written as u64;
            if data.is_some() {
                data_offset += written;
            }
            key += DB_DATA_SIZE as u64;
            block = Some(block_ptr);
        }

        if data_frame.fin {
            let fin_off = frame_end;
            let block = block.expect("insert loop always runs at least once");
            let has_later_bytes =
                unsafe { has_bytes_after(block.as_ref(), (fin_off - block.as_ref().db_off) as usize) };
            if has_later_bytes || self.hash_find(key).is_some() {
                return InsFrame::Err;
            }

            self.fin = true;
            self.fin_off = fin_off;
        }

        InsFrame::Ok
    }

    pub fn get_frame(&mut self, read_offset: u64) -> Option<DataFrameSnapshot> {
        // Mirrors `hash_di_get_frame()`.
        let key = read_offset - (read_offset % DB_DATA_SIZE as u64);
        let block = self.hash_find(key);

        if let Some(block) = block {
            if self.setup_data_frame(read_offset, block) {
                Some(self.current_frame)
            } else {
                None
            }
        } else if self.fin && read_offset == self.fin_off {
            self.last_block = None;
            self.current_frame = DataFrameSnapshot {
                offset: read_offset - read_offset % DB_DATA_SIZE as u64,
                read_off: 0,
                size: 0,
                fin: true,
            };
            Some(self.current_frame)
        } else {
            None
        }
    }

    pub fn copy_current_data(&mut self, read_offset: u64, out: &mut [u8]) -> Option<usize> {
        let frame = self.get_frame(read_offset)?;
        let len = frame.readable_len() as usize;

        if len == 0 {
            return Some(0);
        }

        if self.synthetic_data {
            let to_copy = len.min(out.len());
            out[..to_copy].fill(0);
            return Some(to_copy);
        }

        let block = self
            .last_block
            .expect("readable frame must come from a data block");
        let block = unsafe { block.as_ref() };
        let start = frame.read_off as usize;
        let end = frame.size as usize;
        let src = unsafe {
            slice::from_raw_parts(block.db_data.as_ptr().cast::<u8>().add(start), end - start)
        };
        let to_copy = src.len().min(out.len());
        out[..to_copy].copy_from_slice(&src[..to_copy]);
        Some(to_copy)
    }

    pub fn frame_done(&mut self, data_frame: &DataFrameSnapshot) {
        // Mirrors `hash_di_frame_done()`.
        if let Some(block) = self.last_block {
            let should_remove = unsafe {
                data_frame.read_off as usize == DB_DATA_SIZE
                    || !has_bytes_after(block.as_ref(), data_frame.read_off as usize)
            };

            if should_remove {
                self.hash_remove(block);
                self.last_block = None;
                unsafe { free_block(block) };
                if self.count == 0 && !self.fin {
                    self.switch_impl = true;
                }
            }
        } else {
            assert!(data_frame.fin && data_frame.size == 0);
        }
    }

    #[inline]
    pub fn empty(&self) -> bool {
        self.count == 0
    }

    #[inline]
    pub fn ready_at(&self, read_offset: u64) -> bool {
        let key = read_offset - (read_offset % DB_DATA_SIZE as u64);

        if let Some(block) = self.hash_find(key) {
            let block = unsafe { block.as_ref() };
            let offset = (read_offset % DB_DATA_SIZE as u64) as usize;
            let set = offset >> 6;
            let bit = offset & 0x3F;

            if (block.db_set[set] & (1u64 << bit)) != 0 {
                return true;
            }
        }

        self.fin && read_offset == self.fin_off
    }

    pub fn readable_bytes(&mut self, mut read_offset: u64) -> u64 {
        let starting_offset = read_offset;

        while let Some(frame) = self.get_frame(read_offset) {
            let readable = frame.readable_len();
            if readable == 0 {
                break;
            }

            read_offset += readable;
        }

        read_offset - starting_offset
    }

    pub fn mem_used(&self) -> usize {
        size_of::<Self>()
            + self.count * size_of::<DataBlock>()
            + self.bucket_count() * size_of::<DblockHead>()
    }

    #[inline]
    pub fn block_count(&self) -> usize {
        self.count
    }

    #[inline]
    pub fn bucket_count(&self) -> usize {
        N_BUCKETS!(self.nbits)
    }

    fn setup_data_frame(&mut self, read_offset: u64, block: NonNull<DataBlock>) -> bool {
        // Mirrors `setup_data_frame()` in lsquic_di_hash.c.
        let block_ref = unsafe { block.as_ref() };
        let offset = (read_offset % DB_DATA_SIZE as u64) as usize;
        let set = offset >> 6;
        let bit = offset & 0x3F;

        if (block_ref.db_set[set] & (1u64 << bit)) != 0 {
            let avail = n_avail_bytes(block_ref, set, bit);
            self.last_block = Some(block);
            self.current_frame = DataFrameSnapshot {
                offset: block_ref.db_off,
                read_off: offset as u16,
                size: (offset + avail) as u16,
                fin: self.fin && (offset as u64 + (offset + avail) as u64 == self.fin_off),
            };
            true
        } else if self.fin && read_offset == self.fin_off {
            self.last_block = Some(block);
            self.current_frame = DataFrameSnapshot {
                offset: block_ref.db_off,
                read_off: offset as u16,
                size: offset as u16,
                fin: true,
            };
            true
        } else {
            false
        }
    }

    fn hash_grow(&mut self) -> bool {
        let old_nbits = self.nbits;
        let new_bucket_count = N_BUCKETS!(old_nbits + 1);
        let Some(new_buckets) = alloc_buckets(new_bucket_count) else {
            return false;
        };
        let old_buckets = self.buckets;

        for n in 0..N_BUCKETS!(old_nbits) {
            let old_head = unsafe { &mut *bucket_slot(old_buckets, n) };
            let mut block = old_head.first();
            while let Some(current) = block {
                block = unsafe { current.as_ref().db_next };
                unsafe { old_head.remove(current) };
                let idx = (BUCKNO!(old_nbits + 1, unsafe { current.as_ref().db_off })
                    >> old_nbits as usize)
                    & 1;
                let bucket_idx = if idx == 0 { n } else { n + N_BUCKETS!(old_nbits) };
                unsafe { (&mut *bucket_slot(new_buckets, bucket_idx)).insert_tail(current) };
            }
        }

        self.nbits = old_nbits + 1;
        dealloc_buckets(old_buckets, N_BUCKETS!(old_nbits));
        self.buckets = new_buckets;
        true
    }

    fn hash_insert(&mut self, block: NonNull<DataBlock>) -> bool {
        if self.count >= N_BUCKETS!(self.nbits) / 2 && !self.hash_grow() {
            return false;
        }

        let buckno = BUCKNO!(self.nbits, unsafe { block.as_ref().db_off });
        unsafe { (&mut *bucket_slot(self.buckets, buckno)).insert_tail(block) };
        self.count += 1;
        true
    }

    fn hash_find(&self, off: u64) -> Option<NonNull<DataBlock>> {
        let buckno = BUCKNO!(self.nbits, off);
        let mut block = unsafe { (&*bucket_slot(self.buckets, buckno)).first() };

        while let Some(candidate) = block {
            if unsafe { candidate.as_ref().db_off } == off {
                return Some(candidate);
            }
            block = unsafe { candidate.as_ref().db_next };
        }

        None
    }

    fn hash_remove(&mut self, block: NonNull<DataBlock>) {
        let buckno = BUCKNO!(self.nbits, unsafe { block.as_ref().db_off });
        unsafe { (&mut *bucket_slot(self.buckets, buckno)).remove(block) };
        self.count -= 1;
    }

    fn new_block(&mut self, off: u64) -> Option<NonNull<DataBlock>> {
        assert_eq!(off % DB_DATA_SIZE as u64, 0);

        let block = alloc_block(off)?;
        if !self.hash_insert(block) {
            unsafe { free_block(block) };
            return None;
        }
        Some(block)
    }
}

impl Drop for HashDataIn {
    fn drop(&mut self) {
        for n in 0..self.bucket_count() {
            let head = unsafe { &mut *bucket_slot(self.buckets, n) };
            while let Some(block) = head.first() {
                unsafe { head.remove(block) };
                unsafe { free_block(block) };
            }
        }
        dealloc_buckets(self.buckets, self.bucket_count());
        self.count = 0;
        self.last_block = None;
    }
}

// similar to TAILQ_INIT
fn alloc_buckets(count: usize) -> Option<NonNull<DblockHead>> {
    let layout = buckets_layout(count);
    let ptr = NonNull::new(unsafe { alloc::alloc(layout).cast::<DblockHead>() })?;

    for idx in 0..count {
        let slot = unsafe { bucket_slot(ptr, idx) };
        unsafe {
            ptr::addr_of_mut!((*slot).first).write(None);
            ptr::addr_of_mut!((*slot).last).write(ptr::addr_of_mut!((*slot).first));
        }
    }

    Some(ptr)
}

fn buckets_layout(count: usize) -> Layout {
    Layout::array::<DblockHead>(count).expect("bucket array layout must be representable")
}

unsafe fn bucket_slot(buckets: NonNull<DblockHead>, idx: usize) -> *mut DblockHead {
    unsafe { buckets.as_ptr().add(idx) }
}

fn dealloc_buckets(buckets: NonNull<DblockHead>, count: usize) {
    unsafe { alloc::dealloc(buckets.as_ptr().cast::<u8>(), buckets_layout(count)) };
}

fn alloc_block(off: u64) -> Option<NonNull<DataBlock>> {
    let ptr = NonNull::new(unsafe { alloc::alloc(Layout::new::<DataBlock>()).cast::<DataBlock>() })?;
    unsafe {
        ptr::addr_of_mut!((*ptr.as_ptr()).db_next).write(None);
        ptr::addr_of_mut!((*ptr.as_ptr()).db_prev).write(ptr::null_mut());
        ptr::addr_of_mut!((*ptr.as_ptr()).db_off).write(off);
        ptr::addr_of_mut!((*ptr.as_ptr()).db_set).write([0; N_DB_SETS]);
        ptr::addr_of_mut!((*ptr.as_ptr()).db_data).write(MaybeUninit::uninit());
    }
    Some(ptr)
}

unsafe fn free_block(block: NonNull<DataBlock>) {
    unsafe { alloc::dealloc(block.as_ptr().cast::<u8>(), Layout::new::<DataBlock>()) };
}

fn my_log2(sz: u32) -> u8 {
    (u32::BITS - sz.leading_zeros()) as u8
}

fn block_write(block: &mut DataBlock, mut block_off: usize, data: &[u8], data_sz: usize) -> usize {
    // Mirrors `block_write()`.
    if data_sz == 0 {
        return 0;
    }

    let mut data_sz = data_sz.min(DB_DATA_SIZE - block_off);
    let mut cursor = 0usize;
    let mut set = block_off >> 6;
    let bit = block_off & 0x3F;
    let data_ptr = block.db_data.as_mut_ptr().cast::<u8>();

    if bit != 0 {
        let mut n = 64 - bit;
        if n > data_sz {
            n = data_sz;
        }

        let mask = !((1u64 << bit) - 1)
            & ((1u64 << (bit + n - 1)) | ((1u64 << (bit + n - 1)) - 1));

        block.db_set[set] |= mask;
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr().add(cursor), data_ptr.add(block_off), n);
        }
        cursor += n;
        block_off += n;
        data_sz -= n;
        set += 1;
    }

    let n_full_sets = data_sz >> 6;
    if n_full_sets > 0 {
        let full_len = n_full_sets * 64;
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(cursor),
                data_ptr.add(block_off),
                full_len,
            );
        }
        cursor += full_len;
        block_off += full_len;
        data_sz -= full_len;

        for word in &mut block.db_set[set..set + n_full_sets] {
            *word = u64::MAX;
        }

        set += n_full_sets;
    }

    if data_sz > 0 {
        block.db_set[set] |= (1u64 << data_sz) - 1;
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr().add(cursor),
                data_ptr.add(block_off),
                data_sz,
            );
        }
        cursor += data_sz;
    }

    cursor
}

fn block_mark(block: &mut DataBlock, block_off: usize, data_sz: usize) -> usize {
    if data_sz == 0 {
        return 0;
    }

    let mut data_sz = data_sz.min(DB_DATA_SIZE - block_off);
    let mut cursor = 0usize;
    let mut set = block_off >> 6;
    let bit = block_off & 0x3F;

    if bit != 0 {
        let mut n = 64 - bit;
        if n > data_sz {
            n = data_sz;
        }

        let mask = !((1u64 << bit) - 1)
            & ((1u64 << (bit + n - 1)) | ((1u64 << (bit + n - 1)) - 1));

        block.db_set[set] |= mask;
        cursor += n;
        data_sz -= n;
        set += 1;
    }

    let n_full_sets = data_sz >> 6;
    if n_full_sets > 0 {
        let full_len = n_full_sets * 64;
        cursor += full_len;
        data_sz -= full_len;

        for word in &mut block.db_set[set..set + n_full_sets] {
            *word = u64::MAX;
        }

        set += n_full_sets;
    }

    if data_sz > 0 {
        block.db_set[set] |= (1u64 << data_sz) - 1;
        cursor += data_sz;
    }

    cursor
}

fn has_bytes_after(block: &DataBlock, off: usize) -> bool {
    // Mirrors `has_bytes_after()`.
    let mut set = off >> 6;
    let bit = off & 0x3F;

    let mut has = (block.db_set[set] >> bit) != 0;
    set += 1;

    while set < N_DB_SETS {
        has |= block.db_set[set] != 0;
        set += 1;
    }

    has
}

fn n_avail_bytes(block: &DataBlock, mut set: usize, bit: usize) -> usize {
    // Mirrors `n_avail_bytes()`.
    let part = !(block.db_set[set] >> bit);
    let mut count = if part != 0 {
        let count = part.trailing_zeros() as usize;
        if count < 64 - bit {
            return count;
        }
        count
    } else {
        64
    };

    set += 1;

    while set < N_DB_SETS && block.db_set[set] == u64::MAX {
        count += 64;
        set += 1;
    }

    if set < N_DB_SETS {
        let part = !block.db_set[set];
        count += if part != 0 {
            part.trailing_zeros() as usize
        } else {
            64
        };
    }

    count
}
