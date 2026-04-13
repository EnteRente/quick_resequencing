use std::ptr::NonNull;
use std::sync::Arc;

// Frame metadata used by both ports.
//
// Upstream references:
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_packet_in.h
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_nocopy.c
// - https://github.com/litespeedtech/lsquic/blob/master/src/liblsquic/lsquic_di_hash.c
//
// `DataFrameSnapshot` mirrors the relevant fields of lsquic's `struct
// data_frame`, while `StreamFrame` is the Rust-owned analogue of the
// packet-backed stream frame objects passed into the C data-in layer.
//
// For the nocopy port, `StreamFrame` also carries intrusive `prev` / `next`
// links. This mirrors lsquic's `TAILQ_ENTRY(stream_frame)` placement much more
// closely than the previous wrapper-node approach.
pub type StreamId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsFrame {
    Ok,
    Dup,
    Overlap,
    Err,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataFrameSnapshot {
    pub offset: u64,
    pub read_off: u16,
    pub size: u16,
    pub fin: bool,
}

impl DataFrameSnapshot {
    #[inline]
    pub const fn end_offset(self) -> u64 {
        self.offset + self.size as u64
    }

    #[inline]
    pub const fn read_offset(self) -> u64 {
        self.offset + self.read_off as u64
    }

    #[inline]
    pub const fn readable_len(self) -> u64 {
        self.size as u64 - self.read_off as u64
    }

    #[inline]
    pub fn mark_consumed(&mut self) {
        self.read_off = self.size;
    }
}

#[derive(Debug)]
pub struct StreamFrame {
    pub(crate) prev: Option<NonNull<StreamFrame>>,
    pub(crate) next: Option<NonNull<StreamFrame>>,
    pub packet: Option<Arc<[u8]>>,
    pub packet_offset: usize,
    pub data_frame: DataFrameSnapshot,
    pub stream_id: StreamId,
}

impl Clone for StreamFrame {
    fn clone(&self) -> Self {
        Self {
            prev: None,
            next: None,
            packet: self.packet.as_ref().map(Arc::clone),
            packet_offset: self.packet_offset,
            data_frame: self.data_frame,
            stream_id: self.stream_id,
        }
    }
}

impl StreamFrame {
    pub fn new(
        stream_id: StreamId,
        packet: Arc<[u8]>,
        packet_offset: usize,
        stream_offset: u64,
        fin: bool,
        size: usize,
    ) -> Self {
        assert!(size <= u16::MAX as usize);
        assert!(packet_offset + size <= packet.len());

        Self {
            prev: None,
            next: None,
            packet: Some(packet),
            packet_offset,
            data_frame: DataFrameSnapshot {
                offset: stream_offset,
                read_off: 0,
                size: size as u16,
                fin,
            },
            stream_id,
        }
    }

    pub fn new_len_only(stream_id: StreamId, stream_offset: u64, fin: bool, size: usize) -> Self {
        assert!(size <= u16::MAX as usize);

        Self {
            prev: None,
            next: None,
            packet: None,
            packet_offset: 0,
            data_frame: DataFrameSnapshot {
                offset: stream_offset,
                read_off: 0,
                size: size as u16,
                fin,
            },
            stream_id,
        }
    }

    pub fn from_bytes(
        stream_id: StreamId,
        stream_offset: u64,
        bytes: impl Into<Arc<[u8]>>,
        fin: bool,
    ) -> Self {
        let packet = bytes.into();
        let len = packet.len();
        Self::new(stream_id, packet, 0, stream_offset, fin, len)
    }

    #[inline]
    pub fn data(&self) -> &[u8] {
        assert!(self.has_payload(), "length-only frames do not carry packet bytes");
        let packet = self.packet.as_ref().expect("payload present, different issue");
        let start = self.packet_offset;
        let end = start + self.data_frame.size as usize;
        &packet[start..end]
    }

    #[inline]
    pub fn has_payload(&self) -> bool {
        self.packet.is_some()
    }

    #[inline]
    pub fn packet_len(&self) -> usize {
        self.packet.as_ref().map_or(0, |packet| packet.len())
    }
}
