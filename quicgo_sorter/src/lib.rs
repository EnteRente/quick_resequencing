use std::collections::{HashMap, VecDeque};
use std::hash::BuildHasherDefault;
use std::ops::RangeInclusive;
use nohash_hasher::NoHashHasher;

pub mod bytes;
mod error;
mod DoublyLinkedList;

use bytes::Bytes;
use error::Error::{Done};
use error::Result;



pub struct QuicGoFrameSorter {
    pub(crate) queue: HashMap<u64, Bytes, BuildHasherDefault<NoHashHasher<u64>>>,
    read_pos: u64,
    //TODO: Implement as Doubly Linked List | Possible reference: https://rtoch.com/posts/rust-doubly-linked-list/
    gaps: VecDeque<RangeInclusive<u64>>,
}

impl QuicGoFrameSorter {
    pub fn new() -> Self {
        let mut fs = QuicGoFrameSorter {
            queue: HashMap::with_capacity_and_hasher(1024, BuildHasherDefault::default()),
            read_pos: 0,
            gaps: VecDeque::new(),
        };
        fs.gaps.push_back(0..=u64::MAX);
        fs
    }

    #[inline]
    pub fn ready(&self) -> bool {
        self.queue.get(&self.read_pos).is_some()
    }

    pub(crate) fn clear(&mut self) {
        self.queue.clear();
        self.gaps.clear();
    }

    #[inline]
    pub(crate) fn read_pos(&self) -> u64 {
        self.read_pos
    }

    #[inline]
    pub(crate) fn set_read_pos(&mut self, v: u64) {
        self.read_pos = v;
    }

    pub fn pop(&mut self, max_size: usize) -> Result<Bytes> {
        let mut rb = match self.queue.remove(&self.read_pos) {
            Some(range_buf) => range_buf,
            None => return Err(Done)
        };
        if max_size < rb.len() {
            let out = rb.split_to(max_size);
            self.read_pos += max_size as u64;
            self.queue.insert(self.read_pos, rb);
            Ok(out)
        } else {
            self.read_pos += rb.len() as u64;
            Ok(rb)
        }
    }

