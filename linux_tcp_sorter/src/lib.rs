// Port of Linux TCP receive-side reordering/resequencing logic:
// https://elixir.bootlin.com/linux/v6.18.6/source/net/ipv4/tcp_input.c
//
// Main paths mirrored here:
// - tcp_data_queue_ofo()
// - tcp_ofo_queue()

use intrusive_collections::Bound::{Excluded, Included};
use intrusive_collections::{KeyAdapter, RBTree, RBTreeLink, intrusive_adapter};
use std::collections::VecDeque; // Simulate the receive queue.
use std::fmt;

/// Minimal skb model for resequencing behavior.
pub struct SkBuffNode {
    link: RBTreeLink,
    /// Inclusive start sequence.
    pub seq: u32,
    /// Exclusive end sequence.
    pub end_seq: u32,
    pub data: Vec<u8>,
    pub fin: bool,
}

impl SkBuffNode {
    pub fn new(seq: u32, data: Vec<u8>, fin: bool) -> Box<Self> {
        let end_seq = seq.wrapping_add(data.len() as u32);

        Box::new(Self {
            link: RBTreeLink::new(),
            seq,
            end_seq,
            data,
            fin,
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.data.len()
    }
}

intrusive_adapter!(SkBuffAdapter = Box<SkBuffNode>: SkBuffNode { link => RBTreeLink });

impl<'a> KeyAdapter<'a> for SkBuffAdapter {
    type Key = u32;

    #[inline]
    fn get_key(&self, skb: &'a SkBuffNode) -> u32 {
        skb.seq
    }
}

/// Linux-like receive state for OFO/resequencing.
pub struct TcpOfoQueue {
    receive_queue: VecDeque<Box<SkBuffNode>>,
    out_of_order_queue: RBTree<SkBuffAdapter>,
    /// Cached right-most OFO skb key (Linux: tp->ooo_last_skb).
    ooo_last_seq: Option<u32>,

    /// Next in-order byte expected from the peer (Linux: tp->rcv_nxt).
    rcv_nxt: u32,
    /// Next byte to deliver to the app (Linux: tp->copied_seq).
    copied_seq: u32,

    ooo_bytes: usize,
    ooo_segments: usize,
}

impl TcpOfoQueue {
    pub fn new(initial_seq: u32) -> Self {
        Self {
            receive_queue: VecDeque::new(),
            out_of_order_queue: RBTree::new(SkBuffAdapter::new()),
            ooo_last_seq: None,
            rcv_nxt: initial_seq,
            copied_seq: initial_seq,
            ooo_bytes: 0,
            ooo_segments: 0,
        }
    }

    /// Insert a segment following Linux tcp_data_queue()/tcp_data_queue_ofo().
    ///
    /// Returns true if the segment was accepted (queued/merged), false if it
    /// was fully old/duplicate and dropped.
    pub fn insert(&mut self, seq: u32, data: Vec<u8>, fin: bool) -> bool {
        let skb = SkBuffNode::new(seq, data, fin);

        if skb.seq == skb.end_seq && !skb.fin {
            return false;
        }

        if skb.seq == self.rcv_nxt {
            let end_seq = skb.end_seq;
            self.queue_rcv(skb);
            self.rcv_nxt = end_seq;

            if self.ooo_segments > 0 {
                self.ofo_queue();
            }

            return true;
        }

        if !after(skb.end_seq, self.rcv_nxt) {
            return false;
        }

        self.data_queue_ofo(skb)
    }

    /// Drain one contiguous chunk from receive_queue.
    ///
    /// Returns false when drain happened, true when no data was drainable.
    /// This matches the current benchmark loop contract.
    pub fn drain(&mut self) -> bool {
        loop {
            let Some(front) = self.receive_queue.front() else {
                return true;
            };

            if !after(front.end_seq, self.copied_seq) {
                self.receive_queue.pop_front();
                continue;
            }

            if after(front.seq, self.copied_seq) {
                return true;
            }

            let end_seq = front.end_seq;
            self.receive_queue.pop_front();
            self.copied_seq = end_seq;
            return false;
        }
    }

    pub fn ready(&self) -> bool {
        for skb in &self.receive_queue {
            if !after(skb.end_seq, self.copied_seq) {
                continue;
            }

            return !after(skb.seq, self.copied_seq);
        }

        false
    }

    pub fn rcv_nxt(&self) -> u32 {
        self.rcv_nxt
    }

    pub fn copied_seq(&self) -> u32 {
        self.copied_seq
    }

    pub fn ooo_bytes(&self) -> usize {
        self.ooo_bytes
    }

    pub fn ooo_segments(&self) -> usize {
        self.ooo_segments
    }

    fn queue_rcv(&mut self, skb: Box<SkBuffNode>) {
        let mut queued = Some(skb);

        if let Some(tail) = self.receive_queue.back_mut() {
            if let Some(incoming) = queued.as_ref() {
                if incoming.seq == tail.end_seq {
                    let incoming = queued.take().expect("checked as Some");
                    tail.data.extend_from_slice(&incoming.data);
                    tail.end_seq = incoming.end_seq;
                    tail.fin |= incoming.fin;
                }
            }
        }

        if let Some(incoming) = queued {
            self.receive_queue.push_back(incoming);
        }
    }

    /// Port of Linux tcp_ofo_queue().
    fn ofo_queue(&mut self) {
        loop {
            let first = self
                .out_of_order_queue
                .front()
                .get()
                .map(|skb| (skb.seq, skb.end_seq, skb.fin));

            let Some((seq, _, _)) = first else {
                break;
            };

            if after(seq, self.rcv_nxt) {
                break;
            }

            let Some(skb) = self.remove_ofo_by_seq(seq) else {
                break;
            };

            if !after(skb.end_seq, self.rcv_nxt) {
                continue;
            }

            let fin = skb.fin;
            let end_seq = skb.end_seq;
            self.queue_rcv(skb);
            self.rcv_nxt = end_seq;

            // Linux tcp_fin() purges OFO queue.
            if fin {
                self.out_of_order_queue.clear();
                self.ooo_bytes = 0;
                self.ooo_segments = 0;
                self.ooo_last_seq = None;
                break;
            }
        }
    }

    /// Port of Linux tcp_data_queue_ofo().
    fn data_queue_ofo(&mut self, skb: Box<SkBuffNode>) -> bool {
        if self.ooo_segments == 0 {
            self.insert_ofo(skb);
            return true;
        }

        if let Some(last_seq) = self.ooo_last_seq {
            let last_end = self
                .out_of_order_queue
                .find(&last_seq)
                .get()
                .map(|last| last.end_seq);

            if let Some(last_end) = last_end {
                if skb.seq == last_end {
                    return self.coalesce_with_seq(last_seq, skb);
                }

                if !before(skb.seq, last_end) {
                    self.insert_ofo(skb);
                    return true;
                }
            } else {
                self.recompute_ooo_last();
            }
        }

        let prev = self
            .out_of_order_queue
            .upper_bound(Included(&skb.seq))
            .get()
            .map(|p| (p.seq, p.end_seq));

        if let Some((prev_seq, prev_end)) = prev {
            if before(skb.seq, prev_end) {
                if !after(skb.end_seq, prev_end) {
                    return false;
                }

                // Same start, larger end: replace old with new.
                if !after(skb.seq, prev_seq) {
                    let _ = self.remove_ofo_by_seq(prev_seq);
                }
            } else if skb.seq == prev_end {
                return self.coalesce_with_seq(prev_seq, skb);
            }
        }

        let insert_seq = skb.seq;
        let insert_end = skb.end_seq;
        self.insert_ofo(skb);

        // merge_right: remove fully covered successors.
        loop {
            let next = self
                .out_of_order_queue
                .lower_bound(Excluded(&insert_seq))
                .get()
                .map(|s| (s.seq, s.end_seq));

            let Some((next_seq, next_end)) = next else {
                self.ooo_last_seq = Some(insert_seq);
                break;
            };

            if !after(insert_end, next_seq) {
                break;
            }

            // Keep partial overlap (Linux behavior).
            if before(insert_end, next_end) {
                break;
            }

            let _ = self.remove_ofo_by_seq(next_seq);
        }

        true
    }

    fn coalesce_with_seq(&mut self, prev_seq: u32, skb: Box<SkBuffNode>) -> bool {
        let Some(mut prev) = self.remove_ofo_by_seq(prev_seq) else {
            return false;
        };

        if prev.end_seq != skb.seq {
            self.insert_ofo(prev);
            self.insert_ofo(skb);
            return true;
        }

        prev.data.extend_from_slice(&skb.data);
        prev.end_seq = skb.end_seq;
        prev.fin |= skb.fin;
        self.insert_ofo(prev);
        true
    }

    fn insert_ofo(&mut self, skb: Box<SkBuffNode>) {
        let seq = skb.seq;
        self.ooo_bytes += skb.len();
        self.ooo_segments += 1;
        self.out_of_order_queue.insert(skb);

        match self.ooo_last_seq {
            None => self.ooo_last_seq = Some(seq),
            Some(last_seq) => {
                if !before(seq, last_seq) {
                    self.ooo_last_seq = Some(seq);
                }
            }
        }
    }

    fn remove_ofo_by_seq(&mut self, seq: u32) -> Option<Box<SkBuffNode>> {
        let removed = {
            let mut cursor = self.out_of_order_queue.find_mut(&seq);
            cursor.remove()
        }?;

        self.ooo_bytes = self.ooo_bytes.saturating_sub(removed.len());
        self.ooo_segments = self.ooo_segments.saturating_sub(1);

        if self.ooo_last_seq == Some(seq) {
            self.recompute_ooo_last();
        }

        Some(removed)
    }

    fn recompute_ooo_last(&mut self) {
        self.ooo_last_seq = self.out_of_order_queue.back().get().map(|skb| skb.seq);
    }
}

impl fmt::Display for TcpOfoQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "recv={} ofo_segments={} ofo_bytes={}",
            self.receive_queue.len(),
            self.ooo_segments,
            self.ooo_bytes
        )
    }
}

