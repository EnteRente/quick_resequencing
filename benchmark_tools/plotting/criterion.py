"""Generate timing box plots from normalized Criterion records."""

from __future__ import annotations

import argparse
from collections import defaultdict
from pathlib import Path
from statistics import median
from typing import Iterable

from benchmark_tools.config import (
    DEFAULT_BENCHMARK_GROUP,
    DEFAULT_CRITERION_ROOT,
    DEFAULT_PLOT_OUTPUT_DIR,
    SCHEME_ORDER,
)
from benchmark_tools.criterion import (
    EXCLUDED_PACKET_COUNTS,
    BenchmarkRecord,
    collect_records,
    normalize_selected_implementations,
    normalize_selected_schemes,
)
from benchmark_tools.plotting.scenarios import (
    SCENARIO_SPECS,
    build_figure_specs,
    implementation_sort_key,
    scenario_display_title,
    scenario_sort_key,
)
from benchmark_tools.plotting.style import configure_matplotlib, implementation_color

configure_matplotlib()

import matplotlib.pyplot as plt
from matplotlib.lines import Line2D
from matplotlib.ticker import EngFormatter, MaxNLocator


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate one boxplot per generator type from Criterion benchmark samples."
    )
    parser.add_argument(
        "--criterion-root",
        type=Path,
        default=DEFAULT_CRITERION_ROOT,
        help="Path to the Criterion output directory.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=DEFAULT_PLOT_OUTPUT_DIR,
        help="Directory where one image per generator type will be written.",
    )
    parser.add_argument(
        "--group",
        default=DEFAULT_BENCHMARK_GROUP,
        help="Criterion benchmark group to plot.",
    )
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
    parser.add_argument(
        "--packet-count",
        action="append",
        default=[],
        help="Only include the given packet burst size. Repeat to select more than one.",
    )
    parser.add_argument("--dpi", type=int, default=220, help="Image DPI.")
    return parser.parse_args(argv)


def implementation_offsets(count: int, spacing: float = 0.24) -> list[float]:
    midpoint = (count - 1) / 2.0
    return [(index - midpoint) * spacing for index in range(count)]


def axis_cluster_gaps(x_values: list[float]) -> list[float]:
    if len(x_values) == 1:
        return [max(x_values[0] * 0.5, 1.0)]

    gaps: list[float] = []
    for index, x_value in enumerate(x_values):
        previous_gap = x_value - x_values[index - 1] if index > 0 else None
        next_gap = (
            x_values[index + 1] - x_value
            if index < len(x_values) - 1
            else None
        )
        gaps.append(float(min(gap for gap in (previous_gap, next_gap) if gap is not None)))
    return gaps


