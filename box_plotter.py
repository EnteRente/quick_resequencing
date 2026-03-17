#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import math
import os
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

cache_root = Path("target/.cache").resolve()
cache_root.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("XDG_CACHE_HOME", str(cache_root))

matplotlib_cache = Path("target/.matplotlib").resolve()
matplotlib_cache.mkdir(parents=True, exist_ok=True)
os.environ.setdefault("MPLCONFIGDIR", str(matplotlib_cache))

import matplotlib

matplotlib.use("Agg")

import matplotlib.pyplot as plt
from matplotlib.patches import Patch
from matplotlib.ticker import EngFormatter


IMPLEMENTATION_NAMES = {
    "linux_tcp": "linux_tcp",
    "quicgo": "quic-go",
    "quic-go": "quic-go",
    "quiche": "quiche",
}

IMPLEMENTATION_ORDER = ["linux_tcp", "quic-go", "quiche"]
IMPLEMENTATION_COLORS = {
    "linux_tcp": "#355070",
    "quic-go": "#6D597A",
    "quiche": "#B56576",
}


@dataclass(frozen=True)
class BenchmarkRecord:
    group: str
    implementation: str
    scenario: str
    size: int
    size_label: str
    samples_seconds: list[float]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate boxplots from Criterion benchmark samples."
    )
    parser.add_argument(
        "--criterion-root",
        type=Path,
        default=Path("target/criterion"),
        help="Path to the Criterion output directory.",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=Path("target/criterion_boxplot.png"),
        help="Output image path.",
    )
    parser.add_argument(
        "--group",
        action="append",
        default=[],
        help="Only include the given Criterion benchmark group. Repeat to select more than one.",
    )
    parser.add_argument(
        "--dpi",
        type=int,
        default=220,
        help="Image DPI.",
    )
    return parser.parse_args()


def normalize_implementation(name: str) -> str:
    return IMPLEMENTATION_NAMES.get(name, name)


def parse_value_str(value_str: str) -> tuple[str, int, str]:
    if "/" in value_str:
        scenario, size_label = value_str.rsplit("/", 1)
    else:
        scenario, size_label = "default", value_str

    size = int(size_label.replace("_", ""))
    return scenario, size, size_label


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def select_benchmark_runs(root: Path) -> Iterable[tuple[Path, Path]]:
    selected: dict[Path, tuple[int, Path, Path]] = {}
    priority = {"base": 0, "new": 1}

    for benchmark_path in sorted(root.rglob("benchmark.json")):
        sample_path = benchmark_path.with_name("sample.json")
        if not sample_path.exists():
            continue

        run_name = benchmark_path.parent.name
        bench_root = benchmark_path.parent.parent
        rank = priority.get(run_name, math.inf)
        previous = selected.get(bench_root)

        if previous is None or rank < previous[0]:
            selected[bench_root] = (rank, benchmark_path, sample_path)

    return (
        (benchmark_path, sample_path)
        for _, benchmark_path, sample_path in sorted(selected.values(), key=lambda item: str(item[1]))
    )


def collect_records(root: Path, selected_groups: set[str]) -> list[BenchmarkRecord]:
    records: list[BenchmarkRecord] = []

    for benchmark_path, sample_path in select_benchmark_runs(root):
        benchmark = load_json(benchmark_path)
        sample = load_json(sample_path)

        group = benchmark["group_id"]
        if selected_groups and group not in selected_groups:
            continue

        implementation = normalize_implementation(benchmark["function_id"])
        scenario, size, size_label = parse_value_str(benchmark["value_str"])

        iters = sample["iters"]
        times = sample["times"]
        if len(iters) != len(times):
            raise ValueError(f"Mismatched sample lengths in {sample_path}")

        samples_seconds = [
            (float(total_time) / float(iterations)) / 1_000_000_000.0
            for iterations, total_time in zip(iters, times)
            if float(iterations) > 0
        ]

        if not samples_seconds:
            continue

        records.append(
            BenchmarkRecord(
                group=group,
                implementation=implementation,
                scenario=scenario,
                size=size,
                size_label=size_label,
                samples_seconds=samples_seconds,
            )
        )

    return records


def implementation_sort_key(name: str) -> tuple[int, str]:
    if name in IMPLEMENTATION_ORDER:
        return (IMPLEMENTATION_ORDER.index(name), name)
    return (len(IMPLEMENTATION_ORDER), name)