#[inline]
fn before(seq1: u32, seq2: u32) -> bool {
    (seq1.wrapping_sub(seq2) as i32) < 0
}

#[inline]
fn after(seq1: u32, seq2: u32) -> bool {
    before(seq2, seq1)
}



#[cfg(test)]
mod tests {
    use super::TcpOfoQueue;

    fn payload(len: usize) -> Vec<u8> {
        vec![0_u8; len]
    }

    #[test]
    fn in_order_insert_flushes_ofo_queue() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert(100, payload(100), false));
        assert_eq!(q.ooo_segments(), 1);
        assert!(!q.ready());

        assert!(q.insert(0, payload(100), false));
        assert_eq!(q.rcv_nxt(), 200);
        assert_eq!(q.ooo_segments(), 0);
        assert!(q.ready());
    }

    #[test]
    fn full_duplicate_in_ofo_is_dropped() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert(100, payload(100), false));
        assert!(!q.insert(120, payload(20), false));
        assert_eq!(q.ooo_segments(), 1);
    }

    #[test]
    fn same_start_replaces_and_merges_right() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert(100, payload(100), false)); // [100, 200)
        assert!(q.insert(120, payload(100), false)); // [120, 220)
        assert_eq!(q.ooo_segments(), 2);

        assert!(q.insert(100, payload(150), false)); // [100, 250)
        assert_eq!(q.ooo_segments(), 1);

        let first = q
            .out_of_order_queue
            .front()
            .get()
            .expect("queue has one segment");
        assert_eq!((first.seq, first.end_seq), (100, 250));
    }

    #[test]
    fn overlapping_ofo_segment_kept_and_drained_without_dup_delivery() {
        let mut q = TcpOfoQueue::new(80);

        assert!(q.insert(90, payload(20), false)); // [90, 110) in OFO
        assert!(q.insert(80, payload(20), false)); // [80, 100), then OFO promoted
        assert_eq!(q.rcv_nxt(), 110);
        assert_eq!(q.receive_queue.len(), 2);

        assert!(q.ready());
        assert!(!q.drain());
        assert!(q.ready());
        assert!(!q.drain());
        assert_eq!(q.copied_seq(), 110);
        assert!(!q.ready());
    }
}
