// Rust port of lsquic's receive-side reordering engine.
//
// Upstream references:
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_nocopy.c
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_hash.c
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_stream.c
//
// This file contains the stream-side implementation switch wrapper that mirrors
// how lsquic routes between `di_nocopy` and `di_hash`.
mod frame;
mod hash;
mod list;
mod nocopy;

use std::mem;
use std::sync::{Arc, OnceLock};

pub use frame::{DataFrameSnapshot, InsFrame, StreamFrame, StreamId};
pub use hash::{HashDataIn, DB_DATA_SIZE, N_DB_SETS};
pub use nocopy::{
    EFF_CHECK_THRESH_HIGH, EFF_CHECK_THRESH_LOW, EFF_FAR_TRAVERSE_COUNT, EFF_MAX_HOLES,
    EFF_TINY_FRAME_SZ, NoCopyDataIn,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeKind {
    NoCopy,
    Hash,
}

#[derive(Debug)]
enum Implementation {
    NoCopy(NoCopyDataIn),
    Hash(HashDataIn),
}

#[derive(Debug)]
pub struct ReassemblyEngine {
    mode: Implementation,
    read_offset: u64,
    auto_switch: bool,
    stream_id: StreamId,
}

impl ReassemblyEngine {
    pub fn new_auto(stream_id: StreamId) -> Self {
        Self {
            mode: Implementation::NoCopy(NoCopyDataIn::new(stream_id)),
            read_offset: 0,
            auto_switch: true,
            stream_id,
        }
    }

    pub fn new_nocopy(stream_id: StreamId) -> Self {
        Self {
            mode: Implementation::NoCopy(NoCopyDataIn::new(stream_id)),
            read_offset: 0,
            auto_switch: false,
            stream_id,
        }
    }

    pub fn new_hash(stream_id: StreamId) -> Self {
        Self {
            mode: Implementation::Hash(HashDataIn::new(stream_id, 0)),
            read_offset: 0,
            auto_switch: false,
            stream_id,
        }
    }

    #[inline]
    pub fn mode_kind(&self) -> ModeKind {
        match self.mode {
            Implementation::NoCopy(_) => ModeKind::NoCopy,
            Implementation::Hash(_) => ModeKind::Hash,
        }
    }

    #[inline]
    pub fn read_offset(&self) -> u64 {
        self.read_offset
    }

    #[inline]
    pub fn ready(&self) -> bool {
        match &self.mode {
            Implementation::NoCopy(di) => di.get_frame(self.read_offset).is_some(),
            Implementation::Hash(di) => di.ready_at(self.read_offset),
        }
    }

    pub fn readable_bytes(&mut self) -> u64 {
        match &mut self.mode {
            Implementation::NoCopy(di) => di.readable_bytes(self.read_offset),
            Implementation::Hash(di) => di.readable_bytes(self.read_offset),
        }
    }

    pub fn current_frame(&mut self) -> Option<DataFrameSnapshot> {
        match &mut self.mode {
            Implementation::NoCopy(di) => di.get_frame(self.read_offset),
            Implementation::Hash(di) => di.get_frame(self.read_offset),
        }
    }

    pub fn copy_current_data(&mut self, out: &mut [u8]) -> Option<usize> {
        match &mut self.mode {
            Implementation::NoCopy(di) => di.copy_current_data(self.read_offset, out),
            Implementation::Hash(di) => di.copy_current_data(self.read_offset, out),
        }
    }

    pub fn insert_frame(&mut self, frame: StreamFrame) -> InsFrame {
        // Mirrors the stream-side behavior in lsquic_stream.c:
        // try nocopy first, switch to hash on overlap, then re-insert.
        let result = match &mut self.mode {
            Implementation::NoCopy(di) => {
                let ins = di.insert_frame(frame.clone(), self.read_offset);
                if ins == InsFrame::Overlap && self.auto_switch {
                    self.switch_to_hash();
                    match &mut self.mode {
                        Implementation::Hash(di) => di.insert_frame(frame, self.read_offset),
                        Implementation::NoCopy(_) => unreachable!("switch_to_hash must switch implementation"),
                    }
                } else {
                    ins
                }
            },
            Implementation::Hash(di) => di.insert_frame(frame, self.read_offset),
        };

        if self.auto_switch {
            self.maybe_switch();
        }

        result
    }

    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        let result = match &mut self.mode {
            Implementation::NoCopy(di) => {
                let ins = di.insert_len(offset, len, fin, self.read_offset);
                if ins == InsFrame::Overlap && self.auto_switch {
                    self.switch_to_hash();
                    match &mut self.mode {
                        Implementation::Hash(di) => di.insert_len(offset, len, fin, self.read_offset),
                        Implementation::NoCopy(_) => unreachable!("switch_to_hash must switch implementation"),
                    }
                } else {
                    ins
                }
            },
            Implementation::Hash(di) => di.insert_len(offset, len, fin, self.read_offset),
        };

        if self.auto_switch {
            self.maybe_switch();
        }

        result
    }

    // For benchmarking purposes... TODO: not useful right now, allocating zeroes doesn't mitigate the problem
    pub fn insert_zero_frame(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        let zeros = shared_zero_packet();
        assert!(len <= zeros.len());
        let frame = StreamFrame::new(self.stream_id, Arc::clone(zeros), 0, offset, fin, len);
        self.insert_frame(frame)
    }

    pub fn drain_current(&mut self) -> bool {
        let Some(mut frame) = self.current_frame() else {
            return true;
        };

        self.read_offset += frame.readable_len();
        frame.mark_consumed();

        match &mut self.mode {
            Implementation::NoCopy(di) => di.frame_done(&frame),
            Implementation::Hash(di) => di.frame_done(&frame),
        }

        if self.auto_switch {
            self.maybe_switch();
        }

        false
    }

    pub fn mem_used(&self) -> usize {
        match &self.mode {
            Implementation::NoCopy(di) => di.mem_used(),
            Implementation::Hash(di) => di.mem_used(),
        }
    }

    fn maybe_switch(&mut self) {
        // Mirrors the stream wrapper's "switch implementation when DI asks"
        // behavior from lsquic_stream.c.
        let should_switch = match &self.mode {
            Implementation::NoCopy(di) => di.wants_switch(),
            Implementation::Hash(di) => di.wants_switch(),
        };

        if !should_switch {
            return;
        }

        match self.mode_kind() {
            ModeKind::NoCopy => self.switch_to_hash(),
            ModeKind::Hash => self.switch_to_nocopy(),
        }
    }

    fn switch_to_hash(&mut self) {
        // Rust equivalent of the nocopy -> hash migration path triggered by
        // lsquic_stream.c after overlap or `DI_SWITCH_IMPL`.
        let mode = mem::replace(
            &mut self.mode,
            Implementation::Hash(HashDataIn::new(self.stream_id, 0)),
        );

        let Implementation::NoCopy(mut nocopy) = mode else {
            self.mode = mode;
            return;
        };

        let mut hash = HashDataIn::new(self.stream_id, nocopy.byteage());

        while let Some(frame) = nocopy.pop_front_frame() {
            let ins = if frame.has_payload() {
                hash.insert_data_frame(&frame.data_frame, frame.data(), self.read_offset)
            } else {
                hash.insert_len_data_frame(&frame.data_frame, self.read_offset)
            };
            assert_ne!(ins, InsFrame::Err, "nocopy -> hash migration must preserve valid state");
        }

        self.mode = Implementation::Hash(hash);
    }

    fn switch_to_nocopy(&mut self) {
        // Rust equivalent of the hash -> nocopy switch once the hash-backed
        // representation drains empty.
        let mode = mem::replace(
            &mut self.mode,
            Implementation::NoCopy(NoCopyDataIn::new(self.stream_id)),
        );

        let Implementation::Hash(hash) = mode else {
            self.mode = mode;
            return;
        };

        assert!(hash.empty(), "hash -> nocopy switch is only valid when the hash is empty");
        self.mode = Implementation::NoCopy(NoCopyDataIn::new(self.stream_id));
    }
}

