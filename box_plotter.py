#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import math
import os
import re
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from statistics import median
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
from matplotlib.lines import Line2D
from matplotlib.ticker import EngFormatter, MaxNLocator

# Read criterion benchmark 'new' output files from target/criterion
# Generate one plot per scenario family. Most scenarios use packet burst size on
# the x-axis; the RandomLossRate range uses loss rate instead.

IMPLEMENTATION_NAMES = {
    "linux_tcp": "linux_tcp",
    "linux-tcp": "linux_tcp",
    "tcp": "linux_tcp",
    "quicgo": "quic-go",
    "quic_go": "quic-go",
    "quic-go": "quic-go",
    "quiche": "quiche",
    "s2n": "s2n-quic",
    "s2n_quic": "s2n-quic",
    "s2n-quic": "s2n-quic",
    "lsquic": "lsquic",
}

IMPLEMENTATION_ORDER = ["linux_tcp", "quic-go", "quiche", "s2n-quic", "lsquic"]
IMPLEMENTATION_COLORS = {
    "linux_tcp": "#355070",
    "quic-go": "#ffbf00",
    "quiche": "#B56576",
    "s2n-quic": "#2A9D8F",
    "lsquic": "#E76F51",
}
RANDOM_LOSS_SCENARIO = "RandomLoss"
RANDOM_LOSS_RATE_SCENARIO = "RandomLossRate"
SHUFFLE_SCENARIO = "Shuffle"
SHUFFLE_FOCUS_IMPLEMENTATIONS = {"quic-go", "s2n-quic"}
IN_ORDER_FOCUS_IMPLEMENTATIONS = {"lsquic"}
RANDOM_LOSS_SPLIT_GROUPS = [
    (
        {"quic-go"},
        "quic-go",
        "quic_go",
    ),
    (
        {"lsquic", "quiche"},
        "lsquic + quiche",
        "lsquic_quiche",
    ),
    (
        {"s2n-quic", "linux_tcp"},
        "s2n-quic + Linux TCP",
        "s2n_quic_linux_tcp",
    ),
]
RANDOM_LOSS_RATE_PATTERN = re.compile(r"^RandomLoss with (?P<percent>\d+(?:\.\d+)?)% loss$")
SCHEME_ALIASES = {
    RANDOM_LOSS_SCENARIO: RANDOM_LOSS_SCENARIO,
    RANDOM_LOSS_RATE_SCENARIO: RANDOM_LOSS_RATE_SCENARIO,
    SHUFFLE_SCENARIO: SHUFFLE_SCENARIO,
}
ALLOWED_SCENARIOS = [
    "InOrder",
    "FirstToLast",
    SHUFFLE_SCENARIO,
    RANDOM_LOSS_SCENARIO,
    RANDOM_LOSS_RATE_SCENARIO,
]
SCENARIO_ORDER = [
    "InOrder",
    "FirstToLast",
    SHUFFLE_SCENARIO,
    RANDOM_LOSS_SCENARIO,
    RANDOM_LOSS_RATE_SCENARIO,
]
SCENARIO_LABELS = {
    "InOrder": "In order",
    "FirstToLast": "First to last",
    SHUFFLE_SCENARIO: "Shuffle",
    RANDOM_LOSS_SCENARIO: "Random loss",
    RANDOM_LOSS_RATE_SCENARIO: "Random loss rate",
}
SCENARIO_X_LABELS = {
    "InOrder": "Packet burst size",
    "FirstToLast": "Packet burst size",
    SHUFFLE_SCENARIO: "Packet burst size",
    RANDOM_LOSS_SCENARIO: "Packet burst size",
    RANDOM_LOSS_RATE_SCENARIO: "Loss rate",
}
SCENARIO_FILE_SLUGS = {
    "InOrder": "in_order",
    "FirstToLast": "first_to_last",
    SHUFFLE_SCENARIO: "shuffle",
    RANDOM_LOSS_SCENARIO: "random_loss",
    RANDOM_LOSS_RATE_SCENARIO: "random_loss_rate",
}
EXCLUDED_PACKET_COUNTS = {1_000_000}


@dataclass(frozen=True)
class BenchmarkRecord:
    group: str
    implementation: str
    scenario: str
    size: int
    size_label: str
    x_value: float
    x_label: str
    samples_seconds: list[float]


