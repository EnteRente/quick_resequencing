use criterion::{criterion_group, criterion_main, Criterion, Throughput, BenchmarkId, PlottingBackend};
use std::hint::black_box;

use quick_resequencing::RecvBuffer;
use quick_resequencing::generator::{GeneratorRegistry, BenchmarkData, GeneratorConfig};

use quiche::RecvBuf;
use quicgo_sorter::QuicGoFrameSorter;
use linux_tcp_sorter::TcpOfoQueue;



// Frame sizes for "realistic" benchmarks
// 64B, 256B, 1200B, 1460B and 4096B -> represent fragmentation
const PACKET_SIZE: usize = 1200; // Assumption
const NUM_OF_PACKETS: [usize; 5] = [100, 1_000, 10_000, 100_000, 1_000_000];

// For repeatable benchmarks -> not in use yet
const SEED: u64 = 1;



/// Macro to reduce boilerplate when benchmarking multiple implementations
macro_rules! bench_implementations {
    ($group:expr, $bench_id:expr, $benchmark_data:expr, $( $impl:ty ),+ $(,)?) => {
        $(
            $group.bench_function(
                BenchmarkId::new(<$impl as RecvBuffer>::name(), &$bench_id),
                |b| {
                    b.iter(
                        || bench_recv_buf(&mut <$impl as RecvBuffer>::new(), $benchmark_data)
                    )
                },
            );
        )+
    };
}

// Add Implementations to be benched in here
fn bench_config(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    benchmark_data: &BenchmarkData,
    generator_name: &str,
    num_of_packets: usize) {

    let bench_id = format!("{}/{}", generator_name, num_of_packets);

    bench_implementations!(
        group,
        bench_id,
        benchmark_data,
        RecvBuf,
        QuicGoFrameSorter,
        TcpOfoQueue,
    );
}


/// Benchmark the data structure as specified in _bench_data_ field
fn bench_recv_buf<T: RecvBuffer>(recv_buf: &mut T, bench_data: &BenchmarkData) {

    // Insert data
    for packet in bench_data.get_packets() {
        let payload = packet.get_payload(); // cloning?
        let offset = packet.get_offset();

        // Try inserting data into the buffer
        let _ = recv_buf.insert(offset, payload);

        // Drain all drainable data
        while recv_buf.is_ready() {
            if recv_buf.drain() { // Returns error
                break;
            }
        }
    }

    black_box(recv_buf);
}


/// For each implementation run each order type -> repeat for every packet count
fn bench_main(c: &mut Criterion) {
    // Initial setup of the benchmark
    let mut group = c.benchmark_group("Comparison of Resequencing Data Structures");
    let generator_register = GeneratorRegistry::default();

    for packet_count in NUM_OF_PACKETS {
        // Set the input size for the benchmark
        let config = GeneratorConfig::new(PACKET_SIZE, packet_count, Some(SEED));
        group.throughput(Throughput::Bytes(config.total_bytes()));

        for generator in generator_register.iter() {
            // Generate Data
            let benchmark_data = generator.generate_packets(&config);

            bench_config(
                &mut group,
                &benchmark_data,
                generator.name(),
                packet_count,
            );
        }
    }

    group.finish();
}

criterion_group!(benches, bench_main);
criterion_main!(benches);