def build_time_axis(records: list[BenchmarkRecord]) -> tuple[float, float]:
    samples = [
        sample
        for record in records
        for sample in record.samples_seconds
        if sample >= 0.0
    ]
    max_sample = max(samples)
    return 0.0, max_sample * 1.08 if max_sample > 0.0 else 1.0


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
            positions.append(x_value + offset_factor * cluster_gap * 0.06)
            widths.append(cluster_gap * 0.055)
            datasets.append(samples)
            trend_x.append(x_value)
            trend_y.append(median(samples))

        if not datasets:
            continue

        color = implementation_color(implementation)
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
                "linewidth": 1.8,
                "alpha": 0.95,
                "zorder": 3,
            },
            whiskerprops={"color": "#1F1F1F", "linewidth": 1.8, "zorder": 3},
            capprops={"color": "#1F1F1F", "linewidth": 1.8, "zorder": 3},
            medianprops={"color": "#F7F7F7", "linewidth": 2.4, "zorder": 4},
        )
        ax.plot(
            trend_x,
            trend_y,
            linestyle="--",
            linewidth=1.05,
            color=color,
            alpha=0.55,
            zorder=1,
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


def configure_axes(
    ax: plt.Axes,
    scenario: str,
    records: list[BenchmarkRecord],
    x_values: list[float],
    x_labels: dict[float, str],
) -> None:
    scenario_spec = SCENARIO_SPECS[scenario]
    cluster_gaps = axis_cluster_gaps(x_values)
    if scenario_spec.uses_loss_rate_axis:
        left_margin = cluster_gaps[0] * 0.6
        right_margin = cluster_gaps[-1] * 0.6
    else:
        left_margin = max(cluster_gaps[0] * 0.85, x_values[0] * 4.0)
        right_margin = cluster_gaps[-1] * 0.12

    ax.set_xlim(x_values[0] - left_margin, x_values[-1] + right_margin)
    ax.set_xscale("linear")
    ax.set_xticks(x_values)
    if scenario_spec.uses_loss_rate_axis:
        ax.set_xticklabels([x_labels[x_value] for x_value in x_values], fontsize=11)
    else:
        ax.set_xticklabels([])
        draw_rotated_x_labels(ax, x_values, x_labels)

    ax.set_ylim(*build_time_axis(records))
    ax.set_yscale("linear")
    ax.yaxis.set_major_locator(MaxNLocator(nbins=6))
    ax.yaxis.set_major_formatter(EngFormatter(unit="s"))
    ax.grid(axis="x", which="major", linestyle=":", linewidth=0.6, alpha=0.16)
    ax.grid(axis="y", which="major", linestyle=":", linewidth=0.7, alpha=0.45)
    ax.set_axisbelow(True)
    if scenario_spec.uses_loss_rate_axis:
        ax.set_xlabel(scenario_spec.x_label)
    else:
        ax.set_xlabel(scenario_spec.x_label, labelpad=28)
    ax.set_ylabel("Time per packet")


def add_figure_header(
    fig: plt.Figure,
    group: str,
    title: str,
    implementations: list[str],
) -> None:
    legend_handles = [
        Line2D(
            [0],
            [0],
            color=implementation_color(implementation),
            linestyle="--",
            linewidth=1.05,
            marker="s",
            markersize=8,
            markerfacecolor=implementation_color(implementation),
            markeredgecolor="#1F1F1F",
            label=implementation,
        )
        for implementation in implementations
    ]
    fig.legend(
        handles=legend_handles,
        loc="upper center",
        bbox_to_anchor=(0.56, 0.88),
        frameon=False,
        ncol=len(legend_handles),
        columnspacing=1.4,
        handletextpad=0.5,
    )
    fig.suptitle(title, fontsize=15, x=0.12, y=0.985, ha="left")
    fig.text(0.12, 0.945, group, fontsize=9, color="#5C5C5C", ha="left")
    fig.tight_layout(rect=(0.06, 0.18, 0.99, 0.845))


def create_scenario_figure(
    group: str,
    scenario: str,
    records: list[BenchmarkRecord],
    title: str | None = None,
) -> plt.Figure:
    scenario_spec = SCENARIO_SPECS[scenario]
    implementations = sorted(
        {record.implementation for record in records},
        key=implementation_sort_key,
    )
    x_values = sorted({record.x_value for record in records})
    x_labels = {record.x_value: record.x_label for record in records}
    records_by_key = {
        (record.implementation, record.x_value): record.samples_seconds
        for record in records
    }

    if scenario_spec.uses_loss_rate_axis:
        figure_width = max(12.0, 7.6 + len(x_values) * 0.7)
    else:
        figure_width = max(13.6, 7.2 + len(x_values) * 1.8)

    fig, ax = plt.subplots(figsize=(figure_width, 6.6))
    plot_scenario_series(ax, x_values, implementations, records_by_key)
    configure_axes(ax, scenario, records, x_values, x_labels)
    add_figure_header(
        fig,
        group,
        title or scenario_display_title(scenario, records),
        implementations,
    )
    return fig


def generate_criterion_plots(
    criterion_root: Path,
    output_dir: Path,
    group: str,
    schemes: Iterable[str] = (),
    implementations: Iterable[str] = (),
    packet_counts: Iterable[int] = (),
    dpi: int = 220,
) -> list[Path]:
    if not criterion_root.exists():
        raise FileNotFoundError(
            f"Criterion output directory not found: {criterion_root}"
        )

    selected_schemes = (
        normalize_selected_schemes(schemes) if schemes else set(SCHEME_ORDER)
    )
    selected_implementations = normalize_selected_implementations(implementations)
    selected_packet_counts = {
        int(packet_count)
        for packet_count in packet_counts
        if int(packet_count) not in EXCLUDED_PACKET_COUNTS
    }
    records = collect_records(
        criterion_root,
        group,
        selected_schemes,
        selected_implementations,
        selected_packet_counts,
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

    plot_paths: list[Path] = []
    for scenario in sorted(records_by_scenario, key=scenario_sort_key):
        for figure_spec in build_figure_specs(
            scenario, records_by_scenario[scenario]
        ):
            figure = create_scenario_figure(
                group,
                figure_spec.scenario,
                figure_spec.records,
                title=figure_spec.title,
            )
            output_path = output_dir / f"scenario_{figure_spec.file_slug}.pdf"
            figure.savefig(output_path, dpi=dpi, bbox_inches="tight")
            plt.close(figure)
            plot_paths.append(output_path)
            print(f"Wrote boxplot to {output_path}")
    return plot_paths


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    generate_criterion_plots(
        args.criterion_root.resolve(),
        args.output_dir.resolve(),
        args.group,
        args.scheme,
        args.implementation,
        (int(packet_count) for packet_count in args.packet_count),
        args.dpi,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
