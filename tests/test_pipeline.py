from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from datetime import datetime
from pathlib import Path
from unittest.mock import patch

from benchmark_tools.pipeline import PerfScope, PipelineConfig
from benchmark_tools.pipeline_cli import parse_args
from benchmark_tools.publishing import format_run_directory_name
from benchmark_tools.toolchain import build_bench_binary


class PipelineConfigTests(unittest.TestCase):
    def test_namespace_is_normalized_into_typed_configuration(self) -> None:
        args = parse_args(
            [
                "--no-nix",
                "--perf",
                "stat",
                "--perf-by-scenario",
                "--scheme",
                "random_loss",
                "--implementation",
                "tcp",
                "--packet-count",
                "10000",
            ]
        )

        pipeline_config = PipelineConfig.from_namespace(args)

        self.assertIs(pipeline_config.perf_scope, PerfScope.SCENARIO)
        self.assertEqual(pipeline_config.schemes, ("RandomLoss",))
        self.assertEqual(pipeline_config.implementations, ("linux_tcp",))
        self.assertEqual(pipeline_config.packet_counts, (10_000,))
        self.assertTrue(pipeline_config.criterion_root.is_absolute())

    def test_perf_scope_flags_require_perf_mode(self) -> None:
        args = parse_args(["--no-nix", "--perf-by-scenario"])
        with self.assertRaisesRegex(RuntimeError, "require --perf"):
            PipelineConfig.from_namespace(args)

    def test_perf_counter_boxplots_select_counter_scope(self) -> None:
        args = parse_args(
            [
                "--no-nix",
                "--perf",
                "stat",
                "--perf-counter-boxplots",
                "--perf-counter-samples",
                "5",
            ]
        )

        pipeline_config = PipelineConfig.from_namespace(args)

        self.assertIs(pipeline_config.perf_scope, PerfScope.COUNTERS)
        self.assertEqual(pipeline_config.perf_counter_samples, 5)


class ToolchainTests(unittest.TestCase):
    def test_bench_binary_is_selected_from_cargo_json_output(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            executable = Path(temporary_directory) / "resequencing_bench"
            executable.touch()
            cargo_output = json.dumps(
                {
                    "reason": "compiler-artifact",
                    "target": {"name": "resequencing_bench", "kind": ["bench"]},
                    "executable": str(executable),
                }
            )
            completed = subprocess.CompletedProcess(
                args=[], returncode=0, stdout=cargo_output, stderr=None
            )
            with patch(
                "benchmark_tools.toolchain.run_command", return_value=completed
            ) as run_command:
                selected = build_bench_binary("resequencing_bench")

        self.assertEqual(selected, executable)
        run_command.assert_called_once_with(
            [
                "cargo",
                "bench",
                "--bench",
                "resequencing_bench",
                "--no-run",
                "--message-format=json",
            ],
            capture_stdout=True,
        )


class PublishingTests(unittest.TestCase):
    def test_run_directory_name_is_readable_and_filesystem_safe(self) -> None:
        generated_at = datetime(2026, 4, 21, 9, 36, 18)

        self.assertEqual(
            format_run_directory_name(generated_at),
            "2026-04-21_093618",
        )