fn shared_zero_packet() -> &'static Arc<[u8]> {
    static ZERO_PACKET: OnceLock<Arc<[u8]>> = OnceLock::new();
    ZERO_PACKET.get_or_init(|| Arc::from(vec![0u8; u16::MAX as usize].into_boxed_slice()))
}

#[derive(Debug)]
pub struct LsquicNoCopy {
    engine: ReassemblyEngine,
}

impl LsquicNoCopy {
    pub fn new() -> Self {
        Self {
            engine: ReassemblyEngine::new_nocopy(0),
        }
    }

    #[inline]
    pub fn insert_zero_frame(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_zero_frame(offset, len, fin)
    }

    #[inline]
    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_len(offset, len, fin)
    }

    #[inline]
    pub fn ready(&self) -> bool {
        self.engine.ready()
    }

    #[inline]
    pub fn drain_current(&mut self) -> bool {
        self.engine.drain_current()
    }

    #[inline]
    pub fn readable_bytes(&mut self) -> u64 {
        self.engine.readable_bytes()
    }

    #[inline]
    pub fn inner(&self) -> &ReassemblyEngine {
        &self.engine
    }
}

impl Default for LsquicNoCopy {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct LsquicHash {
    engine: ReassemblyEngine,
}

impl LsquicHash {
    pub fn new() -> Self {
        Self {
            engine: ReassemblyEngine::new_hash(0),
        }
    }

