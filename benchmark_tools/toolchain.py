"""Nix, Python, Rust, and Cargo integration for the benchmark pipeline."""

from __future__ import annotations

import importlib.util
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Optional, Sequence

from benchmark_tools.commands import run_command, running_as_root
from benchmark_tools.config import NIX_DEV_ENV_VAR, REPO_ROOT

FLAKE_FILE_PATH = REPO_ROOT / "flake.nix"
MIN_PYTHON = (3, 9, 0)
MIN_RUST = (1, 85, 0)


def resolve_repo_path(path: Path) -> Path:
    return path if path.is_absolute() else (REPO_ROOT / path).resolve()


def format_version(version: Sequence[int]) -> str:
    return ".".join(str(part) for part in version)


def parse_semver(text: str, pattern: str) -> tuple[int, int, int]:
    match = re.search(pattern, text)
    if not match:
        raise RuntimeError(f"Could not parse version from: {text.strip()}")
    return tuple(int(match.group(index)) for index in range(1, 4))


def running_in_nix_shell() -> bool:
    return (
        os.environ.get(NIX_DEV_ENV_VAR) == "1"
        or bool(os.environ.get("IN_NIX_SHELL"))
    )


def ensure_nix_dev_shell(no_nix: bool, argv: Sequence[str]) -> None:
    if no_nix or running_in_nix_shell():
        return
    if not FLAKE_FILE_PATH.exists():
        raise FileNotFoundError(f"Nix flake not found: {FLAKE_FILE_PATH}")

    nix_executable = shutil.which("nix")
    if nix_executable is None:
        raise RuntimeError(
            "This pipeline is configured to run through the Nix dev shell, but 'nix' "
            "was not found in PATH. Install Nix, or rerun with --no-nix if your "
            "current environment already provides Python/matplotlib, rustc, and cargo."
        )

    command = [
        nix_executable,
        "develop",
        os.fspath(REPO_ROOT),
        "-c",
        "python3",
        "-m",
        "benchmark_tools",
        "--no-nix",
        *argv,
    ]
    display = " ".join(shlex.quote(part) for part in command)
    print(f"[info] Re-entering pipeline through Nix dev shell: {display}")
    completed = subprocess.run(command, cwd=REPO_ROOT)
    raise SystemExit(completed.returncode)


def verify_python_version() -> tuple[int, int, int]:
    current = sys.version_info[:3]
    if current < MIN_PYTHON:
        raise RuntimeError(
            f"Python {format_version(MIN_PYTHON)} or newer is required; "
            f"found {format_version(current)}."
        )
    return current


def verify_python_dependencies(*, skip_plot: bool) -> None:
    if skip_plot:
        return
    if importlib.util.find_spec("matplotlib") is None:
        raise RuntimeError(
            "Python package 'matplotlib' is not available in the active environment. "
            "Enter the Nix dev shell with 'nix develop' and rerun the pipeline."
        )


def verify_rust_toolchain() -> tuple[str, str]:
    rustc = run_command(["rustc", "--version"], capture_output=True).stdout.strip()
    cargo = run_command(["cargo", "--version"], capture_output=True).stdout.strip()
    rustc_version = parse_semver(rustc, r"rustc (\d+)\.(\d+)\.(\d+)")
    if rustc_version < MIN_RUST:
        raise RuntimeError(
            f"Rust {format_version(MIN_RUST)} or newer is required for edition 2024; "
            f"found {format_version(rustc_version)}."
        )
    return rustc, cargo


def verify_perf_support(perf_mode: str, sudo_perf: bool) -> None:
    if perf_mode == "none":
        return
    if sys.platform != "linux":
        raise RuntimeError(
            "Linux perf integration is only supported when this script runs on Linux."
        )
    if shutil.which("perf") is None:
        raise RuntimeError(
            "perf was requested, but the 'perf' executable was not found in PATH."
        )
    if sudo_perf and not running_as_root() and shutil.which("sudo") is None:
        raise RuntimeError(
            "--sudo-perf was requested, but the 'sudo' executable was not found in PATH."
        )


def clean_directory(path: Path) -> None:
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def run_benchmark(bench_name: str, env: dict[str, str]) -> None:
    run_command(["cargo", "bench", "--bench", bench_name], env=env)


def criterion_bench_args(bench_filter: Optional[str] = None) -> list[str]:
    args = ["--bench"]
    if bench_filter:
        args.append(bench_filter)
    return args


def build_bench_binary(bench_name: str) -> Path:
    command = [
        "cargo",
        "bench",
        "--bench",
        bench_name,
        "--no-run",
        "--message-format=json",
    ]
    result = run_command(command, capture_stdout=True)

    bench_binary: Path | None = None
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target") or {}
        target_kinds = target.get("kind") or []
        if target.get("name") != bench_name or "bench" not in target_kinds:
            continue
        executable = message.get("executable")
        if executable:
            bench_binary = Path(executable)

    if bench_binary is None:
        raise RuntimeError(
            f"Could not determine the built executable for bench target '{bench_name}'."
        )
    if not bench_binary.exists():
        raise FileNotFoundError(f"Built bench executable not found: {bench_binary}")
    return bench_binary
