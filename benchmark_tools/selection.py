"""Normalize CLI selections and construct benchmark scenario matrices."""

from __future__ import annotations

from typing import Sequence

from benchmark_tools.config import (
    BENCH_BURST_SIZES_ENV_VAR,
    BENCH_IMPLEMENTATIONS_ENV_VAR,
    BENCH_PACKET_COUNTS_ENV_VAR,
    BENCH_SCHEMES_ENV_VAR,
    IMPLEMENTATION_ORDER,
    PACKET_COUNT_ORDER,
    PERF_COUNTER_IMPLEMENTATION_ORDER,
    PERF_COUNTER_SCHEMES,
    RANDOM_LOSS_RATE_PACKET_COUNT,
    RANDOM_LOSS_RATE_SCENARIO,
    SCHEME_ORDER,
    canonical_implementation,
    canonical_scheme,
)


def normalize_selected_schemes(raw_schemes: Sequence[str]) -> list[str]:
    if not raw_schemes:
        return []

    selected: list[str] = []
    for raw_value in raw_schemes:
        for raw_scheme in raw_value.split(","):
            scheme = raw_scheme.strip()
            if not scheme:
                continue
            if scheme.lower() == "all":
                return []

            canonical = canonical_scheme(scheme)
            if canonical is None:
                allowed = ", ".join(SCHEME_ORDER)
                raise ValueError(
                    f"Unknown scheme '{scheme}'. Supported values: {allowed}"
                )
            if canonical not in selected:
                selected.append(canonical)

    return sorted(selected, key=SCHEME_ORDER.index)


def normalize_selected_implementations(
    raw_implementations: Sequence[str],
    *,
    default_to_all: bool,
) -> list[str]:
    if not raw_implementations:
        return list(IMPLEMENTATION_ORDER) if default_to_all else []

    selected: list[str] = []
    for raw_value in raw_implementations:
        for raw_implementation in raw_value.split(","):
            implementation = raw_implementation.strip()
            if not implementation:
                continue
            if implementation.lower() == "all":
                return list(IMPLEMENTATION_ORDER)

            canonical = canonical_implementation(implementation)
            if canonical is None:
                allowed = ", ".join(IMPLEMENTATION_ORDER)
                raise ValueError(
                    f"Unknown implementation '{implementation}'. "
                    f"Supported values: {allowed}"
                )
            if canonical not in selected:
                selected.append(canonical)

    if not selected and default_to_all:
        return list(IMPLEMENTATION_ORDER)
    return sorted(selected, key=IMPLEMENTATION_ORDER.index)


def normalize_selected_packet_counts(
    raw_packet_counts: Sequence[str],
    *,
    default_to_all: bool,
) -> list[int]:
    if not raw_packet_counts:
        return list(PACKET_COUNT_ORDER) if default_to_all else []

    selected: list[int] = []
    for raw_value in raw_packet_counts:
        for raw_packet_count in raw_value.split(","):
            packet_count_value = raw_packet_count.strip()
            if not packet_count_value:
                continue
            if packet_count_value.lower() == "all":
                return list(PACKET_COUNT_ORDER)

            try:
                packet_count = int(packet_count_value)
            except ValueError as error:
                raise ValueError(
                    f"Invalid packet count '{packet_count_value}'."
                ) from error
            if packet_count <= 0:
                raise ValueError(
                    f"Packet count must be greater than zero: {packet_count}"
                )
            if packet_count not in selected:
                selected.append(packet_count)

    if not selected and default_to_all:
        return list(PACKET_COUNT_ORDER)
    return sorted(
        selected,
        key=lambda packet_count: (
            PACKET_COUNT_ORDER.index(packet_count)
            if packet_count in PACKET_COUNT_ORDER
            else len(PACKET_COUNT_ORDER),
            packet_count,
        ),
    )


def selected_or_default_schemes(selected_schemes: Sequence[str]) -> list[str]:
    return list(selected_schemes) if selected_schemes else list(SCHEME_ORDER)


def selected_or_default_packet_counts(
    selected_packet_counts: Sequence[int],
) -> list[int]:
    return (
        list(selected_packet_counts)
        if selected_packet_counts
        else list(PACKET_COUNT_ORDER)
    )


def selected_or_default_perf_counter_implementations(
    selected_implementations: Sequence[str],
) -> list[str]:
    if not selected_implementations:
        return list(PERF_COUNTER_IMPLEMENTATION_ORDER)

    ordered = [
        implementation
        for implementation in PERF_COUNTER_IMPLEMENTATION_ORDER
        if implementation in selected_implementations
    ]
    ordered.extend(
        implementation
        for implementation in selected_implementations
        if implementation not in ordered
    )
    return ordered


def selected_or_default_perf_counter_schemes(
    selected_schemes: Sequence[str],
) -> list[str]:
    if not selected_schemes:
        return list(PERF_COUNTER_SCHEMES)

    selected = [scheme for scheme in PERF_COUNTER_SCHEMES if scheme in selected_schemes]
    if not selected:
        raise RuntimeError(
            "--perf-counter-boxplots only supports RandomLoss and RandomLossRate."
        )
    return selected


def packet_counts_for_scheme(
    scheme: str, packet_counts: Sequence[int]
) -> list[int]:
    if scheme == RANDOM_LOSS_RATE_SCENARIO:
        return (
            [RANDOM_LOSS_RATE_PACKET_COUNT]
            if RANDOM_LOSS_RATE_PACKET_COUNT in packet_counts
            else []
        )
    return list(packet_counts)


def perf_scenario_matrix(
    implementations: Sequence[str],
    schemes: Sequence[str],
    packet_counts: Sequence[int],
) -> list[tuple[str, str, int]]:
    return [
        (implementation, scheme, packet_count)
        for implementation in implementations
        for scheme in schemes
        for packet_count in packet_counts_for_scheme(scheme, packet_counts)
    ]


def benchmark_env(
    selected_schemes: Sequence[str],
    selected_implementations: Sequence[str] = (),
    selected_packet_counts: Sequence[int] = (),
) -> dict[str, str]:
    env: dict[str, str] = {}
    if selected_schemes:
        env[BENCH_SCHEMES_ENV_VAR] = ",".join(selected_schemes)
    if selected_implementations:
        env[BENCH_IMPLEMENTATIONS_ENV_VAR] = ",".join(selected_implementations)
    if selected_packet_counts:
        packet_counts = ",".join(str(count) for count in selected_packet_counts)
        env[BENCH_BURST_SIZES_ENV_VAR] = packet_counts
        env[BENCH_PACKET_COUNTS_ENV_VAR] = packet_counts
    return env


def split_perf_event_list(raw_value: str) -> list[str]:
    events: list[str] = []
    brace_depth = 0
    in_pmu_config = False
    start = 0

    for index, char in enumerate(raw_value):
        if char == "{":
            brace_depth += 1
        elif char == "}" and brace_depth > 0:
            brace_depth -= 1
        elif char == "/" and brace_depth == 0:
            in_pmu_config = not in_pmu_config
        elif char == "," and brace_depth == 0 and not in_pmu_config:
            event = raw_value[start:index].strip()
            if event:
                events.append(event)
            start = index + 1

    event = raw_value[start:].strip()
    if event:
        events.append(event)
    return events


def normalize_perf_events(raw_perf_events: Sequence[Sequence[str]]) -> list[str]:
    events: list[str] = []
    for raw_group in raw_perf_events:
        for raw_value in raw_group:
            for event in split_perf_event_list(raw_value):
                if event not in events:
                    events.append(event)
    return events
