use std::env;
use std::hint::black_box;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion, Throughput};

use quick_resequencing::generator::{
    BenchmarkData, GeneratorConfig, GeneratorRegistry, OrderGenerator, RandomLossGenerator,
};
use quick_resequencing::RecvBuffer;

use linux_tcp_sorter::TcpOfoQueue;
use lsquic::ReassemblyEngine;
use quicgo_sorter::QuicGoFrameSorter;
use quiche::RecvBuf;
use s2n_quic::buffer::Reassembler;

// Assumption for both TCP and QUIC
// The QUIC specification recommends a conservative default minimum QUIC packet size of 1200 bytes
// (the larger the packet size, the better the performance)
// https://www.fastly.com/blog/measuring-quic-vs-tcp-computational-efficiency
// This is only relevant when working with actual data, which is not the case here yet.
// !Relevant for s2n-quic, because allocation growth depends on the offset:!
// https://github.com/aws/s2n-quic/blob/789ceffa0cdf20d861de454e5561e5abbf802f89/quic/s2n-quic-core/src/buffer/reassembler.rs#L616C9-L616C65
const PACKET_SIZE: usize = 1200;
const NUM_OF_PACKETS: [usize; 6] = [100, 1_000, 10_000, 30_000, 60_000, 100_000];

const RANDOM_LOSS_PACKET_COUNT: usize = 10_000;

// For reproducibility, use a fixed seed
const SEED: u64 = 42;

const ERROR_RATES: [f64; 11] = [
    0.0, 0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.07, 0.08, 0.09, 0.1,
];
const BENCH_SCHEMES_ENV: &str = "QUICK_RESEQUENCING_SCHEMES";
const BENCH_IMPLEMENTATIONS_ENV: &str = "QUICK_RESEQUENCING_IMPLEMENTATIONS";
const BENCH_BURST_SIZES_ENV: &str = "QUICK_RESEQUENCING_BURST_SIZES";
const BENCH_PACKET_COUNTS_ENV: &str = "QUICK_RESEQUENCING_PACKET_COUNTS";

//TODO: Refactor the selection phase into another module
struct SchemeSelection {
    in_order: bool,
    first_to_last: bool,
    shuffle: bool,
    random_loss: bool,
    random_loss_rate: bool,
}

impl SchemeSelection {
    fn all() -> Self {
        // Default
        Self {
            in_order: true,
            first_to_last: true,
            shuffle: true,
            random_loss: true,
            random_loss_rate: true,
        }
    }

    fn from_env() -> Self {
        let Ok(value) = env::var(BENCH_SCHEMES_ENV) else {
            return Self::all();
        };
        if value.trim().is_empty() {
            return Self::all();
        }

        let mut selection = Self {
            in_order: false,
            first_to_last: false,
            shuffle: false,
            random_loss: false,
            random_loss_rate: false,
        };

        for scheme in value
            .split(',')
            .map(str::trim)
            .filter(|scheme| !scheme.is_empty())
        {
            match scheme {
                "InOrder" => selection.in_order = true,
                "FirstToLast" => selection.first_to_last = true,
                "Shuffle" => selection.shuffle = true,
                "RandomLoss" => selection.random_loss = true,
                "RandomLossRate" => selection.random_loss_rate = true,
                other => panic!("Unknown benchmark scheme in {BENCH_SCHEMES_ENV}: {other}"),
            }
        }

        selection
    }

    fn includes(&self, scheme: &str) -> bool {
        match scheme {
            "InOrder" => self.in_order,
            "FirstToLast" => self.first_to_last,
            "Shuffle" => self.shuffle,
            "RandomLoss" => self.random_loss,
            "RandomLossRate" => self.random_loss_rate,
            _ => false,
        }
    }
}

struct ImplementationSelection {
    quiche: bool,
    quic_go: bool,
    linux_tcp: bool,
    s2n_quic: bool,
    lsquic: bool,
}

impl ImplementationSelection {
    fn all() -> Self {
        Self {
            quiche: true,
            quic_go: true,
            linux_tcp: true,
            s2n_quic: true,
            lsquic: true,
        }
    }

    fn from_env() -> Self {
        let Ok(value) = env::var(BENCH_IMPLEMENTATIONS_ENV) else {
            return Self::all();
        };
        if value.trim().is_empty() {
            return Self::all();
        }

        let mut selection = Self {
            quiche: false,
            quic_go: false,
            linux_tcp: false,
            s2n_quic: false,
            lsquic: false,
        };

        for implementation in value
            .split(',')
            .map(str::trim)
            .filter(|implementation| !implementation.is_empty())
        {
            match implementation {
                "all" => return Self::all(),
                "quiche" => selection.quiche = true,
                "quic-go"   | "quicgo"    | "quic_go" => selection.quic_go = true,
                "linux_tcp" | "linux-tcp" | "tcp" => selection.linux_tcp = true,
                "s2n-quic"  | "s2n_quic"  | "s2n" => selection.s2n_quic = true,
                "lsquic" => selection.lsquic = true,
                other => panic!(
                    "Unknown benchmark implementation in {BENCH_IMPLEMENTATIONS_ENV}: {other}"
                ),
            }
        }

        selection
    }

    fn includes(&self, implementation: &str) -> bool {
        match implementation {
            "quiche"    => self.quiche,
            "quic-go"   => self.quic_go,
            "linux_tcp" => self.linux_tcp,
            "s2n-quic"  => self.s2n_quic,
            "lsquic"    => self.lsquic,
            _           => false,
        }
    }
}

