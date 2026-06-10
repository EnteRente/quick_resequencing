/// Port of Linux TCP receive-side reordering/resequencing logic:
/// https://elixir.bootlin.com/linux/v6.19.6/source/net/ipv4/tcp_input.c
///
/// Allocation-minimized model of Linux TCP's out-of-order queue behavior.
/// Focuses on the OFO sorting/promotion path:
/// - tcp_data_queue_ofo()
/// - tcp_ofo_queue()
///
/// It does not model sk_receive_queue. Instead, it tracks when contiguous data
/// becomes readable via `rcv_nxt` / `copied_seq`.
use intrusive_collections::Bound::Included;
use intrusive_collections::{intrusive_adapter, KeyAdapter, RBTree, RBTreeLink};
use std::{fmt, ptr::NonNull};

/// Linux defaults this to CONFIG_MAX_SKB_FRAGS, commonly 17.
///
/// The benchmark does not model actual skb memory layout, so each synthetic
/// data packet is treated as one skb fragment for receive-side coalescing.
const MAX_SKB_FRAGS: usize = 17;

/// Minimal skb model for resequencing behavior.
///
/// [seq, end_seq)
pub struct SkBuffNode {
    link: RBTreeLink,
    /// Inclusive start sequence.
    pub seq: u32,
    /// Exclusive end sequence. Includes FIN sequence space when `fin` is set.
    pub end_seq: u32,
    pub fin: bool,
    frag_count: usize,
}

impl SkBuffNode {
    pub fn new(seq: u32, len_bytes: u32, fin: bool) -> Box<Self> {
        let end_seq = seq.wrapping_add(len_bytes).wrapping_add(u32::from(fin));

        Box::new(Self {
            link: RBTreeLink::new(),
            seq,
            end_seq,
            fin,
            frag_count: usize::from(len_bytes > 0),
        })
    }

