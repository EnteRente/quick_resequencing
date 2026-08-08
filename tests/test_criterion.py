from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from benchmark_tools import config
from benchmark_tools.criterion import BenchmarkRecord, collect_records, parse_value_str
from benchmark_tools.plotting.scenarios import build_figure_specs


class CriterionTests(unittest.TestCase):
    def test_random_loss_rate_value_uses_loss_rate_as_x_value(self) -> None:
        self.assertEqual(
            parse_value_str("RandomLoss with 2.5% loss/10_000"),
            ("RandomLossRate", 10_000, "10_000", 2.5, "2.5%"),
        )

    def test_collect_records_prefers_new_criterion_run(self) -> None:
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            benchmark_root = root / "benchmark"
            for run_name, total_time in (("base", 2_000), ("new", 1_000)):
                run_dir = benchmark_root / run_name
                run_dir.mkdir(parents=True)
                (run_dir / "benchmark.json").write_text(
                    json.dumps(
                        {
                            "group_id": config.DEFAULT_BENCHMARK_GROUP,
                            "function_id": "linux-tcp",
                            "value_str": "RandomLoss/100",
                        }
                    ),
                    encoding="utf-8",
                )
                (run_dir / "sample.json").write_text(
                    json.dumps({"iters": [1], "times": [total_time]}),
                    encoding="utf-8",
                )

            records = collect_records(
                root,
                config.DEFAULT_BENCHMARK_GROUP,
                {"RandomLoss"},
                {"linux_tcp"},
                {100},
            )

        self.assertEqual(len(records), 1)
        self.assertEqual(records[0].implementation, "linux_tcp")
        self.assertEqual(records[0].samples_seconds, [1e-8])

    def test_random_loss_figure_specs_keep_expected_groups(self) -> None:
        records = [
            BenchmarkRecord(
                group=config.DEFAULT_BENCHMARK_GROUP,
                implementation=implementation,
                scenario="RandomLoss",
                size=100,
                size_label="100",
                x_value=100.0,
                x_label="100",
                samples_seconds=[1e-9],
            )
            for implementation in ("quic-go", "quiche", "s2n-quic", "linux_tcp")
        ]

        specs = build_figure_specs("RandomLoss", records)

        self.assertEqual(
            [spec.file_slug for spec in specs],
            [
                "random_loss",
                "random_loss_quic_go",
                "random_loss_quiche",
                "random_loss_linux_tcp_s2n_quic",
            ],
        )
