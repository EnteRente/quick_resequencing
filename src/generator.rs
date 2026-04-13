use rand::{Rng, RngCore, SeedableRng, rngs::StdRng};
use rand_distr::{Uniform, Normal, Distribution};

const DEVIATION: f64 = 2.0;
const ERROR_PROBABILITY: f64 = 0.01; // 1% chance of packet loss

// Run all of them for only one burst case (i.e. 10_000 packets): 0%, 1%, 2%, ..., 10%
const ERROR_RATES : [f64; 11] = [0.0, 0.01, 0.02, 0.03, 0.04, 0.05, 0.06, 0.07, 0.08, 0.09, 0.1];

#[derive(Debug, Clone)]
pub struct Packet {
    pub offset: u64,
    pub length: usize,
    //pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(offset: u64, length: usize) -> Self { Self { offset, length } }

    pub fn get_length(&self) -> u64 { self.length as u64 }
    pub fn end_offset(&self) -> u64 { self.offset + self.length as u64 }
    pub fn get_offset(&self) -> u64 { self.offset }
    //pub fn get_payload(&self) -> &[u8] { &self.payload }
}


/// Holding structure for the test sequence
#[derive(Debug, Clone)]
pub struct BenchmarkData {
    packets: Vec<Packet>,
}

impl BenchmarkData {

    /// Create the benchmark sequence from order
    pub fn from_order(packet_order: Vec<u64>, packet_size: usize) -> Self {
        let packets = packet_order
            .into_iter()
            .map(|i| {
                let offset = i * packet_size as u64;
                //let payload = vec![0; packet_size];

                Packet::new(offset, packet_size)
            })
            .collect();

        Self { packets }
    }

    /// Produce packet_order from the benchmark data (reverse of from_order)
    ///
    /// Helper for confirming sorting behavior of implementations
    pub fn to_order(self) -> Vec<u64> {
        let order = self.packets
            .into_iter()
            .map(|packet| {
                let offset = packet.get_offset();
                let length = packet.get_length();
                offset / length as u64
            }).collect::<Vec<u64>>();

        order
    }


    /// Create the benchmark sequence from packets
    pub fn from_packet(packets: Vec<Packet>) -> Self {
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


/// Generators that create a sequence of numbers
pub trait OrderGenerator: Send + Sync {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64>;

    /// Generator that allows more specific behavior (like overlapping offset)
    fn generate_packets(&self, config: &GeneratorConfig) -> BenchmarkData {
        let order = self.generate(config);
        BenchmarkData::from_order(order, config.packet_size)
    }
    
    fn name(&self) -> &'static str;
}


/// Perfectly ordered packets
#[derive(Debug, Clone)]
pub struct InOrderGenerator {}

impl InOrderGenerator {
    pub fn new() -> Self { Self {} }
}

impl OrderGenerator for InOrderGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        (0..config.packet_count as u64).collect()
    }

    fn name(&self) -> &'static str { "InOrder" }
}

/// Simulates that the first packet is lost and retransmitted (appended to the end of the vector)
#[derive(Debug, Clone)]
pub struct FirstToLastGenerator {
}

impl FirstToLastGenerator {
    pub fn new() -> Self { Self {} }
}

impl OrderGenerator for FirstToLastGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        let mut vec: Vec<u64> = (1..config.packet_count as u64).collect();
        vec.push(0);

        vec
    }

    fn name(&self) -> &'static str { "FirstToLast" }
}

/// Generate packets with a given probability of being lost.
/// When a packet is "lost", it is appended at the end.
pub struct RandomLossGenerator {
    p: f64
}


impl RandomLossGenerator {
    pub fn new(p: f64) -> Self { Self { p } }
}

impl OrderGenerator for RandomLossGenerator {
    fn generate(&self, config: &GeneratorConfig) -> Vec<u64> {
        let mut vec = Vec::with_capacity(config.packet_count);
        let mut loss_vec: Vec<u64> = Vec::new();

        let mut seeded_rng;
        let mut thread_rng;
        let rng: &mut dyn RngCore = if let Some(seed) = config.seed {
            seeded_rng = StdRng::seed_from_u64(seed);
            &mut seeded_rng
        } else {
            thread_rng = rand::rng();
            &mut thread_rng
        };
        let uniform = Uniform::new_inclusive(0.0, 1.0).unwrap();

        for i in 0..config.packet_count as u64 {
            // With probability p, remove packet and add to lost packets
            if uniform.sample(rng) < self.p {
                loss_vec.push(i);
            } else {
                vec.push(i);
            }
        }

        vec.append(&mut loss_vec);
        vec
    }

    fn name(&self) -> &'static str { "RandomLoss" }
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

    fn name(&self) -> &'static str { "Bursts" }
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

    fn name(&self) -> &'static str { "Jitter" }
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
    fn default() -> Self { // Add your generators here
        let mut registry = Self::new();
        registry.register(InOrderGenerator::new());
        registry.register(FirstToLastGenerator::new());
        //registry.register(RandomLossGenerator::new(ERROR_PROBABILITY));

        registry
    }
}