    #[inline]
    pub fn insert_zero_frame(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_zero_frame(offset, len, fin)
    }

    #[inline]
    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_len(offset, len, fin)
    }

    #[inline]
    pub fn ready(&self) -> bool {
        self.engine.ready()
    }

    #[inline]
    pub fn drain_current(&mut self) -> bool {
        self.engine.drain_current()
    }

    #[inline]
    pub fn readable_bytes(&mut self) -> u64 {
        self.engine.readable_bytes()
    }

    #[inline]
    pub fn inner(&self) -> &ReassemblyEngine {
        &self.engine
    }
}

impl Default for LsquicHash {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct LsquicAuto {
    engine: ReassemblyEngine,
}

impl LsquicAuto {
    pub fn new() -> Self {
        Self {
            engine: ReassemblyEngine::new_auto(0),
        }
    }

    #[inline]
    pub fn insert_zero_frame(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_zero_frame(offset, len, fin)
    }

    #[inline]
    pub fn insert_len(&mut self, offset: u64, len: usize, fin: bool) -> InsFrame {
        self.engine.insert_len(offset, len, fin)
    }

    #[inline]
    pub fn ready(&self) -> bool {
        self.engine.ready()
    }

    #[inline]
    pub fn drain_current(&mut self) -> bool {
        self.engine.drain_current()
    }

    #[inline]
    pub fn readable_bytes(&mut self) -> u64 {
        self.engine.readable_bytes()
    }

    #[inline]
    pub fn mode_kind(&self) -> ModeKind {
        self.engine.mode_kind()
    }

    #[inline]
    pub fn inner(&self) -> &ReassemblyEngine {
        &self.engine
    }
}

impl Default for LsquicAuto {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nocopy_rejects_overlap() {
        let mut rb = LsquicNoCopy::new();

        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert_eq!(rb.insert_zero_frame(5, 10, false), InsFrame::Overlap);
    }

    #[test]
    fn nocopy_detects_duplicate() {
        let mut rb = LsquicNoCopy::new();

        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Dup);
    }

    #[test]
    fn hash_merges_overlap_into_contiguous_readable_range() {
        let mut rb = LsquicHash::new();

        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert_eq!(rb.insert_zero_frame(5, 10, false), InsFrame::Ok);
        assert!(rb.ready());
        assert_eq!(rb.readable_bytes(), 15);
        assert!(!rb.drain_current());
        assert!(!rb.ready());
    }

