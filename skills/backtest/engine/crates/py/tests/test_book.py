"""Book shape, hedges, volatility targeting, caps, turnover, bands and costs, exits, the
drawdown stop and universe membership events."""

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from refdata import CONFIG, HOUR, MIN, T0, end, write

DAY = 86_400_000


def test_book_shape_and_caps():
    w = Series({"A": 0.6, "B": 0.2, "C": -0.2})
    out = x.book(gross=1.0, net=0.0)(w, None, None)
    assert out.to_dict() == pytest.approx({"A": 0.375, "B": 0.125, "C": -0.5})
    assert x.book(net=0.2)(w, None, None).to_dict() == pytest.approx({"A": 0.45, "B": 0.15, "C": -0.4})
    # Each market to 0.5, then the long leg down to a net of 0.2.
    capped = x.cap(0.5, net=0.2)(Series({"A": 0.9, "B": 0.3, "C": -0.1}), None, None)
    assert capped.to_dict() == pytest.approx({"A": 0.1875, "B": 0.1125, "C": -0.1})
    assert abs(x.cap(gross=0.3)(capped, None, None)).sum() == pytest.approx(0.3)
    assert x.cap(gross=5)(capped, None, None).to_dict() == capped.to_dict()  # only down
    with pytest.raises(ValueError, match="gross"):
        x.book()


def test_hedge_and_beta_neutral_against_a_market_outside_the_universe(tmp_path):
    # B is A at half the price: the same returns, so a beta of exactly 1.
    write(tmp_path, "A", 3000, "hyperliquid:BTC")
    write(tmp_path, "B", 3000, "hyperliquid:BTC", scale=0.5)
    root = str(tmp_path)
    s = oxb.Strategy(universe=["A"], weights=lambda s, h: Series({"A": 0.5}),
                     risk=x.hedge("B", n=24))  # fmt: skip
    r = oxb.backtest(s, T0 + DAY, end(3000), root, CONFIG)
    a, b = r.final_positions["A"], r.final_positions["B"]
    assert a > 0 and b == pytest.approx(-2 * a, rel=0.01)  # equal and opposite notional
    w = Series({"A": 0.5, "B": -0.2})

    class H:
        class close:
            @staticmethod
            def beta(n, to):
                return Series({"A": 2.0, "B": 0.5})

    assert x.beta_neutral(to="X")(w, H, None).to_dict() == pytest.approx({"A": 0.5, "B": -2.0})


def test_vol_target_scales_to_the_books_own_volatility(tmp_path):
    root = write(tmp_path, "A", 3000)
    got = {}

    def weights(scores, h):
        c = h.close.window(48)["A"]
        if h.time:
            got["c"] = c
        return Series({"A": 1.0})

    def record(w, h, ctx):
        got["w"] = w["A"]
        return w

    s = oxb.Strategy(universe=["A"], weights=weights, risk=[x.vol_target(0.2, n=48), record])
    oxb.backtest(s, T0 + DAY, end(3000), root)
    c = got["c"]
    rets = [c[i + 1] / c[i] - 1 for i in range(47)]
    m = sum(rets) / 47
    sd = (sum((r - m) ** 2 for r in rets) / 46) ** 0.5
    assert got["w"] == pytest.approx(min(3.0, 0.2 / (sd * (365 * 24) ** 0.5)), rel=1e-9)


def test_turnover_bands_and_costs(tmp_path):
    root = write(tmp_path, "A", 600)
    w = lambda s, h: Series({"A": 1.0})
    limited = oxb.Strategy(universe=["A"], weights=w, risk=x.turnover(0.25))
    r = oxb.backtest(limited, T0, end(600), root, CONFIG)
    first = r.fills[0]
    assert abs(first.size * first.price) == pytest.approx(0.25 * 10_000, rel=0.01)
    assert len(r.fills) >= 4  # a quarter of the way each hour
    # Banded: after the first trade the drift stays inside 50%, so nothing more trades.
    banded = oxb.Strategy(universe=["A"], weights=w, execute=x.rebalance(band=0.5))
    assert len(oxb.backtest(banded, T0, end(600), root, CONFIG).fills) == 1
    # Any cost at all is too much: nothing trades.
    free = oxb.Strategy(universe=["A"], weights=w, execute=x.rebalance(max_cost=0.0))
    assert oxb.backtest(free, T0, end(600), root, CONFIG).fills == []
    seen = {}

    def step(ctx, h):
        if ctx.time:
            seen["cost"] = ctx.cost({"A": 1.0})

    oxb.backtest(oxb.Strategy(universe=["A"], step=step), T0, end(120), root, CONFIG)
    # 10k notional: 4.5 bps taker and 1 bps builder fee, plus slippage.
    assert 10_000 * 5.5e-4 < seen["cost"] < 10_000 * 20e-4


def test_exits_close_positions_between_ticks(tmp_path):
    root = write(tmp_path, "A", 3000)
    w = lambda s, h: Series({"A": 1.0})
    tight = oxb.Strategy(universe=["A"], clock="1d", weights=w, exits=[x.stop_loss(0.002),
                         x.take_profit(0.002)])  # fmt: skip
    r = oxb.backtest(tight, T0, end(3000), root, CONFIG)
    assert r.exits and {why for _, _, why in r.exits} <= {"stop_loss", "take_profit"}
    t, market, _ = r.exits[0]
    assert market == "A" and t % DAY != 0, "between the daily ticks"
    close = [f for f in r.fills if f.ts == t]
    assert close and close[0].size < 0
    timed = oxb.Strategy(universe=["A"], clock="1d", weights=w, exits=[x.time_stop("2h")])
    r = oxb.backtest(timed, T0, end(3000), root, CONFIG)
    entry = r.fills[0].ts
    assert r.exits[0][0] == entry + 2 * HOUR and r.exits[0][2] == "time_stop"
    with pytest.raises(ValueError, match="not an exit"):
        oxb.Strategy(universe=["A"], weights=w, exits=[0.05])


def test_drawdown_stop_halts_the_strategy(tmp_path):
    root = write(tmp_path, "A", 3000)
    lev = lambda s, h: Series({"A": 20.0 if (h.time // HOUR) % 2 else -20.0})
    s = oxb.Strategy(universe=["A"], weights=lev, exits=[x.drawdown_stop(0.02)])
    r = oxb.backtest(s, T0, end(3000), root, CONFIG)
    (t, market, why), = r.exits
    assert market is None and why == "drawdown_stop"
    assert all(f.ts <= t for f in r.fills)  # the closing fills, then nothing
    assert all(p == 0 for p in r.final_positions.values())


def test_on_universe_hears_members_join_and_leave(tmp_path):
    for m in ("A", "B"):
        write(tmp_path, m, 4000, f"hyperliquid:{m}")
    day = lambda view: ["A"] if (view.at // DAY) % 2 else ["B"]
    heard = []
    s = oxb.Strategy(universe=x.rule(day, every="1d", anchor=T0), clock="1d",
                     weights=lambda s, h: Series({m: 0.5 for m in h.markets}),
                     on_universe=lambda ctx, joined, left: heard.append((ctx.time, joined, left)))  # fmt: skip
    oxb.backtest(s, T0, end(4000), str(tmp_path), CONFIG)
    first = (T0 // DAY) % 2
    assert heard[0] == (T0 + MIN, ["A"] if first else ["B"], [])
    assert heard[1] == (T0 + DAY, ["B"] if first else ["A"], heard[0][1])
