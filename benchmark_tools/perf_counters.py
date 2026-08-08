"""Repeated perf-counter sampling and CSV generation."""

from __future__ import annotations

import csv
import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

from benchmark_tools.commands import (
    CommandPart,
    perf_command_prefix,
    restore_sudo_output_ownership,
    run_command,
)
from benchmark_tools.config import (
    RANDOM_LOSS_RATE_PACKET_COUNT,
    RANDOM_LOSS_RATE_SCENARIO,
    RANDOM_LOSS_SCENARIO,
    safe_path_component,
)
from benchmark_tools.perf_commands import prepare_perf_output, run_perf_record
from benchmark_tools.publishing import write_json
from benchmark_tools.selection import (
    benchmark_env,
    selected_or_default_packet_counts,
    selected_or_default_perf_counter_implementations,
    selected_or_default_perf_counter_schemes,
)
from benchmark_tools.toolchain import criterion_bench_args

DEFAULT_PERF_COUNTER_EVENTS = ("cycles", "cache-misses")
RANDOM_LOSS_RATE_PERCENTAGES = tuple(range(1, 11))


@dataclass(frozen=True)
class PerfCounterScenario:
    implementation: str
    scheme: str
    packet_count: int
    loss_rate_percent: int
    criterion_value: str
    bench_filter: str
    result_dir: Path


def build_perf_counter_scenarios(
    implementations: Sequence[str],
    schemes: Sequence[str],
    packet_counts: Sequence[int],
) -> list[PerfCounterScenario]:
    scenarios: list[PerfCounterScenario] = []
    for implementation in implementations:
        if RANDOM_LOSS_RATE_SCENARIO in schemes:
            for loss_rate_percent in RANDOM_LOSS_RATE_PERCENTAGES:
                loss_label = f"RandomLoss with {loss_rate_percent}% loss"
                criterion_value = f"{loss_label}/{RANDOM_LOSS_RATE_PACKET_COUNT}"
                relative_dir = (
                    Path(safe_path_component(implementation))
                    / RANDOM_LOSS_RATE_SCENARIO
                    / f"loss_{loss_rate_percent:02d}"
                    / f"packets_{RANDOM_LOSS_RATE_PACKET_COUNT}"
                )
                scenarios.append(
                    PerfCounterScenario(
                        implementation=implementation,
                        scheme=RANDOM_LOSS_RATE_SCENARIO,
                        packet_count=RANDOM_LOSS_RATE_PACKET_COUNT,
                        loss_rate_percent=loss_rate_percent,
                        criterion_value=criterion_value,
                        bench_filter=f"{implementation}/{criterion_value}",
                        result_dir=relative_dir,
                    )
                )

        if RANDOM_LOSS_SCENARIO in schemes:
            for packet_count in packet_counts:
                criterion_value = f"RandomLoss/{packet_count}"
                relative_dir = (
                    Path(safe_path_component(implementation))
                    / RANDOM_LOSS_SCENARIO
                    / f"packets_{packet_count}"
                )
                scenarios.append(
                    PerfCounterScenario(
                        implementation=implementation,
                        scheme=RANDOM_LOSS_SCENARIO,
                        packet_count=packet_count,
                        loss_rate_percent=1,
                        criterion_value=criterion_value,
                        bench_filter=f"{implementation}/{criterion_value}",
                        result_dir=relative_dir,
                    )
                )
    return scenarios


def parse_perf_stat_counter_values(perf_stat_path: Path) -> dict[str, float]:
    counters: dict[str, float] = {}
    with perf_stat_path.open("r", encoding="utf-8", newline="") as handle:
        for row in csv.reader(handle):
            if len(row) < 3 or not row[0] or row[0].startswith("#"):
                continue
            raw_value = row[0].strip()
            event = row[2].strip()
            if not event or raw_value.startswith("<"):
                continue
            try:
                counters[event] = float(raw_value)
            except ValueError:
                continue
    return counters


