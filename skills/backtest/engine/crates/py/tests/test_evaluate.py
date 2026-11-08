"""Stats, attribution, benchmarks, nulls, out-of-sample tests and the deflated Sharpe."""

import math
from statistics import NormalDist

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from refdata import CONFIG, HOUR, T0, end, write

DAY = 86_400_000
N = 6 * 1440


@pytest.fixture
def two(tmp_path):
    write(tmp_path, "A", N, "hyperliquid:A", seed=1)
    write(tmp_path, "B", N, "hyperliquid:B", seed=2)
    return str(tmp_path)


def momentum(lookback=3):
    return oxb.Strategy(universe=["A", "B"], clock="4h",
                        signal=lambda h: h.close.ret(lookback),
                        weights=x.long_short(0.5))  # fmt: skip


def test_stats_exposure_and_attribution(two):
    r = oxb.backtest(momentum(), T0, end(N), two, CONFIG)
    s = r.stats
    for k in ("annual_return", "volatility", "sortino", "calmar", "hit_rate", "turnover",
              "exposure", "net_exposure", "win_rate", "trades"):  # fmt: skip
        assert k in s, k
    # Two markets at quantile 0.5: the lower one is in both legs, so the book is
    # +0.25 / -0.25 (the usual construction).
    assert 0.45 < s["max_exposure"] < 0.6 and abs(s["net_exposure"]) < 0.05
    total = sum(a["pnl"] for a in r.attribution.values())
    assert total == pytest.approx(r.final_equity - 10_000.0, abs=1e-6)
    assert sum(a["fees"] for a in r.attribution.values()) == pytest.approx(s["fees"])
    assert len(r.daily_returns) >= 4
    assert oxb._native._stats(r.equity)["sharpe"] == s["sharpe"]


def test_benchmarks(two):
    r = oxb.backtest(momentum(), T0, end(N), two, CONFIG, benchmark="A")
    assert set(r.benchmark.final_positions) == {"A"}
    vs = r.vs
    assert vs["excess_return"] == pytest.approx(r.stats["return"] - r.benchmark.stats["return"])
    assert -1 <= vs["correlation"] <= 1
    eq = oxb.backtest(momentum(), T0, end(N), two, CONFIG, benchmark="equal")
    assert set(eq.benchmark.final_positions) == {"A", "B"}
    assert all(p > 0 for p in eq.benchmark.final_positions.values())
    assert oxb.backtest(momentum(), T0, end(N), two, CONFIG).vs is None


def test_replace_and_the_null_strategies(two):
    s = momentum()
    assert s.replace(clock="1d").clock.every_ms == DAY and s.clock.every_ms == 4 * HOUR
    with pytest.raises(TypeError, match="no argument"):
        s.replace(colck="1d")
    run = lambda v: oxb.backtest(v, T0, end(N), two, CONFIG)
    base, flip = run(s), run(s.flipped())
    # Long-short on two markets: flipping the signal swaps the legs.
    first = lambda r: {f.market: f.size > 0 for f in r.fills[:2]}
    assert first(base) == {m: not v for m, v in first(flip).items()}
    a, b, c = run(s.random_signal(1)), run(s.random_signal(1)), run(s.random_signal(2))
    assert a.result_hash == b.result_hash != c.result_hash
    stale = s.stale_signal(0, within="1d")
    assert stale.warmup_ms > s.warmup_ms
    assert run(stale).result_hash != base.result_hash
    with pytest.raises(ValueError, match="signal"):
        oxb.Strategy(universe=["A"], step=lambda c, h: None).flipped()


def test_null_test_split_and_walk_forward(two):
    out = oxb.null_test(momentum(), T0, end(N), two, CONFIG, seeds=3)
    assert len(out["random"]) == 3 and len(out["stale"]) == 3
    assert out["flipped"] is not None and 0 <= out["random_share_above"] <= 1
    grid = {"lookback": [2, 6]}
    sp = oxb.split(momentum, grid, T0, end(N), at=T0 + 3 * DAY, source=two, config=CONFIG)
    assert sp["params"] in ({"lookback": 2}, {"lookback": 6}) and len(sp["trials"]) == 2
    assert sp["report"].stats["start"] == T0 + 3 * DAY
    wf = oxb.walk_forward(momentum, grid, T0, end(N), train="2d", test="1d", source=two,
                          config=CONFIG)  # fmt: skip
    assert [f["test"][0] for f in wf["folds"]] == [T0 + 2 * DAY, T0 + 3 * DAY, T0 + 4 * DAY,
                                                    T0 + 5 * DAY]  # fmt: skip
    eq = wf["equity"]
    assert eq[0][0] == T0 + 2 * DAY and eq[-1][0] == end(N)
    assert wf["stats"]["return"] == pytest.approx(eq[-1][1] / eq[0][1] - 1)


