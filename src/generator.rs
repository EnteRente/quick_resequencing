use rand::Rng;
use rand_distr::{Uniform, Normal, Distribution};
const DEVIATION: f64 = 2.0;

#[derive(Debug, Clone)]
pub struct Packet {
    pub offset: u64,
    pub length: usize,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(offset: u64, length: usize, payload: Vec<u8>) -> Self { Self { offset, length, payload } }

    pub fn end_offset(&self) -> u64 { self.offset + self.length as u64 }
    pub fn get_offset(&self) -> u64 { self.offset }
    pub fn get_payload(&self) -> &[u8] { &self.payload }
}


/// Holding structure for the test data
#[derive(Debug, Clone)]
pub struct BenchmarkData {
    packets: Vec<Packet>,
}

impl BenchmarkData {

    pub fn from_order(packet_order: Vec<u64>, packet_size: usize) -> Self {
        let packets = packet_order
            .into_iter()
            .map(|i| {
                let offset = i * packet_size as u64;
                let payload = vec![0; packet_size];

                Packet::new(offset, packet_size, payload)
            })
            .collect();

        Self { packets }
    }

    /// Test data from explicit packets (for more complex scenarios like overlapping)
    pub fn from_packet(packets: Vec<Packet>, length: usize) -> Self {
        Self { packets }
    }
    pub fn get_packets(&self) -> &[Packet] { &self.packets }
}


/// Represents the configuration of the basic parameters in the benchmark
#[derive(Debug, Clone)]
pub struct GeneratorConfig {
    packet_size: usize,
    packet_count: usize,
    seed: Option<u64>,
}

impl GeneratorConfig {
    pub fn new(packet_size: usize, packet_count: usize, seed: Option<u64>) -> Self {
        Self {
            packet_size,
            packet_count,
            seed,
        }
    }

    pub fn total_bytes(&self) -> u64 { (self.packet_size * self.packet_count) as u64 }
}


/// Helper trait: Generators that just create a sequence of order
pub trait OrderGenerator: Send + Sync {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64>;

    // Generator that allows more specific behavior (like overlapping offset)
    fn generate_packets(&self, config: &GeneratorConfig) -> BenchmarkData {
        let order = self.generate(config);
        BenchmarkData::from_order(order, config.packet_size)
    }
    
    fn name(&self) -> &str;
}


/// Perfectly ordered packets
#[derive(Debug, Clone)]
pub struct InOrderGenerator {}

impl InOrderGenerator {
    pub fn new() -> Self { Self {} }
}

impl OrderGenerator for InOrderGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        (0..config.packet_size as u64).collect()
    }

    fn name(&self) -> &str { "InOrder" }
}

/// Worst case: swap first and last packet
#[derive(Debug, Clone)]
pub struct EdgeSwapGenerator {
}

impl EdgeSwapGenerator {
    pub fn new() -> Self { Self {} }
}

impl OrderGenerator for EdgeSwapGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        let mut vec = Vec::with_capacity(config.packet_count);
        vec.push(config.packet_count as u64);
        let vec_rest: Vec<u64> = (1..(config.packet_count-1) as u64).collect();
        [vec, vec_rest].concat()
    }

    fn name(&self) -> &str { "EdgeSwap" }
}



/// Generate packets where "bursts" are jittered.
/// Pick large min_chunk and max_chunk sizes to stress test the data structure.
/// The argument seed shouldn't be too large
#[derive(Debug, Clone)]
pub struct BurstsGenerator {
    min_burst: usize,
    max_burst: usize,
    jitter_d: f64, //TODO
}


impl BurstsGenerator {
    pub fn new(min_burst: usize, max_burst: usize, d: f64) -> Self {
        Self {min_burst, max_burst, jitter_d: d}
    }
}

impl OrderGenerator for BurstsGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        // Check whether min_chunk and max_chunk are valid
        assert!(self.min_burst <= self.max_burst);
        assert!(self.max_burst <= config.packet_count);

        let mut rng = rand::rng(); // Use variable from config
        let normal = Normal::new(0.0, self.jitter_d).unwrap();

        // Store tuples of (jitter_score, start_sequence_number, length)
        let mut chunks: Vec<(f64, u64, usize)> = Vec::new();

        let mut current_pkt = 0;
        let mut chunk_sequence_id = 0.0;

        while current_pkt < config.packet_count {
            // 1. Pick a random size for the current burst
            let mut size = rng.random_range(self.min_burst..= self.max_burst);

            // 2. Clamp: Don't overshoot the total packet count
            if current_pkt + size > config.packet_count {
                size = config.packet_count - current_pkt;
            }

            // 3. Calculate score: The intended order (sequence id) + noise
            // We use chunk_sequence_id (0.0, 1.0, 2.0...) as the base time
            let noise = normal.sample(&mut rng);
            let score = chunk_sequence_id + noise;

            chunks.push((score, current_pkt as u64, size));

            current_pkt += size;
            chunk_sequence_id += 1.0;
        }

        // 4. Sort chunks by their "jittered arrival time"
        chunks.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // 5. Flatten back into a stream of packets
        let mut vec = Vec::with_capacity(config.packet_count);
        for (_, start, len) in chunks {
            for i in 0..len {
                vec.push(start + i as u64);
            }
        }

        vec
    }

    fn name(&self) -> &str { "Bursts" }
}

#[derive(Debug, Clone)]
pub struct JitterGenerator {
    jitter_d: f64, //TODO: use normal distribution instead of standard deviation
}

impl JitterGenerator {
    pub fn new(d: f64) -> Self { Self { jitter_d: d } }
}

impl OrderGenerator for JitterGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        let mut rng = rand::rng();

        let vec: Vec<u64> = (0.. config.packet_count as u64).collect();


        // Normal distribution
        // Higher p = more disorder/gaps
        let normal = Normal::new(0.0, self.jitter_d).unwrap();

        // Shuffle based on distribution
        // "score": original_index + random_jitter
        let mut jittered: Vec<(f64, u64)> = vec
            .iter()
            .map(|&x| {
                let noise = normal.sample(&mut rng);
                (x as f64 + noise, x)
            })
            .collect();

        jittered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        jittered.into_iter().map(|(_, x)| x).collect()

    }

    fn name(&self) -> &str { "Jitter" }
}

//TODO: Implement overlapping with `PacketOrderGenerator`
pub struct OverlappingGenerator {}

impl OverlappingGenerator {
    pub fn new() -> Self { Self {} }
}



pub struct GeneratorRegistry {
    generators: Vec<Box<dyn OrderGenerator>>,
}

impl GeneratorRegistry {
    pub fn new() -> Self {
        Self { generators: Vec::new() }
    }

    pub fn register(&mut self, generator: impl OrderGenerator + 'static) {
        self.generators.push(Box::new(generator));
    }

    pub fn get(&self, index: usize) -> Option<&dyn OrderGenerator> {
        assert!(index < self.generators.len());
        self.generators.get(index).map(|b| b.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn OrderGenerator> {
        self.generators.iter().map(|b| b.as_ref())
    }

    pub fn len(&self) -> usize { self.generators.len() }
}

impl Default for GeneratorRegistry {
    fn default() -> Self { //TODO
        let mut registry = Self::new();
        registry.register(InOrderGenerator::new());
        registry.register(EdgeSwapGenerator::new());
        registry.register(BurstsGenerator::new(5, 10, DEVIATION));
        registry.register(JitterGenerator::new(DEVIATION));

        registry
    }
}