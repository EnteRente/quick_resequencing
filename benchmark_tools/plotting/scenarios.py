"""Pure scenario and figure-group planning for Criterion plots."""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Iterable

from benchmark_tools.config import (
    FIRST_TO_LAST_SCENARIO,
    IN_ORDER_SCENARIO,
    PLOT_IMPLEMENTATION_ORDER,
    RANDOM_LOSS_RATE_SCENARIO,
    RANDOM_LOSS_SCENARIO,
    SCHEME_ORDER,
    SHUFFLE_SCENARIO,
    ordered_sort_key,
)
from benchmark_tools.criterion import BenchmarkRecord

SHUFFLE_FOCUS_IMPLEMENTATIONS = {"quic-go", "s2n-quic"}
IN_ORDER_FOCUS_IMPLEMENTATIONS = {"lsquic"}
RANDOM_LOSS_SPLIT_GROUPS = (
    frozenset({"quic-go"}),
    frozenset({"lsquic", "quiche"}),
    frozenset({"s2n-quic", "linux_tcp"}),
)


@dataclass(frozen=True)
class ScenarioSpec:
    label: str
    x_label: str
    file_slug: str
    uses_loss_rate_axis: bool = False


SCENARIO_SPECS = {
    IN_ORDER_SCENARIO: ScenarioSpec("In order", "Packet burst size", "in_order"),
    FIRST_TO_LAST_SCENARIO: ScenarioSpec(
        "First to last", "Packet burst size", "first_to_last"
    ),
    SHUFFLE_SCENARIO: ScenarioSpec("Shuffle", "Packet burst size", "shuffle"),
    RANDOM_LOSS_SCENARIO: ScenarioSpec(
        "Random loss", "Packet burst size", "random_loss"
    ),
    RANDOM_LOSS_RATE_SCENARIO: ScenarioSpec(
        "Random loss rate",
        "Loss rate",
        "random_loss_rate",
        uses_loss_rate_axis=True,
    ),
}


@dataclass(frozen=True)
class FigureSpec:
    scenario: str
    title: str
    file_slug: str
    records: list[BenchmarkRecord]


def implementation_sort_key(name: str) -> tuple[int, str]:
    return ordered_sort_key(name, PLOT_IMPLEMENTATION_ORDER)


def scenario_sort_key(name: str) -> tuple[int, str]:
    return ordered_sort_key(name, SCHEME_ORDER)


def implementation_group_title(implementations: Iterable[str]) -> str:
    labels = {"linux_tcp": "Linux TCP"}
    return " + ".join(
        labels.get(implementation, implementation)
        for implementation in sorted(implementations, key=implementation_sort_key)
    )


def slugify(value: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", value.lower()).strip("_")


def implementation_group_slug(implementations: Iterable[str]) -> str:
    return "_".join(
        slugify(implementation)
        for implementation in sorted(implementations, key=implementation_sort_key)
    )


def scenario_display_title(scenario: str, records: list[BenchmarkRecord]) -> str:
    label = SCENARIO_SPECS.get(
        scenario, ScenarioSpec(scenario, "Packet burst size", slugify(scenario))
    ).label
    if scenario != RANDOM_LOSS_RATE_SCENARIO:
        return label

    burst_sizes = sorted({record.size for record in records})
    if len(burst_sizes) == 1:
        return f"{label} ({burst_sizes[0]:,}-packet burst)"

    burst_labels = ", ".join(f"{burst_size:,}" for burst_size in burst_sizes)
    return f"{label} (bursts: {burst_labels} packets)"


def _partition_records(
    records: list[BenchmarkRecord], implementations: set[str] | frozenset[str]
) -> tuple[list[BenchmarkRecord], list[BenchmarkRecord]]:
    included = [
        record for record in records if record.implementation in implementations
    ]
    excluded = [
        record for record in records if record.implementation not in implementations
    ]
    return included, excluded


def build_figure_specs(
    scenario: str, records: list[BenchmarkRecord]
) -> list[FigureSpec]:
    scenario_spec = SCENARIO_SPECS.get(
        scenario, ScenarioSpec(scenario, "Packet burst size", slugify(scenario))
    )
    default_title = scenario_display_title(scenario, records)
    overall_spec = FigureSpec(
        scenario,
        f"{default_title} (all implementations)",
        scenario_spec.file_slug,
        records,
    )

    if scenario == IN_ORDER_SCENARIO:
        focus_records, other_records = _partition_records(
            records, IN_ORDER_FOCUS_IMPLEMENTATIONS
        )
        grouped_specs = []
        if focus_records:
            grouped_specs.append(
                FigureSpec(
                    scenario,
                    f"{default_title} (lsquic)",
                    f"{scenario_spec.file_slug}_lsquic",
                    focus_records,
                )
            )
        if other_records:
            grouped_specs.append(
                FigureSpec(
                    scenario,
                    f"{default_title} (other implementations)",
                    f"{scenario_spec.file_slug}_others",
                    other_records,
                )
            )
        return [
            overall_spec,
            *(spec for spec in grouped_specs if len(spec.records) < len(records)),
        ]

    if scenario in {RANDOM_LOSS_SCENARIO, RANDOM_LOSS_RATE_SCENARIO}:
        grouped_specs = []
        for implementations in RANDOM_LOSS_SPLIT_GROUPS:
            group_records, _ = _partition_records(records, implementations)
            if not group_records:
                continue
            group_implementations = {
                record.implementation for record in group_records
            }
            grouped_specs.append(
                FigureSpec(
                    scenario,
                    f"{default_title} ({implementation_group_title(group_implementations)})",
                    (
                        f"{scenario_spec.file_slug}_"
                        f"{implementation_group_slug(group_implementations)}"
                    ),
                    group_records,
                )
            )
        return [
            overall_spec,
            *(spec for spec in grouped_specs if len(spec.records) < len(records)),
        ]

    if scenario != SHUFFLE_SCENARIO:
        return [FigureSpec(scenario, default_title, scenario_spec.file_slug, records)]

    focus_records, other_records = _partition_records(
        records, SHUFFLE_FOCUS_IMPLEMENTATIONS
    )
    if not focus_records or not other_records:
        return [overall_spec]

    return [
        overall_spec,
        FigureSpec(
            scenario,
            f"{default_title} (quic-go + s2n-quic)",
            "shuffle_quic_go_s2n_quic",
            focus_records,
        ),
        FigureSpec(
            scenario,
            f"{default_title} (other implementations)",
            "shuffle_others",
            other_records,
        ),
    ]