def test_deflated_sharpe_follows_bailey_and_lopez_de_prado():
    # Daily returns with a small edge, and 50 trials whose Sharpes vary.
    rets = [0.01 * math.sin(i * 0.7) + 0.0006 for i in range(400)]
    trials = [0.5 + 0.04 * i for i in range(50)]
    out = oxb.deflated_sharpe(rets, trials)
    # The formula by hand, with the standard library's normal distribution.
    nd = NormalDist()
    n = len(rets)
    m = sum(rets) / n
    m2 = sum((v - m) ** 2 for v in rets) / n
    sr = m / math.sqrt(m2 * n / (n - 1))
    skew = sum((v - m) ** 3 for v in rets) / n / m2**1.5
    kurt = sum((v - m) ** 4 for v in rets) / n / m2**2
    srs = [t / math.sqrt(365) for t in trials]
    mu = sum(srs) / 50
    v = sum((t - mu) ** 2 for t in srs) / 49
    g = 0.5772156649015329
    sr0 = math.sqrt(v) * ((1 - g) * nd.inv_cdf(1 - 1 / 50) + g * nd.inv_cdf(1 - 1 / (50 * math.e)))
    want = nd.cdf((sr - sr0) * math.sqrt(n - 1) / math.sqrt(1 - skew * sr + (kurt - 1) / 4 * sr**2))
    assert out["deflated_sharpe"] == pytest.approx(want, rel=1e-9)
    assert out["threshold"] == pytest.approx(sr0 * math.sqrt(365), rel=1e-9)
    assert out["probabilistic_sharpe"] > out["deflated_sharpe"]


def test_shifted_clocks():
    e = x.shifted(x.every("2d", anchor="2025-06-01"), 8 * HOUR)
    assert e.every_ms == 2 * DAY and e.anchor_ms == x.every("2d", anchor="2025-06-01 08:00").anchor_ms
    d = x.shifted(x.daily("20:00"), 8 * HOUR)  # wraps past midnight
    assert d._spec() == x.daily("04:00")._spec()
    w = x.shifted(x.weekly("sun", "20:00"), 8 * HOUR)  # Sunday 20:00 + 8 h is Monday 04:00
    assert w._spec() == x.weekly("mon", "04:00")._spec()
    m = x.shifted(x.monthly(27), 2 * DAY)  # monthly days wrap inside 1 to 28
    assert m._spec() == x.monthly(1)._spec()
    assert x.shifted(x.monthly(-1), DAY) is None and x.shifted(x.at_open("us"), HOUR) is None
    assert x.period_ms(x.every("2d")) == 2 * DAY and x.period_ms(x.weekly()) == 7 * DAY
    assert x.period_ms(x.at_open("us")) is None
    s = momentum()
    assert s.shifted("1h").clock.anchor_ms == s.clock.anchor_ms + HOUR
    assert s.shifted(0).clock._spec() == s.clock._spec()
    with pytest.raises(ValueError, match="can't be moved"):
        s.replace(clock=x.at_open("us")).shifted("1h")


def test_phase_test(two):
    s = momentum()  # a 4 h clock: six phases 40 minutes apart
    out = oxb.phase_test(s, T0, end(N), two, CONFIG)
    rows = out["phases"]
    assert out["period_ms"] == 4 * HOUR and out["step_ms"] == 40 * 60_000
    assert [r["offset_ms"] for r in rows] == [k * 40 * 60_000 for k in range(6)]
    run = lambda v: oxb.backtest(v, T0, end(N), two, CONFIG).stats
    assert rows[0]["sharpe"] == run(s)["sharpe"]
    assert rows[3]["return"] == run(s.shifted(120 * 60_000))["return"]
    assert out["losing"] == sum(1 for r in rows if r["sharpe"] <= 0)
    assert out["return_min"] <= rows[0]["return"] <= out["return_max"]
    with pytest.raises(ValueError, match="no 6 phases"):
        oxb.phase_test(s.replace(clock="1m"), T0, end(N), two, CONFIG)
