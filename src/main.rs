use rand::distr::Uniform;
use rand_distr::Distribution;

use linux_tcp_sorter::{SkBuffNode, TcpOfoQueue};
use quiche::{range_buf, RecvBuf};
use quicgo_sorter::{bytes, QuicGoFrameSorter};
use quiche::range_buf::RangeBuf;
use quick_resequencing::generator::{InOrderGenerator, FirstToLastGenerator, GeneratorConfig, BenchmarkData, OrderGenerator, RandomLossGenerator};
use quick_resequencing::RecvBuffer;

fn main() {
    // quicgo
    println!("size of bytes::Bytes: {}", std::mem::size_of::<bytes::Bytes>());
    println!("align of bytes::Bytes: {}", std::mem::align_of::<bytes::Bytes>());

    // Quiche
    println!("size of RangeBuf: {}", std::mem::size_of::<range_buf::RangeBuf<range_buf::DefaultBufFactory>>());
    println!("align of RangeBuf: {}", std::mem::align_of::<range_buf::RangeBuf<range_buf::DefaultBufFactory>>());

    // TCP
    println!("size of SkBuffNode: {}", std::mem::size_of::<SkBuffNode>());
    println!("align of SkBuffNode: {}", std::mem::align_of::<SkBuffNode>());

    // Approve sorting properties for quiche and quicgo
    let in_order_packets = InOrderGenerator::new().generate(&GeneratorConfig::new(100, 100, None));
    let random_loss_packets = RandomLossGenerator::new(0.1).generate(&GeneratorConfig::new(100, 100, None));

    println!("Random loss packets: {:?}", random_loss_packets);

    println!("Now we sort with quiche and quicgo and compare the results:");
    let mut quiche_buffer = RecvBuf::new(u64::MAX, u64::MAX);
    let mut quicgo_buffer = QuicGoFrameSorter::new();
    let mut linux_buffer = TcpOfoQueue::new(0);

    let mut quicgo_result: Vec<u64> = Vec::default();
    let mut quiche_result: Vec<u64> = Vec::default();

    for packet in BenchmarkData::from_order(random_loss_packets, 100).get_packets() {
        let offset = packet.get_offset();
        let length = packet.get_length();


        let mut quiche_rangebuf = RangeBuf::from(&Vec::default(), offset, false);
        quiche_rangebuf.set_len(length as usize);
        quiche_buffer.write(quiche_rangebuf).unwrap();

        let mut quicgo_bytes = bytes::Bytes::from(Vec::default());
        quicgo_bytes.set_len(length as usize);
        quicgo_buffer.push(offset, quicgo_bytes).unwrap();

        linux_buffer.insert_len(offset as u32, length as u32, false);

        //TODO
        while quiche_buffer.ready() {
            let popped_rangebuf = quiche_buffer.drain();
            // fill results here
        }

        //TODO
        while quicgo_buffer.ready() {
            let popped_bytes = quicgo_buffer.pop(2048).unwrap();
            // fill results here
        }

        while linux_buffer.ready() {
            linux_buffer.drain();
        }

    }

    // Verify results here
    // println!("quicgo_result: {:?}", quicgo_result);
    // println!("quiche_result: {:?}", quiche_result);
    // println!("TCP result: {:?}", linux_buffer.rcv_nxt);

}