@dataclass(frozen=True)
class FigureSpec:
    scenario: str
    title: str
    file_slug: str
    records: list[BenchmarkRecord]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate one boxplot per generator type from Criterion benchmark samples."
    )

    # Where we read the criterion output from
    parser.add_argument(
        "--criterion-root",
        type=Path,
        default=Path("target/criterion"),
        help="Path to the Criterion output directory.",
    )

    # Where the pngs are written to
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path("target/criterion_packet_boxplots"),
        help="Directory where one image per generator type will be written.",
    )

    # Which benchmark group to plot
    parser.add_argument(
        "--group",
        default="Comparison of Resequencing Data Structures",
        help="Criterion benchmark group to plot.",
    )

    # Option to filter packet order schemes
    parser.add_argument(
        "--scheme",
        action="append",
        default=[],
        help="Only include the given scheme/case. Repeat to select more than one.",
    )
    parser.add_argument(
        "--implementation",
        action="append",
        default=[],
        help="Only include the given implementation. Repeat to select more than one.",
    )

    # Option to filter packet burst sizes
    parser.add_argument(
        "--packet-count",
        action="append",
        default=[],
        help="Only include the given packet burst size. Repeat to select more than one.",
    )

    # Specify DPI of image
    parser.add_argument(
        "--dpi",
        type=int,
        default=220,
        help="Image DPI.",
    )
    return parser.parse_args()


def normalize_implementation(name: str) -> str:
    return IMPLEMENTATION_NAMES.get(name, name)


def format_percent_label(value: float) -> str:
    rounded = round(value)
    if math.isclose(value, rounded):
        return f"{int(rounded)}%"
    return f"{value:g}%"


def parse_value_str(value_str: str) -> tuple[str, int, str, float, str]:
    if "/" in value_str:
        raw_scenario, size_label = value_str.rsplit("/", 1)
    else:
        raw_scenario, size_label = "default", value_str

    size = int(size_label.replace("_", ""))
    loss_match = RANDOM_LOSS_RATE_PATTERN.fullmatch(raw_scenario)
    if loss_match:
        loss_percent = float(loss_match.group("percent"))
        return (
            RANDOM_LOSS_RATE_SCENARIO,
            size,
            size_label,
            loss_percent,
            format_percent_label(loss_percent),
        )

    return raw_scenario, size, size_label, float(size), size_label


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def normalize_selected_schemes(schemes: Iterable[str]) -> set[str]:
    return {SCHEME_ALIASES.get(scheme, scheme) for scheme in schemes}


def normalize_selected_implementations(implementations: Iterable[str]) -> set[str]:
    return {normalize_implementation(implementation) for implementation in implementations}


def select_benchmark_runs(root: Path) -> Iterable[tuple[Path, Path]]:
    selected: dict[Path, tuple[int, Path, Path]] = {}
    priority = {"new": 0, "base": 1}

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


def collect_records(
    root: Path,
    selected_group: str,
    selected_schemes: set[str],
    selected_implementations: set[str],
    selected_packet_counts: set[int],
) -> list[BenchmarkRecord]:
    records: list[BenchmarkRecord] = []

    # Collects filtered benchmark records with validated timing samples
    for benchmark_path, sample_path in select_benchmark_runs(root):
        benchmark = load_json(benchmark_path)
        sample = load_json(sample_path)

        group = benchmark["group_id"]
        if group != selected_group:
            continue

        implementation = normalize_implementation(benchmark["function_id"])
        scenario, size, size_label, x_value, x_label = parse_value_str(benchmark["value_str"])
        if scenario not in ALLOWED_SCENARIOS:
            continue
        if selected_implementations and implementation not in selected_implementations:
            continue
        if size in EXCLUDED_PACKET_COUNTS:
            continue
        if selected_schemes and scenario not in selected_schemes:
            continue
        if selected_packet_counts and size not in selected_packet_counts:
            continue

        iters = sample["iters"]
        times = sample["times"]
        if len(iters) != len(times):
            raise ValueError(f"Mismatched sample lengths in {sample_path}")

        samples_seconds = [
            ((float(total_time) / float(iterations)) / 1_000_000_000.0) / float(size)
            for iterations, total_time in zip(iters, times)
            if float(iterations) > 0 and size > 0
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
                x_value=x_value,
                x_label=x_label,
                samples_seconds=samples_seconds,
            )
        )

    return records


