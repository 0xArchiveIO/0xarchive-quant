"""HIP-3 markets: named dex:COIN, stored one folder deeper (xyz/CL/), traded beside core."""

import os

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from oxarchive_backtest.run import market_dir
from refdata import T0, end, write


def test_a_dex_folder_lists_its_markets_as_dex_coin(tmp_path):
    write(tmp_path, "BTC", 120)
    write(tmp_path, os.path.join("xyz", "CL"), 120)
    write(tmp_path, "kPEPE", 120)
    assert oxb.Source(str(tmp_path)).markets() == ["BTC", "kPEPE", "xyz:CL"]
    assert market_dir("data", "xyz:CL") == os.path.join("data", "xyz", "CL")
    assert market_dir("data", "BTC") == os.path.join("data", "BTC")


def test_a_hip3_market_trades_beside_a_core_one(tmp_path):
    n = 3 * 1440
    root = write(tmp_path, "BTC", n, seed=1)
    write(tmp_path, os.path.join("xyz", "CL"), n, seed=2)
    s = oxb.Strategy(
        universe=["BTC", "xyz:CL"],
        clock="1h",
        signal=lambda h: Series({m: 1.0 for m in h.markets}),
        weights=x.long_only(top=2),
    )
    r = oxb.backtest(s, T0, end(n), root)
    assert set(r.final_positions) == {"BTC", "xyz:CL"}
    assert all(v > 0 for v in r.final_positions.values())
