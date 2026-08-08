from __future__ import annotations

import unittest

from benchmark_tools import config
from benchmark_tools.selection import (
    benchmark_env,
    normalize_perf_events,
    normalize_selected_implementations,
    normalize_selected_packet_counts,
    normalize_selected_schemes,
    perf_scenario_matrix,
)


class SelectionTests(unittest.TestCase):
    def test_scheme_aliases_are_normalized_in_pipeline_order(self) -> None:
        self.assertEqual(
            normalize_selected_schemes(["random-loss,shuffle", "in_order"]),
            ["InOrder", "Shuffle", "RandomLoss"],
        )

    def test_implementation_aliases_are_normalized_in_pipeline_order(self) -> None:
        self.assertEqual(
            normalize_selected_implementations(
                ["tcp,quic_go", "s2n"], default_to_all=False
            ),
            ["quic-go", "linux_tcp", "s2n-quic"],
        )

    def test_packet_counts_keep_known_order_then_sort_custom_values(self) -> None:
        self.assertEqual(
            normalize_selected_packet_counts(
                ["700,60000,100", "500"], default_to_all=False
            ),
            [100, 60_000, 500, 700],
        )

    def test_perf_events_preserve_commas_inside_pmu_configuration(self) -> None:
        self.assertEqual(
            normalize_perf_events(
                [["cycles,{cpu/event=0x3c,umask=0x00/}"], ["cache-misses"]]
            ),
            ["cycles", "{cpu/event=0x3c,umask=0x00/}", "cache-misses"],
        )

    def test_all_selects_default_values(self) -> None:
        self.assertEqual(
            normalize_selected_implementations(["all"], default_to_all=False),
            list(config.IMPLEMENTATION_ORDER),
        )
        self.assertEqual(
            normalize_selected_packet_counts(["all"], default_to_all=False),
            list(config.PACKET_COUNT_ORDER),
        )

    def test_scenario_matrix_uses_fixed_packet_count_for_loss_rate(self) -> None:
        self.assertEqual(
            perf_scenario_matrix(
                ["quiche"],
                ["RandomLossRate", "RandomLoss"],
                [100, 10_000],
            ),
            [
                ("quiche", "RandomLossRate", 10_000),
                ("quiche", "RandomLoss", 100),
                ("quiche", "RandomLoss", 10_000),
            ],
        )

    def test_benchmark_environment_sets_both_packet_count_variables(self) -> None:
        self.assertEqual(
            benchmark_env(["Shuffle"], ["quiche"], [100, 1_000]),
            {
                config.BENCH_SCHEMES_ENV_VAR: "Shuffle",
                config.BENCH_IMPLEMENTATIONS_ENV_VAR: "quiche",
                config.BENCH_BURST_SIZES_ENV_VAR: "100,1000",
                config.BENCH_PACKET_COUNTS_ENV_VAR: "100,1000",
            },
        )