def implementation_sort_key(name: str) -> tuple[int, str]:
    if name in IMPLEMENTATION_ORDER:
        return (IMPLEMENTATION_ORDER.index(name), name)
    return (len(IMPLEMENTATION_ORDER), name)


def scenario_sort_key(name: str) -> tuple[int, str]:
    if name in SCENARIO_ORDER:
        return (SCENARIO_ORDER.index(name), name)
    return (len(SCENARIO_ORDER), name)


def scenario_uses_loss_rate_axis(scenario: str) -> bool:
    return scenario == RANDOM_LOSS_RATE_SCENARIO


def scenario_display_title(scenario: str, records: list[BenchmarkRecord]) -> str:
    label = SCENARIO_LABELS.get(scenario, scenario)
    if scenario != RANDOM_LOSS_RATE_SCENARIO:
        return label

    burst_sizes = sorted({record.size for record in records})
    if len(burst_sizes) == 1:
        return f"{label} ({burst_sizes[0]:,}-packet burst)"

    burst_labels = ", ".join(f"{burst_size:,}" for burst_size in burst_sizes)
    return f"{label} (bursts: {burst_labels} packets)"


def build_figure_specs(scenario: str, records: list[BenchmarkRecord]) -> list[FigureSpec]:
    default_title = scenario_display_title(scenario, records)
    default_slug = SCENARIO_FILE_SLUGS.get(scenario, slugify(scenario))
    overall_spec = FigureSpec(
        scenario,
        f"{default_title} (all implementations)",
        default_slug,
        records,
    )

    if scenario == "InOrder":
        focus_records = [
            record for record in records if record.implementation in IN_ORDER_FOCUS_IMPLEMENTATIONS
        ]
        other_records = [
            record for record in records if record.implementation not in IN_ORDER_FOCUS_IMPLEMENTATIONS
        ]

        grouped_specs = []
        if focus_records:
            grouped_specs.append(
                FigureSpec(
                    scenario,
                    f"{default_title} (lsquic)",
                    f"{default_slug}_lsquic",
                    focus_records,
                )
            )
        if other_records:
            grouped_specs.append(
                FigureSpec(
                    scenario,
                    f"{default_title} (other implementations)",
                    f"{default_slug}_others",
                    other_records,
                )
            )

        grouped_specs = [
            figure_spec for figure_spec in grouped_specs if len(figure_spec.records) < len(records)
        ]
        return [overall_spec, *grouped_specs]

    if scenario in {RANDOM_LOSS_SCENARIO, RANDOM_LOSS_RATE_SCENARIO}:
        grouped_specs = []
        for implementations, group_title, group_slug in RANDOM_LOSS_SPLIT_GROUPS:
            group_records = [
                record for record in records if record.implementation in implementations
            ]
            if group_records:
                grouped_specs.append(
                    FigureSpec(
                        scenario,
                        f"{default_title} ({group_title})",
                        f"{default_slug}_{group_slug}",
                        group_records,
                    )
                )

        grouped_specs = [
            figure_spec for figure_spec in grouped_specs if len(figure_spec.records) < len(records)
        ]
        return [overall_spec, *grouped_specs]

    if scenario != SHUFFLE_SCENARIO:
        return [FigureSpec(scenario, default_title, default_slug, records)]

    focus_records = [
        record for record in records if record.implementation in SHUFFLE_FOCUS_IMPLEMENTATIONS
    ]
    other_records = [
        record for record in records if record.implementation not in SHUFFLE_FOCUS_IMPLEMENTATIONS
    ]

    if not focus_records or not other_records:
        return [overall_spec]

    return [
        overall_spec,
        FigureSpec(
            scenario,
            f"{default_title} (quic-go + s2n-quic)",
            "shuffle_quic_go_s2n_quic",
            focus_records,
        ),
        FigureSpec(
            scenario,
            f"{default_title} (other implementations)",
            "shuffle_others",
            other_records,
        ),
    ]


def implementation_offsets(count: int, spacing: float = 0.24) -> list[float]:
    midpoint = (count - 1) / 2.0
    return [(index - midpoint) * spacing for index in range(count)]


