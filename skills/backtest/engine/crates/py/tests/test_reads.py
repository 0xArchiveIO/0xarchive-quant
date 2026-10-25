"""Several bar sizes, log returns, EWMA, beta and residuals, repair, and the cross-sectional
pieces (demean, neutralize, blend)."""

import math

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from refdata import HOUR, MIN, T0, end, write

DAY = 86_400_000


def collect(root, n, read, universe=("A",), **kw):
    got = []

    def step(ctx, h):
        v = read(h)
        if ctx.time:
            got.append((ctx.time, v))

    s = oxb.Strategy(universe=list(universe), clock=kw.pop("clock", "1h"), step=step, **kw)
    oxb.backtest(s, T0, end(n), root)
    return got, s


def test_daily_bars_beside_hourly(tmp_path):
    root = write(tmp_path, "A", 4 * 1440)

    def read(h):
        d = h.bars("1d")
        hours = max(1, (h.time % DAY) // HOUR)
        return d.close.last()["A"], h.close.ago(hours)["A"], d.time, h.time

    got, s = collect(root, 4 * 1440, read, history="1d")
    # Days start at 00:00 UTC; the first tick that has a whole day closed is 2026-10-06 01:00.
    full = [(t, v) for t, v in got if v[0] is not None]
    assert full and all(daily == hourly for _, (daily, hourly, _, t) in full if t % DAY)
    assert all(dt == t - t % DAY for t, (_, _, dt, _) in full)
    # Warm-up starts on a day boundary, and the run keeps both bar sizes.
    assert [h["bar_ms"] for h in s._histories()] == [HOUR, DAY]
    assert s.warm_from(T0) % DAY == 0


def test_log_returns_and_ewma(tmp_path):
    root = write(tmp_path, "A", 2000)

    def read(h):
        w = h.close.window(21)["A"]
        return h.close.logret(20)["A"], h.close.ewma(4, window=20)["A"], w

    got, _ = collect(root, 2000, read)
    lr, ew, w = got[-1][1]
    assert lr == pytest.approx(math.log(w[-1] / w[0]), rel=1e-15)
    weights = [0.5 ** (k / 4) for k in range(20)][::-1]
    want = sum(wt * v for wt, v in zip(weights, w[1:])) / sum(weights)
    assert ew == pytest.approx(want, rel=1e-12)


def test_beta_and_residuals_against_a_market_outside_the_universe(tmp_path):
    write(tmp_path, "A", 3000, seed=1)
    write(tmp_path, "B", 3000, seed=2)
    root = str(tmp_path)

    def read(h):
        a, b = h.close.window(25)["A"], None
        return (h.close.beta(24, to="B")["A"], h.close.corr(24, to="B")["A"],
                h.close.resid(6, to="B", fit=18)["A"], a)  # fmt: skip

    got, s = collect(root, 3000, read)
    assert s.references == ["B"]
    beta, corr, resid, _ = got[-1][1]
    assert beta is not None and -1 <= corr <= 1 and resid is not None
    # The same reads by hand from both markets' windows.
    seen = {}

    def both(ctx, h):
        w = h.close.window(25)
        if ctx.time:
            seen["w"] = w

    oxb.backtest(oxb.Strategy(universe=["A", "B"], clock="1h", step=both), T0, end(3000), root)
    rets = {m: [w[i + 1] / w[i] - 1 for i in range(24)] for m, w in seen["w"].items()}
    ra, rb = rets["A"], rets["B"]
    mean = lambda xs: sum(xs) / len(xs)
    cov = mean([p * q for p, q in zip(ra, rb)]) - mean(ra) * mean(rb)
    var = mean([q * q for q in rb]) - mean(rb) ** 2
    assert beta == pytest.approx(cov / var, rel=1e-9)
    with pytest.raises(ValueError, match="feed it"):
        oxb.Session(s, markets=["A"])


def test_repair_is_opt_in_and_warms_up_its_window(tmp_path):
    root = write(tmp_path, "A", 3000)
    plain = oxb.Strategy(universe=["A"], weights=lambda s, h: Series({"A": 0.5}))
    fixed = oxb.Strategy(universe=["A"], weights=lambda s, h: Series({"A": 0.5}), repair=True)
    assert fixed.repair == 2.5 and fixed.warmup_ms == plain.warmup_ms + 48 * HOUR
    a = oxb.backtest(plain, T0 + 3 * DAY // 2, end(3000), root)
    b = oxb.backtest(fixed, T0 + 3 * DAY // 2, end(3000), root)
    assert a.result_hash == b.result_hash  # the reference rows have no bad prints
    with pytest.raises(ValueError, match="ratio"):
        oxb.Strategy(universe=["A"], weights=x.sign(), repair=0.5)


def test_demean_neutralize_and_blend():
    s = Series({"A": 1.0, "B": 3.0, "C": None, "D": 8.0})
    assert s.demean().to_dict() == {"A": -3.0, "B": -1.0, "C": None, "D": 4.0}
    by = {"A": "l1", "B": "l1", "D": "l2"}
    assert s.demean(by=by).to_dict() == {"A": -1.0, "B": 1.0, "C": None, "D": 0.0}
    # A signal that is 2 x beta + noise: neutralising leaves the noise, centred.
    beta = Series({m: float(i) for i, m in enumerate("ABCDEF")})
    noise = Series({"A": 0.1, "B": -0.2, "C": 0.3, "D": -0.1, "E": 0.0, "F": -0.1})
    out = (beta * 2 + noise).neutralize(beta)
    assert out.sum() == pytest.approx(0.0, abs=1e-12)
    cov = sum(out[m] * (beta[m] - 2.5) for m in "ABCDEF")
    assert cov == pytest.approx(0.0, abs=1e-12)
    # The rank blend: the mean of the two pct ranks.
    a = Series({"A": 1.0, "B": 2.0, "C": 3.0, "D": None})
    b = Series({"A": 3.0, "B": 2.0, "C": 1.0, "D": 5.0})
    got = x.blend(a, b).to_dict()
    assert got["D"] is None
    assert [got[m] for m in "ABC"] == pytest.approx([(1 / 3 + 0.75) / 2, (2 / 3 + 0.5) / 2,
                                                     (1 + 0.25) / 2])  # fmt: skip
    assert Series({"A": math.e, "B": -1.0}).log().to_dict() == {"A": 1.0, "B": None}
