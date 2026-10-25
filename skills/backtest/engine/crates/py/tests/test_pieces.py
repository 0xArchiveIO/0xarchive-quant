"""The history reads, Series and the pieces strategies are built from.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from refdata import CONFIG, MIN, T0, end, write


def test_series_arithmetic_keeps_none_and_order():
    a = Series({"X": 1.0, "Y": None, "Z": 4.0})
    b = Series({"Z": 2.0, "X": 0.0})
    assert (a / b).to_dict() == {"X": None, "Y": None, "Z": 2.0}
    assert (a * 2 + 1).to_dict() == {"X": 3.0, "Y": None, "Z": 9.0}
    assert (1 / a).to_dict() == {"X": 1.0, "Y": None, "Z": 0.25}
    assert list(-a) == ["X", "Y", "Z"]
    assert (a > 2).to_dict() == {"X": False, "Y": None, "Z": True}
    assert a.where(a > 2).dropna().to_dict() == {"Z": 4.0}
    assert a.sum() == 5.0 and a.count() == 2 and a.mean() == 2.5


def test_rank_is_pandas_pct_rank_and_quintiles_cut_like_signal_lab():
    s = Series({m: float(v) for m, v in zip("ABCDE", [3, 1, 2, 2, 5])})
    assert s.rank().to_dict() == {"A": 0.8, "B": 0.2, "C": 0.5, "D": 0.5, "E": 1.0}
    scores = Series({f"M{i:02d}": float(i) for i in range(25)})
    w = x.long_short(0.2)(scores, None)
    longs = [m for m, v in w.items() if v > 0]
    shorts = [m for m, v in w.items() if v < 0]
    assert (len(longs), len(shorts)) == (6, 5)
    assert w.sum() == pytest.approx(0.0) and abs(w).sum() == pytest.approx(1.0)
    assert x.long_short(0.2)(Series({"A": 1.0}), None) is None


def test_weighting_and_risk_pieces():
    scores = Series({"A": 3.0, "B": -1.0, "C": None, "D": 2.0})
    by = Series({"A": 1.0, "B": 1.0, "D": 3.0})
    w = x.long_only(top=2, by=lambda h: by)(scores, None)
    assert w.to_dict() == {"A": 0.25, "D": 0.75}
    assert x.sign(short=False)(scores, None).to_dict() == {"A": 1.0, "B": 0.0, "C": 0.0, "D": 1.0}
    assert x.cap(0.5)(Series({"A": 0.9, "B": -0.7}), None, None).to_dict() == {"A": 0.5, "B": -0.5}
    assert x.gross(1.0)(Series({"A": 1.5, "B": -0.5}), None, None).to_dict() == {"A": 0.75, "B": -0.25}


def test_history_reads_hourly_bars_and_durations(tmp_path):
    root = write(tmp_path, "A", 600)
    got = {}

    def step(ctx, h):
        seen = (h.close.last()["A"], h.close.ret("2h")["A"], h.close.ago(1)["A"],
                h.high.max(3)["A"], h.time)  # fmt: skip
        if ctx.time:  # not the check before the run, whose time is 0
            got[ctx.time] = seen

    s = oxb.Strategy(universe=["A"], clock="1h", step=step)
    oxb.backtest(s, T0, end(600), root)
    ticks = sorted(got)
    assert ticks[0] == T0 + 3_600_000  # the first hour's close
    last, ret2, ago1, _, t = got[ticks[3]]
    assert t == ticks[3] and ret2 is not None and ago1 is not None
    assert ret2 == last / got[ticks[1]][0] - 1
    assert got[ticks[0]][1] is None  # two hours back from the first bar: not yet
    with pytest.raises(ValueError, match="whole number"):
        s2 = oxb.Strategy(universe=["A"], clock="1h", step=lambda ctx, h: h.close.ret("90m"))
        oxb.backtest(s2, T0, end(100), root)


def test_rebalance_trades_to_weights_and_flattens_the_rest(tmp_path):
    write(tmp_path, "A", 300, "hyperliquid:BTC")
    write(tmp_path, "B", 300, "hyperliquid:ETH", 0.5)
    def weights(scores, h):
        odd = (h.time // 3_600_000) % 2
        return Series({"A": 0.5} if odd else {"B": -0.25})

    s = oxb.Strategy(universe=["A", "B"], clock="1h", weights=weights)
    r = oxb.backtest(s, T0, end(300), str(tmp_path), CONFIG)
    # Ticks at 15:00 (odd hour: A), 16:00 (B, and A closed), and so on.
    first = [f for f in r.fills if f.ts == T0 + MIN * 60]
    assert [f.market for f in first] == ["A"]
    eq = 10_000.0
    assert first[0].size == pytest.approx(0.5 * eq / 100.0, rel=0.02)
    second = [f.market for f in r.fills if f.ts == T0 + MIN * 120]
    assert second == ["A", "B"]


def test_strategy_arguments_are_checked():
    with pytest.raises(ValueError, match="weights="):
        oxb.Strategy(universe=["A"])
    with pytest.raises(ValueError, match="replaces"):
        oxb.Strategy(universe=["A"], step=lambda c, h: None, weights=x.sign())
    with pytest.raises(ValueError, match="list of market names"):
        oxb.Strategy(universe=5, weights=x.sign())
    with pytest.raises(ValueError, match="duration"):
        oxb.Strategy(universe=["A"], weights=x.sign(), clock="fortnightly")
    with pytest.raises(ValueError, match="fields"):
        oxb.Strategy(universe=["A"], weights=x.sign(), fields=["closee"])
    s = oxb.Strategy(universe="A", weights=lambda s: None)
    assert s.universe == ["A"] and s.clock.every_ms == 3_600_000