def run_perf_stat_counter_sample(
    bench_binary: Path,
    perf_events: Sequence[str],
    perf_stat_path: Path,
    env: dict[str, str],
    sudo_perf: bool,
    bench_args: Sequence[str],
) -> dict[str, float]:
    command: list[CommandPart] = perf_command_prefix(sudo_perf, env)
    command.append("stat")
    for event in perf_events:
        command.extend(["-e", event])

    perf_stat_path.parent.mkdir(parents=True, exist_ok=True)
    command.extend(["-x", ",", "-o", perf_stat_path, "--", bench_binary, *bench_args])
    run_command(command, env=env)
    restore_sudo_output_ownership(perf_stat_path, sudo_perf)
    counters = parse_perf_stat_counter_values(perf_stat_path)
    missing_events = [event for event in perf_events if event not in counters]
    if missing_events:
        raise RuntimeError(
            f"perf stat did not report counters for: {', '.join(missing_events)}"
        )
    return counters


def criterion_timed_iterations(
    criterion_root: Path,
    group: str,
    implementation: str,
    criterion_value: str,
) -> float:
    for benchmark_path in sorted(criterion_root.rglob("benchmark.json")):
        if benchmark_path.parent.name != "new":
            continue
        sample_path = benchmark_path.with_name("sample.json")
        if not sample_path.exists():
            continue

        benchmark = json.loads(benchmark_path.read_text(encoding="utf-8"))
        if benchmark.get("group_id") != group:
            continue
        if benchmark.get("function_id") != implementation:
            continue
        if benchmark.get("value_str") != criterion_value:
            continue

        sample = json.loads(sample_path.read_text(encoding="utf-8"))
        return sum(float(iterations) for iterations in sample["iters"])

    raise RuntimeError(
        "Could not find Criterion sample output for "
        f"{group}/{implementation}/{criterion_value} under {criterion_root}."
    )


def run_perf_counter_plotter(samples_csv: Path, output_dir: Path) -> list[Path]:
    from benchmark_tools.plotting.perf_counters import plot_perf_counter_boxplots

    plot_paths = plot_perf_counter_boxplots(samples_csv, output_dir)
    if not plot_paths:
        raise RuntimeError(f"No perf counter plot PDFs found in {output_dir}")
    return plot_paths


def _counter_sample_row(
    scenario: PerfCounterScenario,
    event: str,
    sample_index: int,
    raw_count: float,
    timed_iterations: float,
    measured_packets: float,
    relative_stat_path: Path,
) -> dict[str, str]:
    return {
        "implementation": scenario.implementation,
        "scheme": scenario.scheme,
        "packet_count": str(scenario.packet_count),
        "loss_rate_percent": str(scenario.loss_rate_percent),
        "criterion_value": scenario.criterion_value,
        "criterion_filter": scenario.bench_filter,
        "event": event,
        "sample_index": str(sample_index),
        "raw_count": f"{raw_count:.12g}",
        "timed_iterations": f"{timed_iterations:.12g}",
        "measured_packets": f"{measured_packets:.12g}",
        "count_per_burst": f"{raw_count / timed_iterations:.12g}",
        "count_per_packet": f"{raw_count / measured_packets:.12g}",
        "perf_stat_path": os.fspath(relative_stat_path),
    }


