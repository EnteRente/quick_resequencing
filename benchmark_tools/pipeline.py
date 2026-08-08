"""CLI configuration and top-level benchmark pipeline orchestration."""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass
from datetime import datetime
from enum import Enum
from pathlib import Path
from typing import Sequence

from benchmark_tools.config import REPO_ROOT
from benchmark_tools.perf import (
    run_perf_benchmark,
    run_perf_benchmarks_by_implementation,
    run_perf_benchmarks_by_scenario,
)
from benchmark_tools.perf_counters import run_perf_counter_boxplot_suite
from benchmark_tools.publishing import (
    collect_plot_files,
    format_run_directory_name,
    publish_perf_artifacts,
    publish_perf_plot_files,
    publish_plots,
)
from benchmark_tools.selection import (
    benchmark_env,
    normalize_perf_events,
    normalize_selected_implementations,
    normalize_selected_packet_counts,
    normalize_selected_schemes,
    selected_or_default_packet_counts,
    selected_or_default_schemes,
)
from benchmark_tools.toolchain import (
    clean_directory,
    format_version,
    resolve_repo_path,
    run_benchmark,
    verify_perf_support,
    verify_python_dependencies,
    verify_python_version,
    verify_rust_toolchain,
)


class PerfScope(Enum):
    AGGREGATE = "aggregate"
    IMPLEMENTATION = "implementation"
    SCENARIO = "scenario"
    COUNTERS = "counters"


@dataclass(frozen=True)
class PipelineConfig:
    bench_name: str
    criterion_root: Path
    plot_output_dir: Path
    publish_dir: Path
    group: str
    schemes: tuple[str, ...]
    perf_mode: str
    perf_events: tuple[str, ...]
    perf_scope: PerfScope
    perf_counter_samples: int
    implementations: tuple[str, ...]
    packet_counts: tuple[int, ...]
    sudo_perf: bool
    perf_output_dir: Path
    skip_bench: bool
    skip_plot: bool

    @classmethod
    def from_namespace(cls, args: argparse.Namespace) -> "PipelineConfig":
        if args.perf == "none" and (
            args.perf_by_implementation
            or args.perf_by_scenario
            or args.perf_counter_boxplots
        ):
            raise RuntimeError(
                "--perf-by-implementation, --perf-by-scenario, and "
                "--perf-counter-boxplots require --perf stat, record, or both."
            )
        if args.perf_counter_samples <= 0:
            raise RuntimeError("--perf-counter-samples must be greater than zero.")

        schemes = normalize_selected_schemes(args.scheme)
        implementations = normalize_selected_implementations(
            args.implementation,
            default_to_all=args.perf_by_implementation or args.perf_by_scenario,
        )
        packet_counts = normalize_selected_packet_counts(
            args.packet_counts,
            default_to_all=args.perf_by_scenario,
        )

        if args.perf_counter_boxplots:
            perf_scope = PerfScope.COUNTERS
        elif args.perf_by_scenario:
            perf_scope = PerfScope.SCENARIO
        elif args.perf_by_implementation or implementations:
            perf_scope = PerfScope.IMPLEMENTATION
        else:
            perf_scope = PerfScope.AGGREGATE

        return cls(
            bench_name=args.bench_name,
            criterion_root=resolve_repo_path(args.criterion_root),
            plot_output_dir=resolve_repo_path(args.plot_output_dir),
            publish_dir=resolve_repo_path(args.publish_dir),
            group=args.group,
            schemes=tuple(schemes),
            perf_mode=args.perf,
            perf_events=tuple(normalize_perf_events(args.perf_events)),
            perf_scope=perf_scope,
            perf_counter_samples=args.perf_counter_samples,
            implementations=tuple(implementations),
            packet_counts=tuple(packet_counts),
            sudo_perf=args.sudo_perf,
            perf_output_dir=resolve_repo_path(args.perf_output_dir),
            skip_bench=args.skip_bench,
            skip_plot=args.skip_plot,
        )

    def benchmark_environment(self) -> dict[str, str]:
        return benchmark_env(
            self.schemes,
            self.implementations if self.perf_scope is not PerfScope.SCENARIO else (),
            self.packet_counts if self.perf_scope is not PerfScope.SCENARIO else (),
        )


def run_plotter(config: PipelineConfig) -> None:
    clean_directory(config.plot_output_dir)
    from benchmark_tools.plotting.criterion import generate_criterion_plots

    generate_criterion_plots(
        config.criterion_root,
        config.plot_output_dir,
        config.group,
        config.schemes,
        config.implementations,
        config.packet_counts,
    )


