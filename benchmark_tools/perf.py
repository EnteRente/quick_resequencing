"""Aggregate, per-implementation, and per-scenario perf workflows."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Optional, Sequence

from benchmark_tools.config import (
    BENCH_BURST_SIZES_ENV_VAR,
    BENCH_IMPLEMENTATIONS_ENV_VAR,
    BENCH_PACKET_COUNTS_ENV_VAR,
    BENCH_SCHEMES_ENV_VAR,
    safe_path_component,
)
from benchmark_tools.perf_commands import (
    prepare_perf_output,
    run_selected_perf_modes,
)
from benchmark_tools.publishing import write_json
from benchmark_tools.selection import perf_scenario_matrix
from benchmark_tools.toolchain import criterion_bench_args


def run_perf_benchmark(
    bench_name: str,
    perf_mode: str,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    bench_filter: Optional[str] = None,
) -> dict[str, str]:
    bench_binary, metadata = prepare_perf_output(
        bench_name, perf_mode, perf_events, perf_output_dir, sudo_perf
    )
    if bench_filter:
        metadata["criterion_filter"] = bench_filter
    metadata.update(
        run_selected_perf_modes(
            bench_binary,
            perf_mode,
            perf_events,
            perf_output_dir,
            env,
            sudo_perf,
            criterion_bench_args(bench_filter),
        )
    )
    return metadata


def run_perf_benchmarks_by_implementation(
    bench_name: str,
    perf_mode: str,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    implementations: Sequence[str],
) -> dict[str, str]:
    bench_binary, metadata = prepare_perf_output(
        bench_name,
        perf_mode,
        perf_events,
        perf_output_dir,
        sudo_perf,
        perf_by_implementation="true",
        perf_implementations=",".join(implementations),
    )

    result_dirs: list[str] = []
    for implementation in implementations:
        result_dir_name = safe_path_component(implementation)
        result_dir = perf_output_dir / result_dir_name
        result_dir.mkdir(parents=True, exist_ok=True)
        implementation_env = {
            **env,
            BENCH_IMPLEMENTATIONS_ENV_VAR: implementation,
        }
        implementation_metadata = {
            **metadata,
            "implementation": implementation,
            "criterion_filter": implementation,
            "perf_output_dir": str(result_dir),
        }
        implementation_metadata.update(
            run_selected_perf_modes(
                bench_binary,
                perf_mode,
                perf_events,
                result_dir,
                implementation_env,
                sudo_perf,
                criterion_bench_args(implementation),
            )
        )
        write_json(result_dir / "run_metadata.json", implementation_metadata)
        result_dirs.append(result_dir_name)

    metadata["implementation_result_dirs"] = ",".join(result_dirs)
    return metadata


def run_perf_benchmarks_by_scenario(
    bench_name: str,
    perf_mode: str,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    implementations: Sequence[str],
    schemes: Sequence[str],
    packet_counts: Sequence[int],
) -> dict[str, str]:
    scenario_matrix = perf_scenario_matrix(implementations, schemes, packet_counts)
    if not scenario_matrix:
        raise RuntimeError(
            "No perf scenarios were selected. Check --scheme and --burst-size filters."
        )

    bench_binary, metadata = prepare_perf_output(
        bench_name,
        perf_mode,
        perf_events,
        perf_output_dir,
        sudo_perf,
        perf_by_scenario="true",
        perf_implementations=",".join(implementations),
        perf_schemes=",".join(schemes),
        perf_packet_counts=",".join(str(count) for count in packet_counts),
    )
    metadata["expected_scenario_count"] = str(len(scenario_matrix))

    index_entries: list[dict[str, str]] = []
    for implementation, scheme, packet_count in scenario_matrix:
        relative_dir = (
            Path(safe_path_component(implementation))
            / safe_path_component(scheme)
            / f"packets_{packet_count}"
        )
        scenario_dir = perf_output_dir / relative_dir
        scenario_dir.mkdir(parents=True, exist_ok=True)
        scenario_env = {
            **env,
            BENCH_IMPLEMENTATIONS_ENV_VAR: implementation,
            BENCH_SCHEMES_ENV_VAR: scheme,
            BENCH_BURST_SIZES_ENV_VAR: str(packet_count),
            BENCH_PACKET_COUNTS_ENV_VAR: str(packet_count),
        }
        scenario_metadata = {
            **metadata,
            "implementation": implementation,
            "scheme": scheme,
            "packet_count": str(packet_count),
            "criterion_filter": implementation,
            "perf_output_dir": str(scenario_dir),
        }
        scenario_metadata.update(
            run_selected_perf_modes(
                bench_binary,
                perf_mode,
                perf_events,
                scenario_dir,
                scenario_env,
                sudo_perf,
                criterion_bench_args(implementation),
            )
        )
        write_json(scenario_dir / "run_metadata.json", scenario_metadata)

        index_entry = {
            "implementation": implementation,
            "scheme": scheme,
            "packet_count": str(packet_count),
            "result_dir": os.fspath(relative_dir),
        }
        if perf_mode in {"stat", "both"}:
            index_entry["perf_stat_path"] = os.fspath(
                relative_dir / "perf_stat.txt"
            )
        if perf_mode in {"record", "both"}:
            index_entry["perf_data_path"] = os.fspath(relative_dir / "perf.data")
            index_entry["perf_report_path"] = os.fspath(
                relative_dir / "perf_report.txt"
            )
        index_entries.append(index_entry)

    write_json(perf_output_dir / "perf_index.json", index_entries)
    metadata["scenario_count"] = str(len(index_entries))
    metadata["archived_perf_index"] = "perf_index.json"
    return metadata
