from __future__ import annotations

import unittest
from pathlib import Path
from unittest.mock import patch

from benchmark_tools.perf_commands import run_perf_stat
from benchmark_tools.perf_counters import build_perf_counter_scenarios


class PerfScenarioTests(unittest.TestCase):
    def test_perf_counter_scenarios_include_rates_and_packet_counts(self) -> None:
        scenarios = build_perf_counter_scenarios(
            ["s2n-quic"],
            ["RandomLossRate", "RandomLoss"],
            [100, 1_000],
        )

        self.assertEqual(len(scenarios), 12)
        self.assertEqual(
            scenarios[0].result_dir,
            Path("s2n-quic/RandomLossRate/loss_01/packets_10000"),
        )
        self.assertEqual(
            scenarios[-1].result_dir,
            Path("s2n-quic/RandomLoss/packets_1000"),
        )


class PerfCommandTests(unittest.TestCase):
    def test_perf_stat_command_contains_events_output_and_benchmark_args(self) -> None:
        with (
            patch(
                "benchmark_tools.perf_commands.perf_command_prefix",
                return_value=["perf"],
            ),
            patch("benchmark_tools.perf_commands.run_command") as run_command,
            patch("benchmark_tools.perf_commands.restore_sudo_output_ownership"),
        ):
            metadata = run_perf_stat(
                Path("/tmp/resequencing-bench"),
                ["cycles", "cache-misses"],
                Path("/tmp/perf-output"),
                {"BENCH_FILTER": "quiche"},
                False,
                ["--bench", "quiche"],
            )

        self.assertEqual(metadata["perf_stat_path"], "/tmp/perf-output/perf_stat.txt")
        run_command.assert_called_once_with(
            [
                "perf",
                "stat",
                "-e",
                "cycles",
                "-e",
                "cache-misses",
                "-d",
                "-d",
                "-d",
                "-o",
                Path("/tmp/perf-output/perf_stat.txt"),
                Path("/tmp/resequencing-bench"),
                "--bench",
                "quiche",
            ],
            env={"BENCH_FILTER": "quiche"},
        )