def run_selected_benchmark(config: PipelineConfig) -> dict[str, str]:
    environment = config.benchmark_environment()
    if config.perf_mode == "none":
        run_benchmark(config.bench_name, environment)
        return {}
    if config.perf_scope is PerfScope.COUNTERS:
        return run_perf_counter_boxplot_suite(
            config.bench_name,
            config.perf_mode,
            config.perf_events,
            config.perf_output_dir,
            config.criterion_root,
            config.group,
            config.sudo_perf,
            config.implementations,
            config.schemes,
            config.packet_counts,
            config.perf_counter_samples,
        )
    if config.perf_scope is PerfScope.SCENARIO:
        return run_perf_benchmarks_by_scenario(
            config.bench_name,
            config.perf_mode,
            config.perf_events,
            config.perf_output_dir,
            benchmark_env(()),
            config.sudo_perf,
            config.implementations,
            selected_or_default_schemes(config.schemes),
            selected_or_default_packet_counts(config.packet_counts),
        )
    if config.perf_scope is PerfScope.IMPLEMENTATION:
        return run_perf_benchmarks_by_implementation(
            config.bench_name,
            config.perf_mode,
            config.perf_events,
            config.perf_output_dir,
            environment,
            config.sudo_perf,
            config.implementations,
        )
    return run_perf_benchmark(
        config.bench_name,
        config.perf_mode,
        config.perf_events,
        config.perf_output_dir,
        environment,
        config.sudo_perf,
    )


def build_run_metadata(
    config: PipelineConfig,
    generated_at: datetime,
    plots: Sequence[Path],
    python_executable: Path,
    python_version: Sequence[int],
    rustc_version: str,
    cargo_version: str,
    perf_metadata: dict[str, str],
) -> dict[str, str]:
    metadata = {
        "generated_at": generated_at.isoformat(timespec="seconds"),
        "repo_root": str(REPO_ROOT),
        "python_executable": str(python_executable),
        "python_version": format_version(python_version),
        "rustc_version": rustc_version,
        "cargo_version": cargo_version,
        "bench_name": config.bench_name,
        "selected_schemes": ",".join(config.schemes) if config.schemes else "all",
        "selected_implementations": (
            ",".join(config.implementations)
            if config.implementations
            else "aggregate"
        ),
        "selected_packet_counts": (
            ",".join(str(count) for count in config.packet_counts)
            if config.packet_counts
            else "all"
        ),
        "criterion_root": str(config.criterion_root),
        "plot_output_dir": str(config.plot_output_dir),
        "plot_count": str(len(plots)),
        "plot_files": ",".join(plot.name for plot in plots),
        "perf_mode": config.perf_mode,
    }
    metadata.update(perf_metadata)
    return metadata


def run_pipeline(config: PipelineConfig) -> int:
    python_executable = Path(sys.executable).resolve()
    python_version = verify_python_version()
    verify_python_dependencies(
        skip_plot=config.skip_plot and config.perf_scope is not PerfScope.COUNTERS
    )
    rustc_version, cargo_version = verify_rust_toolchain()
    verify_perf_support(config.perf_mode, config.sudo_perf)
    print(
        "[info] Toolchain OK: "
        f"python={format_version(python_version)} "
        f"rustc={rustc_version} cargo={cargo_version}"
    )

    perf_metadata = {} if config.skip_bench else run_selected_benchmark(config)
    if not config.skip_plot:
        run_plotter(config)

    plots = collect_plot_files(config.plot_output_dir)
    generated_at = datetime.now().astimezone()
    metadata = build_run_metadata(
        config,
        generated_at,
        plots,
        python_executable,
        python_version,
        rustc_version,
        cargo_version,
        perf_metadata,
    )
    run_name = format_run_directory_name(generated_at)
    run_dir, latest_dir = publish_plots(
        config.plot_output_dir,
        config.publish_dir,
        metadata,
        run_name,
    )
    print("[done] Published plots:")
    print(f"  latest: {latest_dir}")
    print(f"  archived: {run_dir}")

    if config.perf_mode != "none" and config.perf_output_dir.exists():
        perf_run_dir, perf_latest_dir = publish_perf_artifacts(
            config.perf_output_dir,
            config.publish_dir,
            metadata,
            run_name,
        )
        print("[done] Published perf artifacts:")
        print(f"  latest: {perf_latest_dir}")
        print(f"  archived: {perf_run_dir}")
        perf_plot_dir = publish_perf_plot_files(
            config.perf_output_dir, config.publish_dir, metadata
        )
        if perf_plot_dir is not None:
            print("[done] Published perf plots:")
            print(f"  latest: {perf_plot_dir}")
    return 0
