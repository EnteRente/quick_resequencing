"""Low-level perf commands and shared run initialization."""

from __future__ import annotations

from pathlib import Path
from typing import Sequence

from benchmark_tools.commands import (
    CommandPart,
    perf_command_prefix,
    restore_sudo_output_ownership,
    run_command,
)
from benchmark_tools.publishing import archive_perf_context
from benchmark_tools.toolchain import build_bench_binary, clean_directory

DEFAULT_PERF_RECORD_OPTIONS = ("--call-graph", "fp", "-F", "999")


def run_perf_stat(
    bench_binary: Path,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    bench_args: Sequence[str] = (),
) -> dict[str, str]:
    command: list[CommandPart] = perf_command_prefix(sudo_perf, env)
    command.append("stat")
    for event in perf_events:
        command.extend(["-e", event])

    perf_stat_path = perf_output_dir / "perf_stat.txt"
    command.extend(
        ["-d", "-d", "-d", "-o", perf_stat_path, bench_binary, *bench_args]
    )
    run_command(command, env=env)
    restore_sudo_output_ownership(perf_stat_path, sudo_perf)
    return {"perf_stat_path": str(perf_stat_path)}


def run_perf_record(
    bench_binary: Path,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    bench_args: Sequence[str] = (),
) -> dict[str, str]:
    command: list[CommandPart] = perf_command_prefix(sudo_perf, env)
    command.extend(["record", *DEFAULT_PERF_RECORD_OPTIONS])
    for event in perf_events:
        command.extend(["-e", event])

    perf_data_path = perf_output_dir / "perf.data"
    perf_report_path = perf_output_dir / "perf_report.txt"
    command.extend(["-o", perf_data_path, bench_binary, *bench_args])
    run_command(command, env=env)

    report_command: list[CommandPart] = perf_command_prefix(sudo_perf, env)
    report_command.extend(["report", "--stdio", "-i", perf_data_path])
    report_result = run_command(report_command, capture_output=True, env=env)
    restore_sudo_output_ownership(perf_data_path, sudo_perf)
    print(report_result.stdout, end="")
    perf_report_path.write_text(report_result.stdout, encoding="utf-8")
    return {
        "perf_data_path": str(perf_data_path),
        "perf_report_path": str(perf_report_path),
        "perf_record_options": " ".join(DEFAULT_PERF_RECORD_OPTIONS),
    }


def run_selected_perf_modes(
    bench_binary: Path,
    perf_mode: str,
    perf_events: Sequence[str],
    output_dir: Path,
    env: dict[str, str],
    sudo_perf: bool,
    bench_args: Sequence[str],
) -> dict[str, str]:
    metadata: dict[str, str] = {}
    if perf_mode in {"stat", "both"}:
        metadata.update(
            run_perf_stat(
                bench_binary,
                perf_events,
                output_dir,
                env,
                sudo_perf,
                bench_args,
            )
        )
    if perf_mode in {"record", "both"}:
        metadata.update(
            run_perf_record(
                bench_binary,
                perf_events,
                output_dir,
                env,
                sudo_perf,
                bench_args,
            )
        )
    return metadata


def prepare_perf_output(
    bench_name: str,
    perf_mode: str,
    perf_events: Sequence[str],
    perf_output_dir: Path,
    sudo_perf: bool,
    **metadata_values: str,
) -> tuple[Path, dict[str, str]]:
    bench_binary = build_bench_binary(bench_name)
    clean_directory(perf_output_dir)
    metadata = {
        "bench_binary": str(bench_binary),
        "perf_mode": perf_mode,
        "sudo_perf": str(sudo_perf),
        **metadata_values,
    }
    metadata.update(archive_perf_context(perf_output_dir, bench_binary))
    if perf_events:
        metadata["perf_events"] = ",".join(perf_events)
    return bench_binary, metadata