    #[inline]
    pub fn len_bytes(&self) -> usize {
        self.end_seq
            .wrapping_sub(self.seq)
            .wrapping_sub(u32::from(self.fin)) as usize
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

/// OFO-only TCP receive model.
pub struct TcpOfoQueue {
    out_of_order_queue: RBTree<SkBuffAdapter>,
    /// Fast path -> cached right-most OFO skb (Linux: tp->ooo_last_skb).
    ooo_last_skb: Option<NonNull<SkBuffNode>>,
    /// Next in-order byte available after OFO promotion.
    pub rcv_nxt: u32,
    /// Next byte to be consumed.
    copied_seq: u32,

    ooo_bytes: usize,
    ooo_segments: usize,
}

impl TcpOfoQueue {
    pub fn new(initial_seq: u32) -> Self {
        Self {
            out_of_order_queue: RBTree::new(SkBuffAdapter::new()),
            ooo_last_skb: None,
            rcv_nxt: initial_seq,
            copied_seq: initial_seq,
            ooo_bytes: 0,
            ooo_segments: 0,
        }
    }

    /// Insert segment metadata only.
    ///
    /// Returns true if the segment changed the OFO/in-order state, false if it
    /// was fully duplicate or old.
    pub fn insert_len(&mut self, seq: u32, len_bytes: u32, fin: bool) -> bool {
        let skb = SkBuffNode::new(seq, len_bytes, fin); //TODO: should be allocated outside for benchmark

        if skb.seq == skb.end_seq && !skb.fin {
            return false;
        }

        // In-order insert.
        if skb.seq == self.rcv_nxt {
            self.rcv_nxt = skb.end_seq;

            // Check for now possible contiguous data in the ofo queue.
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

    #[inline]
    pub fn ready(&self) -> bool {
        after(self.rcv_nxt, self.copied_seq)
    }

    /// Constant-time consume of all newly contiguous bytes.
    ///
    /// Returns false when progress was made, true when nothing was ready.
    pub fn drain(&mut self) -> bool {
        if !self.ready() {
            return true;
        }

        self.copied_seq = self.rcv_nxt;
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

    /// Port of the OFO promotion loop from tcp_ofo_queue().
    ///
    /// - looks at the leftmost OFO skb
    /// - stops if there is still a gap
    /// - removes OFO skbs that are now contiguous
    /// - drops already-covered duplicates
    /// - advances rcv_nxt
    /// - clears the OFO queue on FIN
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

            self.rcv_nxt = skb.end_seq;

            if skb.fin {
                self.out_of_order_queue.clear();
                self.ooo_bytes = 0;
                self.ooo_segments = 0;
                self.ooo_last_skb = None;
                break;
            }
        }
    }

    /// Port of the OFO insertion/merge logic from tcp_data_queue_ofo().
    fn data_queue_ofo(&mut self, skb: Box<SkBuffNode>) -> bool {
        if self.ooo_segments == 0 {
            self.insert_ofo(skb);
            return true;
        }

        // Linux caches the right-most skb itself, which avoids a O(logn) tree search
        // on the common tail append/coalesce path.
        if let Some(last_ptr) = self.ooo_last_skb {
            let last = unsafe { last_ptr.as_ref() };

            // Typical case
            if skb.seq == last.end_seq {
                if Self::can_coalesce(last, &skb) {
                    self.coalesce_with_ptr(last_ptr, skb);
                } else {
                    self.insert_ofo_after(last_ptr, skb);
                }
                return true;
            }

            if !before(skb.seq, last.end_seq) {
                self.insert_ofo_after(last_ptr, skb);
                return true;
            }
        }

        let prev = self
            .out_of_order_queue
            .upper_bound(Included(&skb.seq))
            .get()
            .map(|p| (NonNull::from(p), p.seq, p.end_seq));

        if let Some((prev_ptr, prev_seq, prev_end)) = prev {
            if before(skb.seq, prev_end) {
                if !after(skb.end_seq, prev_end) {
                    return false;
                }

                if skb.seq == prev_seq {
                    let current_ptr = self
                        .replace_ofo_by_ptr(prev_ptr, skb)
                        .expect("cursor from cached predecessor must be valid");
                    self.merge_right_from(current_ptr);
                    return true;
                }

                let mut skb = skb;
                skb.seq = prev_end;

                if skb.seq == skb.end_seq && !skb.fin {
                    return false;
                }

                let current_ptr = if Self::can_coalesce(unsafe { prev_ptr.as_ref() }, &skb) {
                    self.coalesce_with_ptr(prev_ptr, skb)
                } else {
                    self.insert_ofo_after(prev_ptr, skb)
                };
                self.merge_right_from(current_ptr);
                return true;
            }

            if skb.seq == prev_end {
                let current_ptr = if Self::can_coalesce(unsafe { prev_ptr.as_ref() }, &skb) {
                    self.coalesce_with_ptr(prev_ptr, skb)
                } else {
                    self.insert_ofo_after(prev_ptr, skb)
                };
                self.merge_right_from(current_ptr);
                return true;
            }
        }

        let current_ptr = self.insert_ofo(skb);
        self.merge_right_from(current_ptr);
        true
    }

    #[inline]
    fn can_coalesce(prev: &SkBuffNode, skb: &SkBuffNode) -> bool {
        prev.end_seq == skb.seq && prev.frag_count + skb.frag_count <= MAX_SKB_FRAGS
    }

    fn coalesce_with_ptr(
        &mut self,
        prev_ptr: NonNull<SkBuffNode>,
        skb: Box<SkBuffNode>,
    ) -> NonNull<SkBuffNode> {
        let added_bytes = skb.len_bytes();
        let end_seq = skb.end_seq;
        let fin = skb.fin;
        let mut prev_ptr = prev_ptr;
        let prev = unsafe { prev_ptr.as_mut() };

        debug_assert_eq!(prev.end_seq, skb.seq);
        debug_assert!(Self::can_coalesce(prev, &skb));

        prev.end_seq = end_seq;
        prev.fin |= fin;
        prev.frag_count += skb.frag_count;
        self.ooo_bytes += added_bytes;

        prev_ptr
    }

    fn insert_ofo(&mut self, skb: Box<SkBuffNode>) -> NonNull<SkBuffNode> {
        let seq = skb.seq;
        let ptr = NonNull::from(skb.as_ref());
        self.ooo_bytes += skb.len_bytes();
        self.ooo_segments += 1;
        self.out_of_order_queue.insert(skb);

        // Update the cached right-most skb.
        match self.ooo_last_skb {
            None => self.ooo_last_skb = Some(ptr),
            Some(last_ptr) => {
                if !before(seq, unsafe { last_ptr.as_ref().seq }) {
                    self.ooo_last_skb = Some(ptr);
                }
            }
        }

        ptr
    }

    fn insert_ofo_after(
        &mut self,
        after_ptr: NonNull<SkBuffNode>,
        skb: Box<SkBuffNode>,
    ) -> NonNull<SkBuffNode> {
        let ptr = NonNull::from(skb.as_ref());
        self.ooo_bytes += skb.len_bytes();
        self.ooo_segments += 1;

        let mut cursor = unsafe {
            self.out_of_order_queue
                .cursor_mut_from_ptr(after_ptr.as_ptr())
        };
        cursor.insert_after(skb);

        if self.ooo_last_skb == Some(after_ptr) {
            self.ooo_last_skb = Some(ptr);
        }

        ptr
    }

    fn replace_ofo_by_ptr(
        &mut self,
        current_ptr: NonNull<SkBuffNode>,
        skb: Box<SkBuffNode>,
    ) -> Option<NonNull<SkBuffNode>> {
        let ptr = NonNull::from(skb.as_ref());
        let new_len = skb.len_bytes();

        let removed = {
            let mut cursor = unsafe {
                self.out_of_order_queue
                    .cursor_mut_from_ptr(current_ptr.as_ptr())
            };
            cursor.replace_with(skb).ok()?
        };

        self.ooo_bytes = self
            .ooo_bytes
            .saturating_sub(removed.len_bytes())
            .saturating_add(new_len);

        if self.ooo_last_skb == Some(current_ptr) {
            self.ooo_last_skb = Some(ptr);
        }

        Some(ptr)
    }

    fn remove_ofo_by_seq(&mut self, seq: u32) -> Option<Box<SkBuffNode>> {
        let ptr = self
            .out_of_order_queue
            .find(&seq)
            .get()
            .map(NonNull::from)?;
        self.remove_ofo_by_ptr(ptr)
    }

    fn remove_ofo_by_ptr(&mut self, ptr: NonNull<SkBuffNode>) -> Option<Box<SkBuffNode>> {
        let removed = {
            let mut cursor = unsafe { self.out_of_order_queue.cursor_mut_from_ptr(ptr.as_ptr()) };
            cursor.remove()
        }?;

        self.ooo_bytes = self.ooo_bytes.saturating_sub(removed.len_bytes());
        self.ooo_segments = self.ooo_segments.saturating_sub(1);

        if self.ooo_last_skb == Some(ptr) {
            self.recompute_ooo_last();
        }

        Some(removed)
    }

    fn merge_right_from(&mut self, current_ptr: NonNull<SkBuffNode>) {
        loop {
            let current_end = unsafe { current_ptr.as_ref().end_seq };
            let next = {
                let cursor = unsafe {
                    self.out_of_order_queue
                        .cursor_from_ptr(current_ptr.as_ptr())
                };
                cursor
                    .peek_next()
                    .get()
                    .map(|skb| (NonNull::from(skb), skb.seq, skb.end_seq))
            };

            let Some((next_ptr, next_seq, next_end)) = next else {
                self.ooo_last_skb = Some(current_ptr);
                break;
            };

            if !after(current_end, next_seq) {
                break;
            }

            if before(current_end, next_end) {
                let Some(mut next) = self.remove_ofo_by_ptr(next_ptr) else {
                    break;
                };

                next.seq = current_end;

                if after(next.end_seq, next.seq) {
                    self.insert_ofo(next);
                }
                break;
            }

            let _ = self.remove_ofo_by_ptr(next_ptr);
        }
    }

    fn recompute_ooo_last(&mut self) {
        self.ooo_last_skb = self.out_of_order_queue.back().get().map(NonNull::from);
    }
}

impl fmt::Display for TcpOfoQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ofo_segments={} ofo_bytes={} rcv_nxt={} copied_seq={}",
            self.ooo_segments, self.ooo_bytes, self.rcv_nxt, self.copied_seq
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
    use super::{TcpOfoQueue, MAX_SKB_FRAGS};

    #[test]
    fn in_order_insert_flushes_ofo_queue() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert_len(100, 100, false));
        assert_eq!(q.ooo_segments(), 1);
        assert!(!q.ready());

        assert!(q.insert_len(0, 100, false));
        assert_eq!(q.rcv_nxt(), 200);
        assert_eq!(q.ooo_segments(), 0);
        assert!(q.ready());
    }