    #[test]
    fn auto_switches_to_hash_on_overlap() {
        let mut rb = LsquicAuto::new();

        assert_eq!(rb.mode_kind(), ModeKind::NoCopy);
        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert_eq!(rb.insert_zero_frame(5, 10, false), InsFrame::Ok);
        assert_eq!(rb.mode_kind(), ModeKind::Hash);
        assert_eq!(rb.readable_bytes(), 15);
    }

    #[test]
    fn auto_switches_back_to_nocopy_when_hash_drains_empty() {
        let mut rb = LsquicAuto::new();

        assert_eq!(rb.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert_eq!(rb.insert_zero_frame(5, 10, false), InsFrame::Ok);
        assert_eq!(rb.mode_kind(), ModeKind::Hash);
        assert!(!rb.drain_current());
        assert_eq!(rb.mode_kind(), ModeKind::NoCopy);
    }

    #[test]
    fn hash_fin_conflict_is_error() {
        let mut engine = ReassemblyEngine::new_hash(0);

        assert_eq!(engine.insert_zero_frame(0, 10, true), InsFrame::Ok);
        assert_eq!(engine.insert_zero_frame(0, 12, true), InsFrame::Err);
    }

    #[test]
    fn nocopy_accepts_zero_length_fin_at_read_offset() {
        let mut engine = ReassemblyEngine::new_nocopy(0);

        assert_eq!(engine.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert!(!engine.drain_current());
        assert_eq!(engine.insert_zero_frame(10, 0, true), InsFrame::Ok);

        let frame = engine.current_frame().expect("zero-length FIN should be readable");
        assert_eq!(frame.readable_len(), 0);
        assert!(frame.fin);
    }

    #[test]
    fn nocopy_treats_retroactive_data_fin_as_overlap() {
        let mut engine = ReassemblyEngine::new_nocopy(0);

        assert_eq!(engine.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert!(!engine.drain_current());
        assert_eq!(engine.insert_zero_frame(0, 10, true), InsFrame::Overlap);
    }

    #[test]
    fn hash_accepts_fully_consumed_duplicate_and_keeps_empty_block() {
        let mut engine = ReassemblyEngine::new_hash(0);

        assert_eq!(engine.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert!(!engine.drain_current());
        assert_eq!(engine.read_offset(), 10);

        assert_eq!(engine.insert_zero_frame(0, 10, false), InsFrame::Ok);
        assert!(!engine.ready());
        assert_eq!(engine.readable_bytes(), 0);

        match &engine.mode {
            Implementation::Hash(di) => assert_eq!(di.block_count(), 1),
            Implementation::NoCopy(_) => panic!("hash test must stay on the hash implementation"),
        }
    }

    #[test]
    fn hash_exposes_fin_on_later_block_as_separate_zero_length_frame() {
        let mut engine = ReassemblyEngine::new_hash(0);

        assert_eq!(
            engine.insert_zero_frame(0, DB_DATA_SIZE, false),
            InsFrame::Ok
        );
        assert_eq!(
            engine.insert_zero_frame(DB_DATA_SIZE as u64, 1, true),
            InsFrame::Ok
        );

        let frame = engine.current_frame().expect("first block should be readable");
        assert_eq!(frame.readable_len(), DB_DATA_SIZE as u64);
        assert!(!frame.fin);
        assert!(!engine.drain_current());

        let frame = engine.current_frame().expect("final byte should be readable");
        assert_eq!(frame.offset, DB_DATA_SIZE as u64);
        assert_eq!(frame.readable_len(), 1);
        assert!(!frame.fin);
        assert!(!engine.drain_current());

        let frame = engine.current_frame().expect("hash keeps FIN as a zero-length frame here");
        assert_eq!(frame.readable_len(), 0);
        assert!(frame.fin);
    }

    #[test]
    fn nocopy_unconditionally_switches_after_high_threshold() {
        let mut engine = ReassemblyEngine::new_auto(0);

        for i in 0..=EFF_CHECK_THRESH_HIGH as u64 {
            assert_eq!(
                engine.insert_zero_frame(i * EFF_TINY_FRAME_SZ, 1, false),
                InsFrame::Ok
            );
        }

        assert_eq!(engine.mode_kind(), ModeKind::Hash);
    }
}