    pub fn push(&mut self, offset: u64, mut buf: Bytes) -> Result<()> {
        debug_assert!(!buf.is_empty());
        debug_assert!(self.gaps.front().is_some());
        
        let mut start = offset;
        let mut end = offset + buf.len() as u64;

        if end <= *self.gaps.front().unwrap().start() {
            return Ok(())
        }

        let (start_gap_idx, starts_in_gap) = self.find_start_gap(start);
        let (mut end_gap_idx, ends_in_gap) = self.find_end_gap(start_gap_idx, end);


        let start_gap_equals_end_gap = start_gap_idx == end_gap_idx;

        if (start_gap_equals_end_gap && end <= *self.gaps[start_gap_idx].start()) ||
            (!start_gap_equals_end_gap &&
                *self.gaps[start_gap_idx].end() >= *self.gaps[end_gap_idx].start() &&
                end <= *self.gaps[start_gap_idx].start()) {
            return Ok(()); // duplicate stream data
        }

        let mut start_gap_next_idx = start_gap_idx + 1;
        let start_gap_end = *self.gaps[start_gap_idx].end();
        let end_gap_start = *self.gaps[end_gap_idx].start();
        let end_gap_end = *self.gaps[end_gap_idx].end();
        let mut adjusted_start_gap_end = false;

        let mut pos = start;
        let mut has_replaced_at_least_one = false;

        // Handle overlapping existing entries
        while let Some(old_entry) = self.queue.remove(&pos) {
            let old_entry_len = old_entry.len() as u64;

            if end - pos > old_entry_len || (has_replaced_at_least_one && end - pos == old_entry_len) {
                // Replace existing shorter frame
                pos += old_entry_len;
                has_replaced_at_least_one = true;
            } else {
                if !has_replaced_at_least_one {
                    return Ok(());
                }

                // Cut new frame to align with existing frame
                let _ = buf.split_off((pos - start) as usize);
                end = pos;
                break;
            }
        }

        // Adjust data start if not starting in a gap
        if !starts_in_gap && !has_replaced_at_least_one {
            let gap_start = *self.gaps[start_gap_idx].start();
            buf = buf.split_off((gap_start - start) as usize);
            start = gap_start;
        }

        // Modify start gap
        if start <= *self.gaps[start_gap_idx].start() {
            if end >= *self.gaps[start_gap_idx].end() {
                // Frame covers whole start gap, remove it
                self.gaps.remove(start_gap_idx);
                end_gap_idx = end_gap_idx.saturating_sub(1);
                start_gap_next_idx -= 1;
            } else {
                // Update gap start
                self.gaps[start_gap_idx] = end..=*self.gaps[start_gap_idx].end();
            }
        } else if !has_replaced_at_least_one {
            // Adjust end of start gap
            let mut new_gap = self.gaps[start_gap_idx].clone();
            new_gap = *new_gap.start()..=start;
            self.gaps[start_gap_idx] = new_gap;
            adjusted_start_gap_end = true;
        }

        // Handle intermediate gaps if not a single gap
        if !start_gap_equals_end_gap {
            // Delete consecutive frames and intermediate gaps
            self.delete_consecutive(start_gap_end);
            let gap_idx = start_gap_next_idx;
            while *self.gaps[gap_idx].end() < end_gap_start {
                self.delete_consecutive(*self.gaps[gap_idx].end());
                self.gaps.remove(gap_idx);
                end_gap_idx -= 1;
            }
        }

        // Adjust end of frame if needed
        if !ends_in_gap && start != end_gap_end && end > end_gap_end {
            let _ = buf.split_off((end_gap_end - start) as usize);
            end = end_gap_end;
        }

        // Handle end gap
        if end == end_gap_end {
            if !start_gap_equals_end_gap {
                // Frame covers whole end gap, remove it
                self.gaps.remove(end_gap_idx);
            }
        } else {
            if start_gap_equals_end_gap && adjusted_start_gap_end {
                // Frame split existing gap, insert new gap
                self.gaps.insert(
                    start_gap_idx + 1,
                    end..=start_gap_end
                );
            } else if !start_gap_equals_end_gap {
                // Update end gap start
                self.gaps[end_gap_idx] = end..=*self.gaps[end_gap_idx].end();
            }
        }

        const MAX_STREAM_FRAME_SORTER_GAPS: usize = 1000;

        // Check gap limit
        if self.gaps.len() > MAX_STREAM_FRAME_SORTER_GAPS {
            panic!("too many gaps in received data");
        }

        // Add entry to queue
        self.queue.insert(start, buf);

        Ok(())
    }

    #[inline]
    pub(crate) fn off(&self) -> u64 {
        self.read_pos
    }
}

impl QuicGoFrameSorter {

    /// return index and whether starts in range
    fn find_start_gap(&self, offset: u64) -> (usize, bool) {
        for (idx, gap) in self.gaps.iter().enumerate() {
            if offset >= *gap.start() && offset <= *gap.end() {
                return (idx, true);
            }
            if offset < *gap.start() {
                return (idx, false);
            }
        }
        panic!("no gap found")
    }

    /// return index and whether ends in range
    fn find_end_gap(&self, start_gap_idx: usize, offset: u64) -> (usize, bool) {
        for (idx, gap) in self.gaps.iter().enumerate().skip(start_gap_idx) {
            if offset >= *gap.start() && offset < *gap.end() {
                return (idx, true);
            }
            if offset < *gap.start() {
                return (idx - 1, false);
            }
        }
        panic!("no gap found")
    }

    fn delete_consecutive(&mut self, mut pos: u64) {
        loop {
            let old_entry = match self.queue.remove(&pos) {
                Some(e) => e,
                None => break,
            };
            pos += old_entry.len() as u64;
        }
    }

}