    #[test]
    fn full_duplicate_in_ofo_is_dropped() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert_len(100, 100, false));
        assert!(!q.insert_len(120, 20, false));
        assert_eq!(q.ooo_segments(), 1);
    }

    #[test]
    fn tail_coalescing_stops_at_linux_fragment_cap() {
        let mut q = TcpOfoQueue::new(0);
        let packet_len = 100;

        for i in 1..=MAX_SKB_FRAGS as u32 {
            assert!(q.insert_len(i * packet_len, packet_len, false));
        }

        assert_eq!(q.ooo_segments(), 1);
        let first = q
            .out_of_order_queue
            .front()
            .get()
            .expect("queue has one coalesced segment");
        assert_eq!(first.seq, packet_len);
        assert_eq!(first.end_seq, (MAX_SKB_FRAGS as u32 + 1) * packet_len);
        assert_eq!(first.frag_count, MAX_SKB_FRAGS);

        assert!(q.insert_len((MAX_SKB_FRAGS as u32 + 1) * packet_len, packet_len, false));

        assert_eq!(q.ooo_segments(), 2);
        let mut cursor = q.out_of_order_queue.front();
        let first = cursor.get().expect("first segment");
        let first_end = first.end_seq;
        cursor.move_next();
        let second = cursor.get().expect("second segment");
        assert_eq!(second.seq, first_end);
        assert_eq!(second.frag_count, 1);
    }

    #[test]
    fn predecessor_coalescing_stops_at_linux_fragment_cap() {
        let mut q = TcpOfoQueue::new(0);
        let packet_len = 100;

        for i in 1..=MAX_SKB_FRAGS as u32 {
            assert!(q.insert_len(i * packet_len, packet_len, false));
        }
        assert!(q.insert_len(3_000, packet_len, false));

        assert_eq!(q.ooo_segments(), 2);
        assert!(q.insert_len((MAX_SKB_FRAGS as u32 + 1) * packet_len, packet_len, false));

        assert_eq!(q.ooo_segments(), 3);
        let mut cursor = q.out_of_order_queue.front();
        let first = cursor.get().expect("first segment");
        assert_eq!(first.frag_count, MAX_SKB_FRAGS);
        cursor.move_next();
        let second = cursor.get().expect("second segment");
        assert_eq!(second.seq, first.end_seq);
        assert_eq!(second.frag_count, 1);
    }

    #[test]
    fn same_start_replaces_and_merges_right() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert_len(100, 100, false));
        assert!(q.insert_len(120, 100, false));
        assert_eq!(q.ooo_segments(), 1);

        assert!(q.insert_len(100, 150, false));
        assert_eq!(q.ooo_segments(), 1);

        let first = q
            .out_of_order_queue
            .front()
            .get()
            .expect("queue has one segment");
        assert_eq!((first.seq, first.end_seq), (100, 250));
    }

    #[test]
    fn overlap_with_previous_trims_left_and_merges() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert_len(100, 100, false));
        assert!(q.insert_len(250, 100, false));
        assert!(q.insert_len(150, 200, false));

        assert_eq!(q.ooo_segments(), 1);

        let first = q
            .out_of_order_queue
            .front()
            .get()
            .expect("queue has one segment");
        assert_eq!((first.seq, first.end_seq), (100, 350));
    }

    #[test]
    fn overlap_with_next_trims_successor() {
        let mut q = TcpOfoQueue::new(0);

        assert!(q.insert_len(100, 100, false)); // (100, 200)
        assert!(q.insert_len(50, 75, false)); // (50, 125)

        assert_eq!(q.ooo_segments(), 2);

        let mut cursor = q.out_of_order_queue.front();
        let first = cursor.get().expect("first segment");
        assert_eq!((first.seq, first.end_seq), (50, 125));

        cursor.move_next();
        let second = cursor.get().expect("second segment");
        assert_eq!((second.seq, second.end_seq), (125, 200));
    }

    #[test]
    fn drain_is_constant_time_without_receive_queue() {
        let mut q = TcpOfoQueue::new(80);

        assert!(q.insert_len(90, 20, false));
        assert!(q.insert_len(80, 20, false));

        assert_eq!(q.rcv_nxt(), 110);
        assert_eq!(q.copied_seq(), 80);
        assert!(q.ready());
        assert!(!q.drain());
        assert_eq!(q.copied_seq(), 110);
        assert!(!q.ready());
    }
}
