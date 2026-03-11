use quiche::{RecvBuf, range_buf::RangeBuf};
use quicgo_sorter::QuicGoFrameSorter;
use linux_tcp_sorter::TcpOfoQueue;

pub mod generator;

/// Common interface for all frame sorter implementations
/// with common naming
pub trait RecvBuffer {

    fn new() -> Self;

    //&[u8] -> affects the benchmark, in every instance, to_vec() is used
    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()>;
    fn is_ready(&self) -> bool;
    fn drain(&mut self) -> bool;

    fn name() -> &'static str;
}

impl RecvBuffer for RecvBuf {

    fn new() -> Self {
        RecvBuf::new(u64::MAX, u64::MAX)
    }
    fn insert(&mut self, offset: u64, len: u64) -> Result<(), ()> {
        let mut range_buf = RangeBuf::from(&Vec::default(), offset, false);
        range_buf.set_len(len as usize);
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
        let mut bytes = quicgo_sorter::bytes::Bytes::from(Vec::default());
        bytes.set_len(len as usize);
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
