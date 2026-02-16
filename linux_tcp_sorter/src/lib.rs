mod tcp_input;

use intrusive_collections::{intrusive_adapter, RBTree, RBTreeLink, KeyAdapter};
use std::fmt;

/// Socket buffer node with intrusive RB-tree link
pub struct SkBuffNode {
    link: RBTreeLink,
    /// TCP sequence number (start)
    pub seq: u32,
    /// TCP sequence number (end, exclusive)
    pub end_seq: u32,
    /// Actual data payload
    pub data: Vec<u8>,
    /// FIN flag
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

// Adapter to use SkBuffNode in an RBTree, keyed by end_seq (like Linux)
intrusive_adapter!(
    SkBuffAdapter = Box<SkBuffNode>: SkBuffNode { link => RBTreeLink });

impl<'a> KeyAdapter<'a> for SkBuffAdapter {
    type Key = u32;

    fn get_key(&self, node: &'a SkBuffNode) -> u32 {
        node.end_seq
    }
}

/// TCP out-of-order queue using intrusive RB-tree
pub struct TcpOfoQueue {
    tree: RBTree<SkBuffAdapter>,
    /// Next expected sequence number
    rcv_nxt: u32,
    /// Bytes in OOO queue
    ooo_bytes: usize,
}

impl TcpOfoQueue {
    pub fn new(initial_seq: u32) -> Self {
        Self {
            tree: RBTree::new(SkBuffAdapter::new()),
            rcv_nxt: initial_seq,
            ooo_bytes: 0,
        }
    }

    /// Insert segment - returns true if data was in-order
    pub fn insert(&mut self, seq: u32, data: Vec<u8>, fin: bool) -> bool {
        let end_seq = seq.wrapping_add(data.len() as u32);

        // Discard old/duplicate data
        if self.before(end_seq, self.rcv_nxt) || end_seq == self.rcv_nxt {
            return false;
        }

        // In-order segment
        if seq == self.rcv_nxt {
            self.rcv_nxt = end_seq;
            self.coalesce();
            return true;
        }

        // Out-of-order: insert into tree
        let node = SkBuffNode::new(seq, data, fin);
        self.ooo_bytes += node.len();
        self.tree.insert(node);

        false
    }

    /// Coalesce contiguous segments from OOO queue
    fn coalesce(&mut self) {
        loop {
            let mut cursor = self.tree.front_mut();

            match cursor.get() {
                Some(node) if node.seq == self.rcv_nxt => {
                    self.rcv_nxt = node.end_seq;
                    self.ooo_bytes -= node.len();
                    cursor.remove();
                }
                _ => break,
            }
        }
    }

    #[inline]
    fn before(&self, seq1: u32, seq2: u32) -> bool {
        (seq1.wrapping_sub(seq2) as i32) < 0
    }

    pub fn rcv_nxt(&self) -> u32 {
        self.rcv_nxt
    }

    pub fn ooo_bytes(&self) -> usize {
        self.ooo_bytes
    }

    pub fn ooo_segments(&self) -> usize {
        self.tree.iter().count()
    }

    /// Check if data is ready to be read
    pub fn ready(&self) -> bool {
        self.rcv_nxt > 0 // Simplified: real impl would track delivered bytes
    }

}



//TODO
impl fmt::Display for TcpOfoQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OOO queue: {} bytes, {} segments", self.ooo_bytes, self.ooo_segments())
    }
}