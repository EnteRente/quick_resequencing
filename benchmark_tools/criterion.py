"""Read and normalize Criterion benchmark output."""

from __future__ import annotations

import json
import math
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

from benchmark_tools.config import (
    RANDOM_LOSS_RATE_SCENARIO,
    SCHEME_ORDER,
    normalize_implementation,
    normalize_scheme,
)

PLOT_EXCLUDED_IMPLEMENTATIONS = {"lsquic"}
EXCLUDED_PACKET_COUNTS = {1_000_000}
RANDOM_LOSS_RATE_PATTERN = re.compile(
    r"^RandomLoss with (?P<percent>\d+(?:\.\d+)?)% loss$"
)


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
    return {normalize_scheme(scheme) for scheme in schemes}


def normalize_selected_implementations(implementations: Iterable[str]) -> set[str]:
    return {
        normalize_implementation(implementation)
        for implementation in implementations
    }


def select_benchmark_runs(root: Path) -> Iterable[tuple[Path, Path]]:
    selected: dict[Path, tuple[float, Path, Path]] = {}
    priority = {"new": 0.0, "base": 1.0}

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
        for _, benchmark_path, sample_path in sorted(
            selected.values(), key=lambda item: str(item[1])
        )
    )


def collect_records(
    root: Path,
    selected_group: str,
    selected_schemes: set[str],
    selected_implementations: set[str],
    selected_packet_counts: set[int],
) -> list[BenchmarkRecord]:
    records: list[BenchmarkRecord] = []

    for benchmark_path, sample_path in select_benchmark_runs(root):
        benchmark = load_json(benchmark_path)
        sample = load_json(sample_path)

        group = benchmark["group_id"]
        if group != selected_group:
            continue

        implementation = normalize_implementation(benchmark["function_id"])
        if implementation in PLOT_EXCLUDED_IMPLEMENTATIONS:
            continue

        scenario, size, size_label, x_value, x_label = parse_value_str(
            benchmark["value_str"]
        )
        if scenario not in SCHEME_ORDER:
            continue
        if selected_implementations and implementation not in selected_implementations:
            continue
        if size in EXCLUDED_PACKET_COUNTS:
            continue
        if selected_schemes and scenario not in selected_schemes:
            continue
        if selected_packet_counts and size not in selected_packet_counts:
            continue

        iterations = sample["iters"]
        times = sample["times"]
        if len(iterations) != len(times):
            raise ValueError(f"Mismatched sample lengths in {sample_path}")

        samples_seconds = [
            ((float(total_time) / float(iteration_count)) / 1_000_000_000.0)
            / float(size)
            for iteration_count, total_time in zip(iterations, times)
            if float(iteration_count) > 0 and size > 0
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
