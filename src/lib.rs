use quiche::{RecvBuf, range_buf::RangeBuf};
use quicgo_sorter::QuicGoFrameSorter;
use linux_tcp_sorter::TcpOfoQueue;
use s2n_quic::s2n_quic_core::{buffer::Reassembler, varint::VarInt};
use lsquic::{InsFrame, LsquicAuto, LsquicHash, LsquicNoCopy, ReassemblyEngine};


pub mod generator;

/// Common interface for all frame sorter implementations
/// with common naming
pub trait RecvBuffer {

    fn new() -> Self;

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()>;
    fn is_ready(&self) -> bool;
    fn drain(&mut self) -> bool;

    fn name() -> &'static str;
}

fn map_lsquic_insert(result: InsFrame) -> Result<(), ()> {
    match result {
        InsFrame::Ok | InsFrame::Dup => Ok(()),
        InsFrame::Overlap | InsFrame::Err => Err(()),
    }
}

impl RecvBuffer for RecvBuf {

    fn new() -> Self {
        RecvBuf::new(u64::MAX, u64::MAX)
    }
    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        let range_buf = RangeBuf::from_len(offset, len as usize, false);
        self.write(range_buf).map_err(|_| ())
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.emit_void().is_err()
    }

    fn name() -> &'static str { "quiche" }
}

impl RecvBuffer for QuicGoFrameSorter {

    fn new() -> Self {
        QuicGoFrameSorter::new()
    }
    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        let bytes = quicgo_sorter::bytes::Bytes::from_len(len as usize);
        self.push(offset, bytes).map_err(|_| ())
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.pop(2048).is_err() //TODO: fix input
    }

    fn name() -> &'static str { "quic-go" }
}


impl RecvBuffer for TcpOfoQueue {

    fn new() -> Self {
        TcpOfoQueue::new(0)
    }
    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        // offset will be u32 in the input
        let res = self.insert_len(offset as u32, len as u32, false);
        if res { Ok(()) } else { Err(()) }
    }
    fn is_ready(&self) -> bool {
        self.ready()
    }
    fn drain(&mut self) -> bool {
        TcpOfoQueue::drain(self)
    }

    fn name() -> &'static str { "linux_tcp" }
}

impl RecvBuffer for Reassembler {
    fn new() -> Self { Reassembler::new() }

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        let offset = VarInt::new(offset).map_err(|_| ())?;
        self.write_at_len(offset, len as usize).map_err(|_| ())
    }

    fn is_ready(&self) -> bool {
        !self.is_empty()
    }

    fn drain(&mut self) -> bool {
        self.pop_len().is_none()
    }

    fn name() -> &'static str {
        "s2n-quic"
    }
}

impl RecvBuffer for ReassemblyEngine {
    fn new() -> Self {
        ReassemblyEngine::new_auto(0)
    }

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        map_lsquic_insert(self.insert_len(offset, len as usize, false))
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.drain_current()
    }

    fn name() -> &'static str {
        "lsquic"
    }
}

impl RecvBuffer for LsquicNoCopy {
    fn new() -> Self {
        LsquicNoCopy::new()
    }

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        map_lsquic_insert(self.insert_len(offset, len as usize, false))
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.drain_current()
    }

    fn name() -> &'static str {
        "lsquic-nocopy"
    }
}

impl RecvBuffer for LsquicHash {
    fn new() -> Self {
        LsquicHash::new()
    }

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        map_lsquic_insert(self.insert_len(offset, len as usize, false))
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.drain_current()
    }

    fn name() -> &'static str {
        "lsquic-hash"
    }
}

impl RecvBuffer for LsquicAuto {
    fn new() -> Self {
        LsquicAuto::new()
    }

    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        map_lsquic_insert(self.insert_len(offset, len as usize, false))
    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        self.drain_current()
    }

    fn name() -> &'static str {
        "lsquic-auto"
    }
}

#[cfg(test)]
mod tests {
    use super::RecvBuffer;
    use lsquic::{LsquicAuto, LsquicHash, LsquicNoCopy, ReassemblyEngine};
    use s2n_quic::buffer::Reassembler;

    fn adapter_handles_basic_out_of_order_flow<T: RecvBuffer>() {
        let mut recv_buf = T::new();

        assert!(recv_buf.insert(1200, 1200).is_ok());
        assert!(!recv_buf.is_ready());

        assert!(recv_buf.insert(0, 1200).is_ok());
        assert!(recv_buf.is_ready());
        let mut drained = 0usize;
        while recv_buf.is_ready() {
            assert!(!recv_buf.drain());
            drained += 1;
        }
        assert!(drained >= 1);
        assert!(!recv_buf.is_ready());
    }

    #[test]
    fn lsquic_nocopy_adapter_handles_basic_out_of_order_flow() {
        adapter_handles_basic_out_of_order_flow::<LsquicNoCopy>();
    }

    #[test]
    fn lsquic_hash_adapter_handles_basic_out_of_order_flow() {
        adapter_handles_basic_out_of_order_flow::<LsquicHash>();
    }

    #[test]
    fn lsquic_auto_adapter_handles_basic_out_of_order_flow() {
        adapter_handles_basic_out_of_order_flow::<LsquicAuto>();
    }

    #[test]
    fn lsquic_engine_adapter_handles_basic_out_of_order_flow() {
        adapter_handles_basic_out_of_order_flow::<ReassemblyEngine>();
    }

    #[test]
    fn s2n_adapter_handles_basic_out_of_order_flow() {
        let mut recv_buf = Reassembler::new();

        assert!(recv_buf.insert(1200, 1200).is_ok());
        assert!(!recv_buf.is_ready());

        assert!(recv_buf.insert(0, 1200).is_ok());
        assert!(recv_buf.is_ready());

        while recv_buf.is_ready() {
            assert!(!RecvBuffer::drain(&mut recv_buf));
        }

        assert!(!recv_buf.is_ready());
    }

}
