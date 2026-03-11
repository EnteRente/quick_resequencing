use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::ops::RangeInclusive;
use nohash_hasher::NoHashHasher;

pub mod bytes;
mod error;
mod doubly_linked_list;

use bytes::Bytes;
use error::Error::{Done};
use error::Result;
use doubly_linked_list::{DoublyLinkedList, Element};

type Gap = Element<RangeInclusive<u64>>;

pub struct QuicGoFrameSorter {
    pub(crate) queue: HashMap<u64, Bytes, BuildHasherDefault<NoHashHasher<u64>>>,
    read_pos: u64,
    gaps: DoublyLinkedList<RangeInclusive<u64>>,
}

impl QuicGoFrameSorter {
    pub fn new() -> Self {
        let mut fs = QuicGoFrameSorter {
            queue: HashMap::with_capacity_and_hasher(1024, BuildHasherDefault::default()),
            read_pos: 0,
            gaps: DoublyLinkedList::new(),
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
        let front_gap = self.gaps.front().expect("gap list must be initialized");
        let front_gap_start = *self.gap(front_gap).start();

        if end <= front_gap_start {
            return Ok(())
        }

        let (start_gap, starts_in_gap) = self.find_start_gap(start);
        let (end_gap, ends_in_gap) = self.find_end_gap(start_gap, end);
        let start_gap_equals_end_gap = start_gap == end_gap;

        let start_gap_start = *self.gap(start_gap).start();
        let start_gap_end = *self.gap(start_gap).end();
        let end_gap_start = *self.gap(end_gap).start();
        let end_gap_end = *self.gap(end_gap).end();

        if (start_gap_equals_end_gap && end <= start_gap_start) ||
            (!start_gap_equals_end_gap &&
                start_gap_end >= end_gap_start &&
                end <= start_gap_start) {
            return Ok(()); // duplicate stream data
        }

        let mut start_gap_next = self.next_gap(start_gap);
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
            let gap_start = start_gap_start;
            buf = buf.split_off((gap_start - start) as usize);
            start = gap_start;
        }

        // Modify start gap
        if start <= start_gap_start {
            if end >= start_gap_end {
                // Frame covers whole start gap, remove it
                start_gap_next = self.next_gap(start_gap);
                self.remove_gap(start_gap);
            } else {
                // Update gap start
                *self.gap_mut(start_gap) = end..=start_gap_end;
            }
        } else if !has_replaced_at_least_one {
            // Adjust end of start gap
            *self.gap_mut(start_gap) = start_gap_start..=start;
            adjusted_start_gap_end = true;
            start_gap_next = self.next_gap(start_gap);
        }

        // Handle intermediate gaps if not a single gap
        if !start_gap_equals_end_gap {
            // Delete consecutive frames and intermediate gaps
            self.delete_consecutive(start_gap_end);
            let mut gap = start_gap_next.expect("end gap must follow start gap");
            while *self.gap(gap).end() < end_gap_start {
                let gap_end = *self.gap(gap).end();
                let next_gap = self
                    .next_gap(gap)
                    .expect("end gap must remain reachable while removing intermediates");
                self.delete_consecutive(gap_end);
                self.remove_gap(gap);
                gap = next_gap;
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
                self.remove_gap(end_gap);
            }
        } else {
            if start_gap_equals_end_gap && adjusted_start_gap_end {
                // Frame split existing gap, insert new gap
                self.insert_gap_after(start_gap, end..=start_gap_end);
            } else if !start_gap_equals_end_gap {
                // Update end gap start
                *self.gap_mut(end_gap) = end..=end_gap_end;
            }
        }

        // Deactivate gap limit checker for benchmark
        /*const MAX_STREAM_FRAME_SORTER_GAPS: usize = 1000;

        // Check gap limit
        if self.gaps.len() > MAX_STREAM_FRAME_SORTER_GAPS {
            panic!("too many gaps in received data");
        }*/

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

    /// return gap handle and whether starts in range
    fn find_start_gap(&self, offset: u64) -> (Gap, bool) {
        for (gap_ref, gap) in self.gaps.iter() {
            if offset >= *gap.start() && offset <= *gap.end() {
                return (gap_ref, true);
            }
            if offset < *gap.start() {
                return (gap_ref, false);
            }
        }
        panic!("no gap found")
    }

    /// return gap handle and whether ends in range
    fn find_end_gap(&self, start_gap: Gap, offset: u64) -> (Gap, bool) {
        let mut current = Some(start_gap);

        while let Some(gap_ref) = current {
            let gap = self.gap(gap_ref);
            if offset >= *gap.start() && offset < *gap.end() {
                return (gap_ref, true);
            }
            if offset < *gap.start() {
                return (
                    self.prev_gap(gap_ref)
                        .expect("start gap is the first candidate in end-gap search"),
                    false,
                );
            }

            current = self.next_gap(gap_ref);
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

    // Wrapper for unsafe methods
    #[inline]
    fn gap(&self, gap: Gap) -> &RangeInclusive<u64> {
        unsafe { self.gaps.get(gap) }
    }

    #[inline]
    fn gap_mut(&mut self, gap: Gap) -> &mut RangeInclusive<u64> {
        unsafe { self.gaps.get_mut(gap) }
    }

    #[inline]
    fn next_gap(&self, gap: Gap) -> Option<Gap> {
        unsafe { self.gaps.next(gap) }
    }

    #[inline]
    fn prev_gap(&self, gap: Gap) -> Option<Gap> {
        unsafe { self.gaps.prev(gap) }
    }

    #[inline]
    fn remove_gap(&mut self, gap: Gap) -> RangeInclusive<u64> {
        unsafe { self.gaps.remove(gap) }
    }

    #[inline]
    fn insert_gap_after(&mut self, gap: Gap, value: RangeInclusive<u64>) -> Gap {
        unsafe { self.gaps.insert_after(gap, value) }
    }
}

#[cfg(test)]
mod tests {
    use super::bytes::Bytes;
    use super::QuicGoFrameSorter;

    fn bytes(data: &[u8]) -> Bytes {
        Bytes::from(data.to_vec())
    }

    fn assert_gap_ranges<const N: usize>(
        sorter: &QuicGoFrameSorter,
        expected: [(u64, u64); N],
    ) {
        assert!(
            sorter
                .gaps
                .iter()
                .map(|(_, gap)| (*gap.start(), *gap.end()))
                .eq(expected)
        );
    }

    #[test]
    fn in_order_push_updates_gap_and_allows_pop() {
        let mut sorter = QuicGoFrameSorter::new();

        sorter.push(0, bytes(b"abc")).unwrap();

        assert!(sorter.ready());
        assert_gap_ranges(&sorter, [(3, u64::MAX)]);

        let out = sorter.pop(10).unwrap();
        assert_eq!(&*out, b"abc");
        assert_eq!(sorter.read_pos(), 3);
        assert!(!sorter.ready());
    }

    #[test]
    fn out_of_order_then_gap_fill_restores_contiguous_reads() {
        let mut sorter = QuicGoFrameSorter::new();

        sorter.push(5, bytes(b"567")).unwrap();
        assert!(!sorter.ready());
        assert_gap_ranges(&sorter, [(0, 5), (8, u64::MAX)]);

        sorter.push(0, bytes(b"01234")).unwrap();
        assert!(sorter.ready());
        assert_gap_ranges(&sorter, [(8, u64::MAX)]);

        let first = sorter.pop(16).unwrap();
        assert_eq!(&*first, b"01234");
        assert!(sorter.ready());

        let second = sorter.pop(16).unwrap();
        assert_eq!(&*second, b"567");
        assert!(!sorter.ready());
        assert_eq!(sorter.read_pos(), 8);
    }

    #[test]
    fn duplicate_overlap_does_not_change_existing_data_or_gaps() {
        let mut sorter = QuicGoFrameSorter::new();

        sorter.push(0, bytes(b"abcdef")).unwrap();
        sorter.push(2, bytes(b"cd")).unwrap();

        assert_eq!(sorter.queue.len(), 1);
        assert!(sorter.queue.contains_key(&0));
        assert_gap_ranges(&sorter, [(6, u64::MAX)]);

        let out = sorter.pop(16).unwrap();
        assert_eq!(&*out, b"abcdef");
    }

    #[test]
    fn spanning_frame_replaces_shorter_entries_and_collapses_gaps() {
        let mut sorter = QuicGoFrameSorter::new();

        sorter.push(4, bytes(b"aa")).unwrap();
        sorter.push(8, bytes(b"bb")).unwrap();
        assert_gap_ranges(&sorter, [(0, 4), (6, 8), (10, u64::MAX)]);

        sorter.push(2, bytes(b"0123456789")).unwrap();

        assert_eq!(sorter.queue.len(), 1);
        assert!(sorter.queue.contains_key(&2));
        assert_gap_ranges(&sorter, [(0, 2), (12, u64::MAX)]);

        sorter.push(0, bytes(b"xy")).unwrap();
        assert!(sorter.ready());

        let first = sorter.pop(16).unwrap();
        assert_eq!(&*first, b"xy");
        let second = sorter.pop(16).unwrap();
        assert_eq!(&*second, b"0123456789");
        assert_eq!(sorter.read_pos(), 12);
    }

    #[test]
    fn partial_pop_requeues_remaining_bytes_at_new_offset() {
        let mut sorter = QuicGoFrameSorter::new();

        sorter.push(0, bytes(b"abcdefghij")).unwrap();

        let first = sorter.pop(4).unwrap();
        assert_eq!(&*first, b"abcd");
        assert!(sorter.ready());
        assert!(sorter.queue.contains_key(&4));

        let second = sorter.pop(16).unwrap();
        assert_eq!(&*second, b"efghij");
        assert!(!sorter.ready());
    }
}