use quiche::{RecvBuf, range_buf::RangeBuf};
use quicgo_sorter::QuicGoFrameSorter;
use linux_tcp_sorter::TcpOfoQueue;

pub mod generator;

/// Common interface for all frame sorter implementations
/// with common naming
pub trait RecvBuffer {

    fn new() -> Self;

    //&[u8] -> affects the benchmark, in every instance, to_vec() is used
    fn insert(&mut self, offset: u64, data: &[u8]) -> Result<(), ()>;
    fn is_ready(&self) -> bool;
    fn drain(&mut self) -> bool;

    fn name() -> &'static str;
}

impl RecvBuffer for RecvBuf {

    fn new() -> Self {
        RecvBuf::new(u64::MAX, u64::MAX)
    }
    fn insert(&mut self, offset: u64, data: &[u8]) -> Result<(), ()> {
        let range_buf = RangeBuf::from(&data.to_vec(), offset, false);
        self.write(range_buf).map_err(|_| ())

    }

    fn is_ready(&self) -> bool {
        self.ready()
    }

    fn drain(&mut self) -> bool {
        !self.emit_void().is_err()
    }

    fn name() -> &'static str { "quiche" }
}

impl RecvBuffer for QuicGoFrameSorter {

    fn new() -> Self {
        QuicGoFrameSorter::new()
    }
    fn insert(&mut self, offset: u64, data: &[u8]) -> Result<(), ()> {
        self.push(offset, quicgo_sorter::bytes::Bytes::from(data.to_vec())).map_err(|_| ())
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
    fn insert(&mut self, offset: u64, data: &[u8]) -> Result<(), ()> {
        // offset will be u32 in the input
        let res = self.insert(offset as u32, data.to_vec(), false);
        if res { Ok(()) } else { Err(()) }
    }
    fn is_ready(&self) -> bool {
        self.ready()
    }
    fn drain(&mut self) -> bool { true }

    fn name() -> &'static str { "linux_tcp" }
}