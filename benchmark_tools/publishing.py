"""Archive benchmark artifacts and maintain latest-run snapshots."""

from __future__ import annotations

import json
import shutil
from datetime import datetime
from pathlib import Path
from typing import Optional

from benchmark_tools.commands import git_output

RUN_DIRECTORY_TIME_FORMAT = "%Y-%m-%d_%H%M%S"


def format_run_directory_name(generated_at: datetime) -> str:
    """Return a readable timestamp that is safe to use as a directory name."""
    return generated_at.strftime(RUN_DIRECTORY_TIME_FORMAT)


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def archive_perf_context(
    perf_output_dir: Path, bench_binary: Path
) -> dict[str, str]:
    context_dir = perf_output_dir / "context"
    context_dir.mkdir(parents=True, exist_ok=True)

    archived_bench_binary = context_dir / bench_binary.name
    shutil.copy2(bench_binary, archived_bench_binary)
    metadata = {
        "archived_bench_binary": f"context/{archived_bench_binary.name}",
    }

    git_commit = git_output(["rev-parse", "HEAD"])
    if git_commit:
        (context_dir / "git_commit.txt").write_text(
            git_commit + "\n", encoding="utf-8"
        )
        metadata["git_commit"] = git_commit
        metadata["archived_git_commit"] = "context/git_commit.txt"

    git_short_commit = git_output(["rev-parse", "--short", "HEAD"])
    if git_short_commit:
        metadata["git_commit_short"] = git_short_commit

    git_status = git_output(["status", "--short"])
    if git_status is not None:
        status_path = context_dir / "git_status.txt"
        status_path.write_text(
            git_status + ("\n" if git_status else ""), encoding="utf-8"
        )
        metadata["git_dirty"] = str(bool(git_status))
        metadata["archived_git_status"] = "context/git_status.txt"

    return metadata


def collect_plot_files(plot_output_dir: Path) -> list[Path]:
    if not plot_output_dir.exists():
        raise FileNotFoundError(
            f"Plot output directory not found: {plot_output_dir}"
        )
    plots = sorted(plot_output_dir.glob("*.pdf"))
    if not plots:
        raise RuntimeError(f"No plot PDF files found in {plot_output_dir}")
    return plots


def _publish_snapshot(
    source_dir: Path,
    publish_dir: Path,
    category: str,
    run_name: str,
) -> tuple[Path, Path]:
    category_dir = publish_dir / category
    run_dir = category_dir / "runs" / run_name
    latest_dir = category_dir / "latest"

    run_dir.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source_dir, run_dir)
    if latest_dir.exists():
        shutil.rmtree(latest_dir)
    shutil.copytree(source_dir, latest_dir)
    return run_dir, latest_dir


def publish_plots(
    plot_output_dir: Path,
    publish_dir: Path,
    metadata: dict[str, str],
    run_name: str,
) -> tuple[Path, Path]:
    targets = _publish_snapshot(
        plot_output_dir, publish_dir, "plots", run_name
    )
    for target_dir in targets:
        write_json(target_dir / "run_metadata.json", metadata)
    return targets


def _published_perf_metadata(
    target_dir: Path, metadata: dict[str, str]
) -> dict[str, str]:
    published = dict(metadata)
    for file_name, metadata_key in (
        ("perf.data", "archived_perf_data"),
        ("perf_report.txt", "archived_perf_report"),
        ("perf_stat.txt", "archived_perf_stat"),
    ):
        path = target_dir / file_name
        if path.exists():
            published[metadata_key] = str(path)

    context_dir = target_dir / "context"
    archived_binary = metadata.get("archived_bench_binary")
    if archived_binary:
        binary_path = context_dir / Path(archived_binary).name
        if binary_path.exists():
            published["archived_bench_binary"] = str(binary_path)

    for file_name, metadata_key in (
        ("git_commit.txt", "archived_git_commit"),
        ("git_status.txt", "archived_git_status"),
    ):
        path = context_dir / file_name
        if path.exists():
            published[metadata_key] = str(path)
    return published


def publish_perf_artifacts(
    perf_output_dir: Path,
    publish_dir: Path,
    metadata: dict[str, str],
    run_name: str,
) -> tuple[Path, Path]:
    targets = _publish_snapshot(perf_output_dir, publish_dir, "perf", run_name)
    for target_dir in targets:
        write_json(
            target_dir / "run_metadata.json",
            _published_perf_metadata(target_dir, metadata),
        )
    return targets


def publish_perf_plot_files(
    perf_output_dir: Path,
    publish_dir: Path,
    metadata: dict[str, str],
) -> Optional[Path]:
    plot_file_names = [
        file_name
        for file_name in metadata.get("perf_counter_plot_files", "").split(",")
        if file_name
    ]
    if not plot_file_names:
        return None

    latest_plot_dir = publish_dir / "latest" / "plots"
    latest_plot_dir.mkdir(parents=True, exist_ok=True)
    for existing_plot in latest_plot_dir.glob("perf_*.pdf"):
        existing_plot.unlink()

    for plot_file_name in plot_file_names:
        source_path = perf_output_dir / plot_file_name
        if not source_path.exists():
            raise FileNotFoundError(f"Perf plot PDF not found: {source_path}")
        shutil.copy2(source_path, latest_plot_dir / source_path.name)

    write_json(latest_plot_dir / "perf_run_metadata.json", metadata)
    return latest_plot_dir
