"""Shared Matplotlib setup and implementation styling."""

from __future__ import annotations

import os

from benchmark_tools.config import REPO_ROOT

IMPLEMENTATION_COLORS = {
    "linux_tcp": "#355070",
    "quic-go": "#ffbf00",
    "quiche": "#B56576",
    "s2n-quic": "#2A9D8F",
    "lsquic": "#E76F51",
}
DEFAULT_IMPLEMENTATION_COLOR = "#8D99AE"


def configure_matplotlib() -> None:
    """Use repository-local caches and a headless plotting backend."""
    cache_directories = {
        "XDG_CACHE_HOME": REPO_ROOT / "target" / ".cache",
        "MPLCONFIGDIR": REPO_ROOT / "target" / ".matplotlib",
    }
    for environment_variable, directory in cache_directories.items():
        directory.mkdir(parents=True, exist_ok=True)
        os.environ.setdefault(environment_variable, str(directory))

    import matplotlib

    matplotlib.use("Agg")


def implementation_color(value: str) -> str:
    return IMPLEMENTATION_COLORS.get(value, DEFAULT_IMPLEMENTATION_COLOR)