def axis_cluster_gaps(x_values: list[float]) -> list[float]:
    if len(x_values) == 1:
        return [max(x_values[0] * 0.5, 1.0)]

    gaps: list[float] = []
    for index, x_value in enumerate(x_values):
        previous_gap = x_value - x_values[index - 1] if index > 0 else None
        next_gap = x_values[index + 1] - x_value if index < len(x_values) - 1 else None
        candidates = [gap for gap in (previous_gap, next_gap) if gap is not None]
        gaps.append(float(min(candidates)))

    return gaps


def build_time_axis(records: list[BenchmarkRecord]) -> tuple[float, float]:
    samples = [
        sample
        for record in records
        for sample in record.samples_seconds
        if sample >= 0.0
    ]
    max_sample = max(samples)
    upper_limit = max_sample * 1.08 if max_sample > 0.0 else 1.0
    return 0.0, upper_limit


def plot_scenario_series(
    ax: plt.Axes,
    x_values: list[float],
    implementations: list[str],
    records_by_key: dict[tuple[str, float], list[float]],
) -> None:
    cluster_gaps = axis_cluster_gaps(x_values)
    offset_factors = implementation_offsets(len(implementations), spacing=1.0)

    for offset_factor, implementation in zip(offset_factors, implementations):
        positions: list[float] = []
        widths: list[float] = []
        datasets: list[list[float]] = []
        trend_x: list[float] = []
        trend_y: list[float] = []

        for x_value, cluster_gap in zip(x_values, cluster_gaps):
            samples = records_by_key.get((implementation, x_value))
            if not samples:
                continue

            position = x_value + offset_factor * cluster_gap * 0.06
            positions.append(position)
            widths.append(cluster_gap * 0.04)
            datasets.append(samples)
            trend_x.append(x_value)
            trend_y.append(median(samples))

        if not datasets:
            continue

        color = IMPLEMENTATION_COLORS.get(implementation, "#8D99AE")
        ax.boxplot(
            datasets,
            positions=positions,
            widths=widths,
            vert=True,
            patch_artist=True,
            showfliers=False,
            manage_ticks=False,
            boxprops={
                "facecolor": color,
                "edgecolor": "#1F1F1F",
                "linewidth": 0.9,
                "alpha": 0.8,
            },
            whiskerprops={"color": "#1F1F1F", "linewidth": 0.9},
            capprops={"color": "#1F1F1F", "linewidth": 0.9},
            medianprops={"color": "#F7F7F7", "linewidth": 1.6},
        )
        ax.plot(
            trend_x,
            trend_y,
            linestyle="--",
            linewidth=0.8,
            color=color,
            alpha=0.8,
            zorder=3,
        )


def draw_rotated_x_labels(
    ax: plt.Axes,
    x_values: list[float],
    x_labels: dict[float, str],
) -> None:
    for x_value in x_values:
        ax.annotate(
            x_labels[x_value],
            xy=(x_value, 0),
            xycoords=ax.get_xaxis_transform(),
            xytext=(0, -10),
            textcoords="offset points",
            rotation=90,
            rotation_mode="default",
            ha="center",
            va="top",
            fontsize=9.5,
            clip_on=False,
            annotation_clip=False,
        )


