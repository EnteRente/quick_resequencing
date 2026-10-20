# quick_resequencing

Benchmarks receive-side resequencing data structures \& algorithms adapted from
[QUIC](https://www.rfc-editor.org/rfc/rfc9000.html) implementations and the
[Linux TCP receive path](https://github.com/torvalds/linux/blob/master/net/ipv4/tcp_input.c).

The project compares how different ordering scenarios and batch sizes affect performance,
using a common Rust interface and [Criterion.rs](https://bheisler.github.io/criterion.rs/book/).


## Methodology

This benchmark is excluding payload data and is **metadata-only** (i.e. the chosen implementations are adapted accordingly).
The transition back to payload data should be straightforward.

Each iteration starts with a fresh receive buffer, inserts an entire synthetic
packet batch (i.e. burst), then drains the available data. Input generation and buffer
construction happen outside the timed criterion routine; insertion, draining, and buffer
destruction happen inside it. Inputs contain byte offsets and lengths, with
payload handling replaced by length-based operations. The measurements describe
the adapted data structures in isolation from network I/O and encryption.

The **box plots** show time spent per packet for each burst.

| Parameter | Default                                                                 |
| --- |-------------------------------------------------------------------------|
| Packets per batch | 100; 1,000; 10,000; 30,000; 60,000; 100,000                             |
| Bytes represented per packet | 1,200                                                                   |
| Random seed | 42; the generated sequence is reused across iterations and implementations |
| Criterion measurement time | 100 s per case; 50 s per loss-rate case, plus warm-up and analysis      |

### Packet ordering scenarios supported

| CLI selector | Arrival order                                                                                             |
| --- |-----------------------------------------------------------------------------------------------------------|
| `InOrder` | Ideal and most common case; Increasing byte offsets.                                                      |
| `FirstToLast` | The first packet arrives after all other packets. (excluded for now)                                      |
| `Shuffle` | Adversarial case; A uniformly shuffled permutation of the batch.                                          |
| `RandomLoss` | Each packet is deferred with probability 1%; deferred packets arrive at the end, in their original order. |
| `RandomLossRate` | The same deferral model, sweeping probabilities from 0% to 10% in 1% steps with 10,000 packets.           |


### Supported Implementations

| Upstream project | Reference release | CLI selector |
| --- | --- | --- |
| [Linux TCP](https://github.com/torvalds/linux/blob/master/net/ipv4/tcp_input.c) | kernel v7.0 | `linux_tcp` |
| [Cloudflare quiche](https://github.com/cloudflare/quiche) | v0.29.0 | `quiche` |
| [quic-go](https://github.com/quic-go/quic-go) | v0.59.0 | `quic-go` |
| [AWS s2n-quic](https://github.com/aws/s2n-quic) | v1.77.0 | `s2n-quic` |
| [LiteSpeed LSQUIC](https://github.com/litespeedtech/lsquic) | v4.7.0 | `lsquic` |

Experiments use the local Rust adaptations; All implementations run by default. **LSQUIC is currently excluded from timing plots**.


## Reproduce results

Use [Nix](https://nixos.org/) with flakes enabled. The development shell provides
the necessary dependencies to run the benchmarking pipeline.

```bash
git clone https://github.com/EnteRente/quick_resequencing.git
cd quick_resequencing
nix develop

# Run the full benchmark matrix and generate plots.
python3 -m benchmark_tools
```

The full matrix takes a long time with the default configuration. For a
smaller run, select specific implementations, scenarios, and batch sizes:

```bash
python3 -m benchmark_tools \
  --implementation quiche --scheme RandomLoss --burst-size 10000
```

`--implementation`, `--scheme`, and `--burst-size` accept repeated options or
comma-separated values. Outside the dev shell, the pipeline enters Nix
automatically when launched with `python3`.


```bash
# Rebuild plots from existing Criterion output.
python3 -m benchmark_tools --skip-bench

# Show all options, including Linux perf profiling.
python3 -m benchmark_tools --help
```

### Results

| Location | Contents                                       |
| --- |------------------------------------------------|
| `target/criterion` | Raw Criterion samples and reports              |
| `target/criterion_packet_boxplots` | Generated boxplots as PDFs                     |
| `benchmark_results/plots/latest` | Latest published plots and `run_metadata.json` |
| `benchmark_results/plots/runs/<YYYY-MM-DD_HHMMSS>` | Archived timing plots and metadata             |


### Example plot output for the InOrder scenario
[in-order scenario (PDF)](example_figures/scenario_in_order.pdf).
See [benchmarking details](docs/benchmarking.md) for profiling and interpretation.


## Continue development

| Task | Starting point |
| --- | --- |
| Adapt an implementation | [`RecvBuffer` and adapters](src/lib.rs) |
| Add an ordering scenario | [`OrderGenerator` and `GeneratorRegistry`](src/generator.rs) |
| Change batch sizes, or timing | [Criterion harness](benches/resequencing_bench.rs) |
| Update CLI selections and defaults | [`benchmark_tools/config.py`](benchmark_tools/config.py) |
| Change parsing or figures | [`benchmark_tools/criterion.py`](benchmark_tools/criterion.py) and [`benchmark_tools/plotting`](benchmark_tools/plotting) |

### Add more implementations
To add a new implementation, you need to create a new subfolder with its own Cargo.toml and confiure it as a bench under `[features]`.
Then add it to the top-level `Cargo.toml` and add its path under `[dependencies]`.
Finally, implement the `RecvBuffer` trait in src/lib.rs for the new implementation.


When adding an implementation or scenario, also update the harness selections,
Python configuration, and plotting definitions. Run the existing checks from
the development shell:

```bash
cargo test --workspace
python3 -m unittest discover -s tests -v
```

## License

See [LICENSE](LICENSE) (Apache 2.0) and the notices in the adapted sources.
