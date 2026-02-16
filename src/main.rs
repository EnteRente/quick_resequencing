use rand::{Rng, SeedableRng};
use rand_distr::{Normal, Distribution};



fn generate_incoming_packet_jitter_chunks(
    packet_count: usize,
    min_chunk: usize,
    max_chunk: usize,
    d: f64)
    -> Vec<u64> {

    // Check whether min_chunk and max_chunk are valid
    assert!(min_chunk <= max_chunk);
    assert!(max_chunk <= packet_count);


    let mut rng = rand::rng();
    let normal = Normal::new(0.0, d).unwrap();

    // Store tuples of (jitter_score, start_sequence_number, length)
    let mut chunks: Vec<(f64, u64, usize)> = Vec::new();

    let mut current_pkt = 0;
    let mut chunk_sequence_id = 0.0;

    while current_pkt < packet_count {
        // 1. Pick a random size for the current burst
        let mut size = rng.random_range(min_chunk..=max_chunk);

        // 2. Clamp: Don't overshoot the total packet count
        if current_pkt + size > packet_count {
            size = packet_count - current_pkt;
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
    let mut vec = Vec::with_capacity(packet_count);
    for (_, start, len) in chunks {
        for i in 0..len {
            vec.push(start + i as u64);
        }
    }

    vec
}

fn main() {
    let result = generate_incoming_packet_jitter_chunks(1000, 5, 10, 0.6);
    println!("{:?}", result);

}