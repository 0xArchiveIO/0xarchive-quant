"""examples/momentum.py, the demo's default, gives the result hash of the same strategy
written by hand on the old per-minute API (0ed429d550204eda, 1,110 fills): the pieces, the
history reads, the universe and the run loop together reproduce it bit for bit.

Needs the full 62-market dataset in data/hyperliquid (scripts/backfill-launch-set.sh);
skipped without it. About 11 s.
"""

import pathlib

import pytest

import oxarchive_backtest as oxb

ROOT = pathlib.Path(__file__).resolve().parents[3]
DATA = ROOT / "data" / "hyperliquid"


@pytest.mark.skipif(not (DATA / "ZEC").is_dir(), reason="needs the dataset in data/hyperliquid")
def test_pieces_reproduce_the_hand_written_momentum():
    module = oxb.load_strategy(ROOT / "examples" / "momentum.py", name="example_momentum")
    r = oxb.backtest(module.strategy, "2025-03-01", "2026-10-01", str(DATA),
                     oxb.Config(starting_cash=100_000.0, builder_fee_bps=1.0))  # fmt: skip
    assert (r.result_hash, len(r.fills), len(r.final_positions)) == ("0ed429d550204eda", 1110, 62)
