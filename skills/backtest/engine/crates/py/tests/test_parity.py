"""A strategy written with oxb.Strategy must reproduce the Rust reference backtest's hash,
and the report must describe the run.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import json

import pytest

import oxarchive_backtest as oxb
from refdata import CONFIG, MIN, T0, end, mean_cross, write


@pytest.fixture(scope="module")
def ref(tmp_path_factory):
    return write(tmp_path_factory.mktemp("src"), "REF", 10_000)


def test_python_strategy_matches_rust_exactly(ref):
    rust_fills, rust_hash = oxb.reference()
    fills = []
    r = oxb.backtest(mean_cross(fills=fills), T0, end(10_000), ref, CONFIG)
    assert (len(r.fills), r.result_hash) == (rust_fills, rust_hash)
    assert len(fills) == rust_fills, "on_fill saw every fill"


def test_report_shape(ref):
    r = oxb.backtest(mean_cross(), T0, end(500), ref, CONFIG)
    assert len(r.equity) == 500 and r.equity[0][0] == T0 + MIN
    s = r.stats
    assert s["start_equity"] == 10_000.0 and s["end_equity"] == r.final_equity
    assert s["fills"] == len(r.fills) and s["return"] == r.final_equity / 10_000.0 - 1
    assert {f.liquidity for f in r.fills} <= {"maker", "taker", "liquidation"}
    body = json.loads(r.receipt("src").json)
    assert body["window"] == [T0, end(500)]
    assert body["datasets"] == [{"market": "synthetic:reference", "month": "2026-10",
                                 "dataset_version": "lcg-10000"}]  # fmt: skip


def test_the_run_reads_only_what_the_strategy_needs(ref):
    s = mean_cross()
    assert s._check() == ({MIN: {"close": 30}}, [])
    assert "close" in s._columns() and "quote_volume" not in s._columns()
    # 30 bars plus 6 hours of carry, in minutes.
    assert s.warmup_ms == (30 + 360) * MIN