def plot_axis(ax: plt.Axes, group: str, scenario: str, records: list[BenchmarkRecord]) -> list[str]:
    sizes = sorted({record.size for record in records})
    size_labels = {record.size: record.size_label for record in records}
    implementations = sorted(
        {record.implementation for record in records},
        key=implementation_sort_key,
    )

    records_by_key = {
        (record.size, record.implementation): record.samples_seconds for record in records
    }

    centers = list(range(1, len(sizes) + 1))
    cluster_width = 0.78
    box_width = min(0.22, cluster_width / max(len(implementations), 1) * 0.9)
    step = cluster_width / max(len(implementations), 1)

    for impl_index, implementation in enumerate(implementations):
        offset = (impl_index - (len(implementations) - 1) / 2.0) * step
        datasets = []
        positions = []

        for center, size in zip(centers, sizes):
            samples = records_by_key.get((size, implementation))
            if not samples:
                continue
            datasets.append(samples)
            positions.append(center + offset)

        if not datasets:
            continue

        ax.boxplot(
            datasets,
            positions=positions,
            widths=box_width,
            patch_artist=True,
            showfliers=False,
            manage_ticks=False,
            boxprops={
                "facecolor": IMPLEMENTATION_COLORS.get(implementation, "#8D99AE"),
                "edgecolor": "#1F1F1F",
                "linewidth": 1.0,
                "alpha": 0.85,
            },
            whiskerprops={"color": "#1F1F1F", "linewidth": 1.0},
            capprops={"color": "#1F1F1F", "linewidth": 1.0},
            medianprops={"color": "#F7F7F7", "linewidth": 1.2},
        )

    title = group if scenario == "default" else f"{group} | {scenario}"
    ax.set_title(title, fontsize=11, loc="left")
    ax.set_xticks(centers)
    ax.set_xticklabels([size_labels[size] for size in sizes])
    ax.set_xlabel("Packets")
    ax.set_ylabel("Time per iteration")
    ax.set_yscale("log")
    ax.yaxis.set_major_formatter(EngFormatter(unit="s"))
    ax.grid(axis="y", which="both", linestyle=":", linewidth=0.7, alpha=0.5)
    ax.set_axisbelow(True)

    return implementations


def create_figure(records: list[BenchmarkRecord]) -> plt.Figure:
    groups = sorted({record.group for record in records})
    group_to_scenarios: dict[str, list[str]] = defaultdict(list)

    for group in groups:
        scenarios = sorted(
            {record.scenario for record in records if record.group == group},
            key=lambda scenario: (scenario == "default", scenario),
        )
        group_to_scenarios[group] = scenarios

    total_axes = sum(len(scenarios) for scenarios in group_to_scenarios.values())
    max_sizes = max(
        len({record.size for record in records if record.group == group})
        for group in groups
    )
    width = max(10.0, 2.0 + max_sizes * 1.5)
    height = max(4.0, total_axes * 3.1 + 1.2)

    fig, axes = plt.subplots(total_axes, 1, figsize=(width, height), squeeze=False)
    flattened_axes = axes.flatten()

    legend_order: list[str] = []
    axis_index = 0

    for group in groups:
        for scenario in group_to_scenarios[group]:
            axis_records = [
                record
                for record in records
                if record.group == group and record.scenario == scenario
            ]
            implementations = plot_axis(flattened_axes[axis_index], group, scenario, axis_records)
            for implementation in implementations:
                if implementation not in legend_order:
                    legend_order.append(implementation)
            axis_index += 1

    legend_handles = [
        Patch(
            facecolor=IMPLEMENTATION_COLORS.get(implementation, "#8D99AE"),
            edgecolor="#1F1F1F",
            label=implementation,
        )
        for implementation in sorted(legend_order, key=implementation_sort_key)
    ]

    fig.suptitle("Criterion Benchmark Suite Boxplots", fontsize=14, y=0.995)
    if legend_handles:
        fig.legend(
            handles=legend_handles,
            ncol=len(legend_handles),
            frameon=False,
            loc="upper center",
            bbox_to_anchor=(0.5, 0.975),
        )
    fig.tight_layout(rect=(0, 0, 1, 0.96))
    return fig


def main() -> int:
    args = parse_args()
    criterion_root = args.criterion_root.resolve()
    output_path = args.output.resolve()

    if not criterion_root.exists():
        raise FileNotFoundError(f"Criterion output directory not found: {criterion_root}")

    records = collect_records(criterion_root, set(args.group))
    if not records:
        raise RuntimeError(
            f"No Criterion benchmark samples found under {criterion_root} "
            "for the requested group selection."
        )

    output_path.parent.mkdir(parents=True, exist_ok=True)
    figure = create_figure(records)
    figure.savefig(output_path, dpi=args.dpi, bbox_inches="tight")
    plt.close(figure)

    print(f"Wrote boxplot to {output_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
