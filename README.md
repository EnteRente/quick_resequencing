# quick_resequencing

Benchmarks receive-side resequencing algorithms from
[QUIC](https://www.rfc-editor.org/rfc/rfc9000.html) implementations and the
[Linux TCP receive path](https://github.com/torvalds/linux/blob/master/net/ipv4/tcp_input.c).
The benchmark harness uses [Criterion.rs](https://bheisler.github.io/criterion.rs/book/)
and [Rust](https://rust-lang.org/).

## Quick start

[Nix](https://nixos.org/) with flakes enabled is the recommended environment:

```bash
git clone <repo-url>
cd quick_resequencing
python3 -m benchmark_tools
```

The command enters the dev shell from [flake.nix](flake.nix), runs the
benchmarks, generates PDF plots, and publishes the results. Use
`--no-nix` only when the current environment already provides Python with
Matplotlib, Rust 1.85 or newer, and Cargo.

## Common commands

```bash
# Run a subset
python3 -m benchmark_tools \
  --implementation quiche \
  --scheme RandomLoss \
  --burst-size 10000

# Rebuild plots from existing Criterion output
python3 -m benchmark_tools --skip-bench

# Show every option
python3 -m benchmark_tools --help
```

`--implementation`, `--scheme`, and `--burst-size` may be repeated or given
comma-separated values.

To invoke only the Criterion plotter:

```bash
python3 -m benchmark_tools.plotting.criterion \
  --criterion-root target/criterion \
  --output-dir target/criterion_packet_boxplots
```

## Linux perf

[`perf`](https://perf.wiki.kernel.org/index.php/Main_Page) integration is
Linux-only:

```bash
# Aggregate counters
python3 -m benchmark_tools --perf stat

# Record a profile and produce a text report
python3 -m benchmark_tools --perf record

# Measure each implementation separately
python3 -m benchmark_tools --perf stat --perf-by-implementation

# Measure a selected scenario
python3 -m benchmark_tools --perf record --perf-by-scenario \
  --implementation quiche --scheme RandomLoss --burst-size 10000
```

Use `--perf-events` to select counters and `--sudo-perf` when the kernel
restricts access. Only the `perf` commands are elevated.

### Perf counter boxplots

Collect repeated `cycles` and `cache-misses` samples and generate boxplots:

```bash
python3 -m benchmark_tools --perf stat --perf-counter-boxplots \
  --implementation quiche --scheme RandomLoss --burst-size 10000
```

Use `--perf-counter-samples` to change the default of three samples. To plot an
existing sample CSV without collecting data again:

```bash
python3 -m benchmark_tools.plotting.perf_counters \
  --input benchmark_results/perf/latest/perf_counter_samples.csv \
  --output-dir benchmark_results/latest/plots
```

## Results

- `target/criterion`: raw Criterion output
- `target/criterion_packet_boxplots`: generated PDFs
- `benchmark_results/plots/latest`: latest published plots
- `benchmark_results/plots/runs/<YYYY-MM-DD_HHMMSS>`: archived plot runs
- `target/perf_artifacts`: intermediate perf output
- `benchmark_results/perf/latest`: latest published perf artifacts
- `benchmark_results/perf/runs/<YYYY-MM-DD_HHMMSS>`: archived perf runs
- `benchmark_results/latest/plots`: latest perf counter boxplots

Each published run includes `run_metadata.json`. Perf recordings also archive
the benchmark binary and Git context needed to identify the measured build.

## Benchmarked cases

Ordering schemes: `InOrder`, `FirstToLast`, `Shuffle`, `RandomLoss`, and
`RandomLossRate`.

Implementations:

- [Linux TCP](https://github.com/torvalds/linux/blob/master/net/ipv4/tcp_input.c)
- [Cloudflare quiche](https://github.com/cloudflare/quiche)
- [quic-go](https://github.com/quic-go/quic-go)
- [AWS s2n-quic](https://github.com/aws/s2n-quic)
- [LiteSpeed LSQUIC](https://github.com/litespeedtech/lsquic)

## Development

The `benchmark_tools` package separates CLI parsing, pipeline orchestration,
Criterion parsing and plotting, perf execution, selection, and publishing.
Run the Python tests with:

```bash
python3 -m unittest discover -s tests -v
```
