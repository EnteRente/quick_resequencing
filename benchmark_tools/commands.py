"""Subprocess boundaries used by the benchmark pipeline."""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
from pathlib import Path
from typing import Optional, Sequence, Union

from benchmark_tools.config import REPO_ROOT

CommandPart = Union[str, os.PathLike[str]]


def run_command(
    command: Sequence[CommandPart],
    *,
    cwd: Path = REPO_ROOT,
    capture_output: bool = False,
    capture_stdout: bool = False,
    env: Optional[dict[str, str]] = None,
) -> subprocess.CompletedProcess[str]:
    if capture_output and capture_stdout:
        raise ValueError("capture_output and capture_stdout are mutually exclusive")
    display = " ".join(shlex.quote(os.fspath(part)) for part in command)
    print(f"[run] {display}")
    command_env = os.environ.copy()
    if env:
        command_env.update(env)
    return subprocess.run(
        [os.fspath(part) for part in command],
        cwd=cwd,
        check=True,
        text=True,
        capture_output=capture_output,
        stdout=subprocess.PIPE if capture_stdout else None,
        env=command_env,
    )


def running_as_root() -> bool:
    return hasattr(os, "geteuid") and os.geteuid() == 0


def perf_executable() -> str:
    return shutil.which("perf") or "perf"


def perf_command_prefix(sudo_perf: bool, env: dict[str, str]) -> list[str]:
    if not sudo_perf or running_as_root():
        return [perf_executable()]

    command = ["sudo"]
    if env:
        command.append(f"--preserve-env={','.join(sorted(env))}")
    command.append(perf_executable())
    return command


def restore_sudo_output_ownership(path: Path, sudo_perf: bool) -> None:
    if not sudo_perf or running_as_root():
        return
    if not hasattr(os, "getuid") or not hasattr(os, "getgid"):
        return

    owner = f"{os.getuid()}:{os.getgid()}"
    run_command(["sudo", "chown", owner, path])


def git_output(args: Sequence[str]) -> Optional[str]:
    if shutil.which("git") is None:
        return None

    try:
        result = subprocess.run(
            ["git", *args],
            cwd=REPO_ROOT,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None
    return result.stdout.strip()