def create_scenario_figure(
    group: str,
    scenario: str,
    records: list[BenchmarkRecord],
    title: str | None = None,
) -> plt.Figure:
    implementations = sorted(
        {record.implementation for record in records},
        key=implementation_sort_key,
    )
    x_values = sorted({record.x_value for record in records})
    x_labels = {record.x_value: record.x_label for record in records}
    records_by_key = {
        (record.implementation, record.x_value): record.samples_seconds for record in records
    }

    if scenario_uses_loss_rate_axis(scenario):
        figure_width = max(12.0, 7.6 + len(x_values) * 0.7)
    else:
        figure_width = max(13.6, 7.2 + len(x_values) * 1.8)

    fig, ax = plt.subplots(figsize=(figure_width, 6.6))
    y_min, y_max = build_time_axis(records)
    plot_scenario_series(ax, x_values, implementations, records_by_key)

    cluster_gaps = axis_cluster_gaps(x_values)
    if scenario_uses_loss_rate_axis(scenario):
        left_margin = cluster_gaps[0] * 0.6
        right_margin = cluster_gaps[-1] * 0.6
    else:
        left_margin = max(cluster_gaps[0] * 0.85, x_values[0] * 4.0)
        right_margin = cluster_gaps[-1] * 0.12

    ax.set_xlim(x_values[0] - left_margin, x_values[-1] + right_margin)
    ax.set_xscale("linear")
    ax.set_xticks(x_values)
    if scenario_uses_loss_rate_axis(scenario):
        ax.set_xticklabels([x_labels[x_value] for x_value in x_values], fontsize=11)
    else:
        ax.set_xticklabels([])
        draw_rotated_x_labels(ax, x_values, x_labels)
    ax.set_ylim(y_min, y_max)
    ax.set_yscale("linear")
    ax.yaxis.set_major_locator(MaxNLocator(nbins=6))
    ax.yaxis.set_major_formatter(EngFormatter(unit="s"))
    ax.grid(axis="x", which="major", linestyle=":", linewidth=0.6, alpha=0.16)
    ax.grid(axis="y", which="major", linestyle=":", linewidth=0.7, alpha=0.45)
    ax.set_axisbelow(True)
    if scenario_uses_loss_rate_axis(scenario):
        ax.set_xlabel(SCENARIO_X_LABELS[scenario])
    else:
        ax.set_xlabel(SCENARIO_X_LABELS[scenario], labelpad=28)
    ax.set_ylabel("Time per packet")

    legend_handles = [
        Line2D(
            [0],
            [0],
            color=IMPLEMENTATION_COLORS.get(implementation, "#8D99AE"),
            linestyle="--",
            linewidth=0.8,
            marker="s",
            markersize=8,
            markerfacecolor=IMPLEMENTATION_COLORS.get(implementation, "#8D99AE"),
            markeredgecolor="#1F1F1F",
            label=implementation,
        )
        for implementation in implementations
    ]
    fig.legend(
        handles=legend_handles,
        loc="upper center",
        bbox_to_anchor=(0.56, 0.905),
        frameon=False,
        ncol=len(legend_handles),
        columnspacing=1.4,
        handletextpad=0.5,
    )

    fig.suptitle(title or scenario_display_title(scenario, records), fontsize=15, x=0.12, y=0.985, ha="left")
    fig.text(0.12, 0.945, group, fontsize=9, color="#5C5C5C", ha="left")
    fig.tight_layout(rect=(0.06, 0.18, 0.99, 0.84))
    return fig


def slugify(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", value.lower()).strip("_")


def main() -> int:
    args = parse_args()
    criterion_root = args.criterion_root.resolve()
    output_dir = args.output_dir.resolve()

    if not criterion_root.exists():
        raise FileNotFoundError(f"Criterion output directory not found: {criterion_root}")

    selected_schemes = (
        normalize_selected_schemes(args.scheme)
        if args.scheme
        else set(ALLOWED_SCENARIOS)
    )
    selected_implementations = normalize_selected_implementations(args.implementation)
    packet_counts = {
        int(packet_count)
        for packet_count in args.packet_count
        if int(packet_count) not in EXCLUDED_PACKET_COUNTS
    }
    records = collect_records(
        criterion_root,
        args.group,
        selected_schemes,
        selected_implementations,
        packet_counts,
    )
    if not records:
        raise RuntimeError(
            f"No Criterion benchmark samples found under {criterion_root} "
            "for the requested group/scheme/packet-burst-size selection."
        )

    output_dir.mkdir(parents=True, exist_ok=True)
    records_by_scenario: dict[str, list[BenchmarkRecord]] = defaultdict(list)
    for record in records:
        records_by_scenario[record.scenario].append(record)

    for scenario in sorted(records_by_scenario, key=scenario_sort_key):
        for figure_spec in build_figure_specs(scenario, records_by_scenario[scenario]):
            figure = create_scenario_figure(
                args.group,
                figure_spec.scenario,
                figure_spec.records,
                title=figure_spec.title,
            )
            output_path = output_dir / f"scenario_{figure_spec.file_slug}.png"
            figure.savefig(output_path, dpi=args.dpi, bbox_inches="tight")
            plt.close(figure)
            print(f"Wrote boxplot to {output_path}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
