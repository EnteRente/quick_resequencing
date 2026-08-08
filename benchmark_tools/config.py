"""Domain names and defaults shared by the benchmark tools."""

from __future__ import annotations

import re
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CRITERION_ROOT = Path("target/criterion")
DEFAULT_PLOT_OUTPUT_DIR = Path("target/criterion_packet_boxplots")
DEFAULT_PERF_OUTPUT_DIR = Path("target/perf_artifacts")
DEFAULT_BENCHMARK_GROUP = "Comparison of Resequencing Data Structures"

NIX_DEV_ENV_VAR = "QUICK_RESEQUENCING_NIX_DEV"
BENCH_SCHEMES_ENV_VAR = "QUICK_RESEQUENCING_SCHEMES"
BENCH_IMPLEMENTATIONS_ENV_VAR = "QUICK_RESEQUENCING_IMPLEMENTATIONS"
BENCH_BURST_SIZES_ENV_VAR = "QUICK_RESEQUENCING_BURST_SIZES"
BENCH_PACKET_COUNTS_ENV_VAR = "QUICK_RESEQUENCING_PACKET_COUNTS"

IN_ORDER_SCENARIO = "InOrder"
FIRST_TO_LAST_SCENARIO = "FirstToLast"
SHUFFLE_SCENARIO = "Shuffle"
RANDOM_LOSS_SCENARIO = "RandomLoss"
RANDOM_LOSS_RATE_SCENARIO = "RandomLossRate"

SCHEME_ORDER = (
    IN_ORDER_SCENARIO,
    FIRST_TO_LAST_SCENARIO,
    SHUFFLE_SCENARIO,
    RANDOM_LOSS_SCENARIO,
    RANDOM_LOSS_RATE_SCENARIO,
)
SCHEME_ALIASES = {
    "inorder": IN_ORDER_SCENARIO,
    "in_order": IN_ORDER_SCENARIO,
    "firsttolast": FIRST_TO_LAST_SCENARIO,
    "first_to_last": FIRST_TO_LAST_SCENARIO,
    "shuffle": SHUFFLE_SCENARIO,
    "randomloss": RANDOM_LOSS_SCENARIO,
    "random_loss": RANDOM_LOSS_SCENARIO,
    "randomlossrate": RANDOM_LOSS_RATE_SCENARIO,
    "random_loss_rate": RANDOM_LOSS_RATE_SCENARIO,
}

IMPLEMENTATION_ORDER = ("quiche", "quic-go", "linux_tcp", "s2n-quic", "lsquic")
PLOT_IMPLEMENTATION_ORDER = ("linux_tcp", "quic-go", "quiche", "s2n-quic", "lsquic")
PERF_COUNTER_IMPLEMENTATION_ORDER = ("s2n-quic", "linux_tcp", "quiche", "quic-go")
PERF_COUNTER_SCHEMES = (RANDOM_LOSS_RATE_SCENARIO, RANDOM_LOSS_SCENARIO)
IMPLEMENTATION_ALIASES = {
    "quiche": "quiche",
    "quicgo": "quic-go",
    "quic_go": "quic-go",
    "linux_tcp": "linux_tcp",
    "tcp": "linux_tcp",
    "s2n": "s2n-quic",
    "s2n_quic": "s2n-quic",
    "lsquic": "lsquic",
}

PACKET_COUNT_ORDER = (100, 1_000, 10_000, 30_000, 60_000, 100_000)
RANDOM_LOSS_RATE_PACKET_COUNT = 10_000


def _alias_key(value: str) -> str:
    return value.strip().replace("-", "_").lower()


def canonical_scheme(value: str) -> str | None:
    return SCHEME_ALIASES.get(_alias_key(value))


def normalize_scheme(value: str) -> str:
    return canonical_scheme(value) or value


def canonical_implementation(value: str) -> str | None:
    return IMPLEMENTATION_ALIASES.get(_alias_key(value))


def normalize_implementation(value: str) -> str:
    return canonical_implementation(value) or value


def ordered_sort_key(value: str, order: Sequence[str]) -> tuple[int, str]:
    try:
        return order.index(value), value
    except ValueError:
        return len(order), value


def safe_path_component(value: str) -> str:
    safe_value = re.sub(r"[^A-Za-z0-9_.-]+", "_", value).strip("_")
    if not safe_value:
        raise ValueError(f"Cannot derive a filesystem-safe name from: {value!r}")
    return safe_value