fn selected_packet_counts_from_env() -> Vec<usize> {
    let Ok(value) = env::var(BENCH_BURST_SIZES_ENV).or_else(|_| env::var(BENCH_PACKET_COUNTS_ENV))
    else {
        return NUM_OF_PACKETS.to_vec();
    };
    if value.trim().is_empty() {
        return NUM_OF_PACKETS.to_vec();
    }

    let mut packet_counts = Vec::new();
    for raw_packet_count in value
        .split(',')
        .map(str::trim)
        .filter(|packet_count| !packet_count.is_empty())
    {
        if raw_packet_count.eq_ignore_ascii_case("all") {
            return NUM_OF_PACKETS.to_vec();
        }

        let packet_count = raw_packet_count.parse::<usize>().unwrap_or_else(|_| {
            panic!(
                "Invalid packet count in {BENCH_BURST_SIZES_ENV}/{BENCH_PACKET_COUNTS_ENV}: {raw_packet_count}"
            )
        });
        if packet_count == 0 {
            panic!("Packet count in {BENCH_BURST_SIZES_ENV}/{BENCH_PACKET_COUNTS_ENV} must be greater than zero");
        }
        if !packet_counts.contains(&packet_count) {
            packet_counts.push(packet_count);
        }
    }

    if packet_counts.is_empty() {
        NUM_OF_PACKETS.to_vec()
    } else {
        packet_counts
    }
}

/// Macro to reduce boilerplate when benchmarking multiple implementations
macro_rules! bench_implementations {
    ($group:expr, $bench_id:expr, $benchmark_data:expr, $selected_implementations:expr, $( $impl:ty ),+ $(,)?) => {
        $(
            if $selected_implementations.includes(<$impl as RecvBuffer>::name()) {
                $group.bench_function(
                    BenchmarkId::new(<$impl as RecvBuffer>::name(), &$bench_id),
                    |b| {
                        b.iter_batched(
                            || <$impl as RecvBuffer>::new(),
                            |mut recv_buf| {
                                bench_recv_buf(&mut recv_buf, black_box($benchmark_data));
                                black_box(recv_buf);
                            },
                            BatchSize::LargeInput,

                        )
                    },
                );
            }
        )+
    };
}

fn bench_config(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    benchmark_data: &BenchmarkData,
    selected_implementations: &ImplementationSelection,
    generator_name: &str,
    num_of_packets: usize,
) {
    //TODO: fix bench_id formatting
    let bench_id = format!("{}/{}", generator_name, num_of_packets);

    bench_implementations!(
        group,
        bench_id,
        benchmark_data,
        selected_implementations,
        RecvBuf,          //cloudflare quiche
        QuicGoFrameSorter,
        TcpOfoQueue,
        Reassembler,      //s2n-quic
        ReassemblyEngine, //lsquic
    );
}

/// Benchmark the receive buffer under test.
/// Insert and drain in a loop
fn bench_recv_buf<T: RecvBuffer>(recv_buf: &mut T, bench_data: &BenchmarkData) {
    for packet in bench_data.get_packets() {
        let length = packet.get_length();
        let offset = packet.get_offset();

        // Try inserting data into the buffer
        let _ = recv_buf.insert(offset, length);
    }

    // Drain all drainable data after all data has been inserted
    while recv_buf.is_ready() {
        if recv_buf.drain() {
            break;
        }
    }
}

/// For each implementation run each order type (configured by the generator).
///
/// Repeat this for each packet configuration (i.e. 100, 1_000, 100_000).
fn bench_main(c: &mut Criterion) {
    // Initial setup of the benchmark
    let mut group = c.benchmark_group("Comparison of Resequencing Data Structures");
    let generator_register = GeneratorRegistry::default();
    let selected_schemes = SchemeSelection::from_env();
    let selected_implementations = ImplementationSelection::from_env();
    let selected_packet_counts = selected_packet_counts_from_env();

    for packet_count in selected_packet_counts.iter().copied() {
        // Configure settings
        let config = GeneratorConfig::new(PACKET_SIZE, packet_count, Some(SEED));
        group.throughput(Throughput::Elements(packet_count as u64));
        group.measurement_time(Duration::from_secs_f64(100.0));

        for generator in generator_register.iter() {
            if !selected_schemes.includes(generator.name()) {
                continue;
            }

            let benchmark_data = generator.generate_packets(&config);

            bench_config(
                &mut group,
                &benchmark_data,
                &selected_implementations,
                generator.name(),
                packet_count,
            );
        }
    }

    // TODO: "Hard coded" (i.e. not integrated into incorporated into the code structure as intended)
    // Random Loss 0% up to 10% error rate with 10_000 packets
    if selected_schemes.includes("RandomLossRate")
        && selected_packet_counts.contains(&RANDOM_LOSS_PACKET_COUNT)
    {
        group.throughput(Throughput::Elements(RANDOM_LOSS_PACKET_COUNT as u64));
        group.measurement_time(Duration::from_secs_f64(50.0));

        for error_rate in ERROR_RATES {
            let config = GeneratorConfig::new(PACKET_SIZE, RANDOM_LOSS_PACKET_COUNT, Some(SEED));
            let generator = RandomLossGenerator::new(error_rate);
            let benchmark_data = generator.generate_packets(&config);

            let loss_label = format!("RandomLoss with {:.0}% loss", error_rate * 100.0);
            bench_config(
                &mut group,
                &benchmark_data,
                &selected_implementations,
                &loss_label,
                RANDOM_LOSS_PACKET_COUNT,
            )
        }
    }

    group.finish();
}

criterion_group!(benches, bench_main);
criterion_main!(benches);