def run_perf_counter_boxplot_suite(
    bench_name: str,
    perf_mode: str,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    criterion_root: Path,
    group: str,
    sudo_perf: bool,
    implementations: Sequence[str],
    schemes: Sequence[str],
    packet_counts: Sequence[int],
    sample_count: int,
) -> dict[str, str]:
    if sample_count <= 0:
        raise ValueError("--perf-counter-samples must be greater than zero.")

    events = list(perf_events) if perf_events else list(DEFAULT_PERF_COUNTER_EVENTS)
    selected_implementations = selected_or_default_perf_counter_implementations(
        implementations
    )
    selected_schemes = selected_or_default_perf_counter_schemes(schemes)
    selected_packet_counts = selected_or_default_packet_counts(packet_counts)
    scenarios = build_perf_counter_scenarios(
        selected_implementations, selected_schemes, selected_packet_counts
    )
    if not scenarios:
        raise RuntimeError("No perf counter scenarios were selected.")

    bench_binary, metadata = prepare_perf_output(
        bench_name,
        perf_mode,
        events,
        perf_output_dir,
        sudo_perf,
        perf_counter_boxplots="true",
        perf_counter_samples=str(sample_count),
        perf_implementations=",".join(selected_implementations),
        perf_schemes=",".join(selected_schemes),
        perf_packet_counts=",".join(str(count) for count in selected_packet_counts),
    )

    sample_rows: list[dict[str, str]] = []
    index_entries: list[dict[str, str]] = []
    for scenario in scenarios:
        scenario_dir = perf_output_dir / scenario.result_dir
        scenario_dir.mkdir(parents=True, exist_ok=True)
        scenario_env = benchmark_env(
            [scenario.scheme], [scenario.implementation], [scenario.packet_count]
        )
        bench_args = criterion_bench_args(scenario.bench_filter)
        scenario_metadata: dict[str, str] = {
            "implementation": scenario.implementation,
            "scheme": scenario.scheme,
            "packet_count": str(scenario.packet_count),
            "loss_rate_percent": str(scenario.loss_rate_percent),
            "criterion_value": scenario.criterion_value,
            "criterion_filter": scenario.bench_filter,
            "perf_output_dir": str(scenario_dir),
        }

        if perf_mode in {"record", "both"}:
            scenario_metadata.update(
                run_perf_record(
                    bench_binary,
                    events,
                    scenario_dir,
                    scenario_env,
                    sudo_perf,
                    bench_args,
                )
            )

        stat_sample_paths: list[str] = []
        for sample_index in range(1, sample_count + 1):
            perf_stat_path = (
                scenario_dir
                / "stat_samples"
                / f"perf_stat_sample_{sample_index:02d}.csv"
            )
            counters = run_perf_stat_counter_sample(
                bench_binary,
                events,
                perf_stat_path,
                scenario_env,
                sudo_perf,
                bench_args,
            )
            timed_iterations = criterion_timed_iterations(
                criterion_root,
                group,
                scenario.implementation,
                scenario.criterion_value,
            )
            measured_packets = timed_iterations * float(scenario.packet_count)
            if measured_packets <= 0.0:
                raise RuntimeError(
                    f"Criterion reported no timed iterations for {scenario.bench_filter}."
                )

            relative_stat_path = perf_stat_path.relative_to(perf_output_dir)
            stat_sample_paths.append(os.fspath(relative_stat_path))
            sample_rows.extend(
                _counter_sample_row(
                    scenario,
                    event,
                    sample_index,
                    counters[event],
                    timed_iterations,
                    measured_packets,
                    relative_stat_path,
                )
                for event in events
            )

        scenario_metadata["perf_stat_samples"] = ",".join(stat_sample_paths)
        write_json(scenario_dir / "run_metadata.json", scenario_metadata)
        index_entry = dict(scenario_metadata)
        index_entry["result_dir"] = os.fspath(scenario.result_dir)
        if perf_mode in {"record", "both"}:
            index_entry["perf_data_path"] = os.fspath(
                scenario.result_dir / "perf.data"
            )
            index_entry["perf_report_path"] = os.fspath(
                scenario.result_dir / "perf_report.txt"
            )
        index_entries.append(index_entry)

    samples_csv = perf_output_dir / "perf_counter_samples.csv"
    fieldnames = list(sample_rows[0]) if sample_rows else []
    with samples_csv.open("w", encoding="utf-8", newline="") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(sample_rows)

    plot_paths = run_perf_counter_plotter(samples_csv, perf_output_dir)
    write_json(perf_output_dir / "perf_index.json", index_entries)
    metadata["scenario_count"] = str(len(index_entries))
    metadata["archived_perf_index"] = "perf_index.json"
    metadata["perf_counter_samples_csv"] = "perf_counter_samples.csv"
    metadata["perf_counter_plot_files"] = ",".join(
        os.fspath(path.relative_to(perf_output_dir)) for path in plot_paths
    )
    return metadata
