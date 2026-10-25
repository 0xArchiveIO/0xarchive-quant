"""Is it real? Benchmarks, nulls, out-of-sample tests and a deflated Sharpe.

    r = oxb.backtest(s, start, end, benchmark="BTC")      # r.benchmark, r.vs
    oxb.null_test(s, start, end)                          # flipped and random-signal nulls
    oxb.split(make, grid, start, end, at="2026-01-01")    # pick in sample, test after
    oxb.walk_forward(make, grid, start, end, train="180d", test="30d")
    oxb.deflated_sharpe(report, trials)                   # after trying many settings
    oxb.gauntlet(make, grid, start, end)                  # all of it, and a verdict

Everything here is ordinary backtests: the same engine, the same receipts. The backtests a
check runs for itself are recorded in an active study's ledger as checks, not trials
(study.py).
"""

import math as _m

from . import _native
from . import math as _math
from . import study as _study
from .run import _strategy, backtest, sweep
from .series import Series
from .strategy import Strategy, duration_ms, when

_EULER = 0.5772156649015329
_SQRT2 = 1.4142135623730951
_SQRT_PI = 1.7724538509055159


def benchmark_of(kind, strategy=None):
    """A benchmark strategy: a market name (hold it, fully invested), "equal" (the
    strategy's own universe at equal weight, rebalanced on its clock), or a Strategy."""
    from . import x

    if isinstance(kind, Strategy):
        return kind
    if kind == "equal":
        if strategy is None:
            raise ValueError('"equal" is the strategy\'s own universe; pass the strategy')
        return Strategy(universe=strategy.universe, clock=strategy.clock,
                        weights=lambda s, h: Series({m: 1.0 / len(h.markets) for m in h.markets}))  # fmt: skip
    if isinstance(kind, str):
        return Strategy(universe=[kind], clock="1d", weights=lambda s, h: Series({kind: 1.0}),
                        execute=x.rebalance(band=0.01))  # fmt: skip
    raise TypeError("benchmark is a market name, 'equal' or a Strategy")


def _score(row, metric):
    v = row.get(metric)
    return float("-inf") if v is None else v


def split(make, params, start, end, at, source="data/hyperliquid", config=None, *,
          metric="sharpe", max_leverage=None, base=None):  # fmt: skip
    """Picks the best point of the grid `params` by `metric` in sample, [start, at), then
    runs it out of sample, [at, end). Returns {params, in_sample, out_of_sample (stats),
    report (the out-of-sample run), trials (every in-sample point)}."""
    with _study.kind("check"):
        return _split(make, params, start, end, at, source, config, metric=metric,
                      max_leverage=max_leverage, base=base)  # fmt: skip


def _split(make, params, start, end, at, source, config, *, metric, max_leverage, base):
    trials = sweep(make, start, at, params, source, config, max_leverage=max_leverage, base=base)
    best = max(trials, key=lambda r: _score(r, metric))
    p = {**(base or {}), **best["params"]}
    oos = backtest(make, at, end, source, config, max_leverage=max_leverage, params=p)
    return {"params": best["params"], "in_sample": best, "out_of_sample": oos.stats,
            "report": oos, "trials": trials}  # fmt: skip


def walk_forward(make, params, start, end, train="180d", test="30d",
                 source="data/hyperliquid", config=None, *, metric="sharpe", anchored=False,
                 max_leverage=None, base=None):  # fmt: skip
    """Walk-forward: for each `test` window from start + train to end, picks the best grid
    point by `metric` on the `train` before it (from `start` throughout, if anchored),
    then runs it on the test window. Returns {folds: [{train, test, params, stats}],
    equity: the test windows' curves chained into one, stats: that curve's stats}."""
    with _study.kind("check"):
        return _walk_forward(make, params, start, end, train, test, source, config,
                             metric=metric, anchored=anchored, max_leverage=max_leverage,
                             base=base)  # fmt: skip


def _walk_forward(make, params, start, end, train, test, source, config, *, metric, anchored,
                  max_leverage, base):  # fmt: skip
    t0, t1 = when(start), when(end)
    tr, te = duration_ms(train), duration_ms(test)
    folds, curve = [], []
    t = t0 + tr
    while t < t1:
        f0 = t0 if anchored else t - tr
        f1 = min(t + te, t1)
        trials = sweep(make, f0, t, params, source, config, max_leverage=max_leverage,
                       base=base)  # fmt: skip
        best = max(trials, key=lambda r: _score(r, metric))
        p = {**(base or {}), **best["params"]}
        r = backtest(make, t, f1, source, config, max_leverage=max_leverage, params=p)
        eq = r.equity
        if eq:
            # Chain: this window starts where the last one ended.
            k = curve[-1][1] / eq[0][1] if curve else 1.0
            curve.extend((ts, e * k) for ts, e in eq)
        folds.append({"train": (f0, t), "test": (t, f1), "params": best["params"],
                      "in_sample": best.get(metric), "stats": r.stats})  # fmt: skip
        t = f1
    return {"folds": folds, "equity": curve, "stats": _native._stats(curve)}


def null_test(strategy, start, end, source="data/hyperliquid", config=None, *, seeds=20,
              stale=True, every=None, max_leverage=None, params=None):  # fmt: skip
    """The strategy against its nulls over [start, end): flipped (the negated signal),
    `seeds` random signals (our factor research's construction-only null) and, with `stale`, as
    many runs on its own signal at random earlier times. Returns the Sharpes, and the share
    of each null at or above the strategy's: small shares mean the signal, not the
    machinery or luck, made the result. `every` redraws the random and stale nulls only
    once per that duration (Strategy.random_signal), for a strategy that ticks more often
    than it trades."""
    with _study.kind("check"):
        return _null_test(strategy, start, end, source, config, seeds=seeds, stale=stale,
                          every=every, max_leverage=max_leverage, params=params)  # fmt: skip


def _null_test(strategy, start, end, source, config, *, seeds, stale, every, max_leverage,
               params):  # fmt: skip
    s = _strategy(strategy, params)
    run = lambda v: backtest(v, start, end, source, config, max_leverage=max_leverage).stats
    sharpe = lambda st: (st or {}).get("sharpe")
    actual = sharpe(run(s))
    out = {"sharpe": actual, "flipped": sharpe(run(s.flipped()))}
    kinds = [("random", s.random_signal)] + ([("stale", s.stale_signal)] if stale else [])
    for name, make in kinds:
        xs = [sharpe(run(make(seed, every=every))) for seed in range(seeds)]
        known = [v for v in xs if v is not None]
        series = Series(dict(enumerate(known)))
        out[name] = xs
        out[f"{name}_mean"] = series.mean()
        out[f"{name}_sd"] = series.std()
        out[f"{name}_share_above"] = (None if actual is None or not known
                                      else sum(1 for v in known if v >= actual) / len(known))  # fmt: skip
    return out


# The standard normal, deterministically: series and continued fractions on + - * / and
# the package's exp and log (oxarchive_backtest.math), never the platform's erf.


def _erfc(z):
    """erfc(z) for z >= 0."""
    if z < 2.0:
        # erf by its Taylor series.
        term, total, n = z, z, 0
        while abs(term) > 1e-17 * abs(total):
            n += 1
            term *= -z * z / n
            total += term / (2 * n + 1)
        return 1.0 - 2.0 / _SQRT_PI * total
    k = z
    for i in range(120, 0, -1):
        k = z + (i / 2.0) / k
    return _math.exp(-z * z) / (_SQRT_PI * k)


def norm_cdf(x):
    """The standard normal CDF."""
    return 0.5 * _erfc(-x / _SQRT2) if x < 0 else 1.0 - 0.5 * _erfc(x / _SQRT2)


def norm_ppf(p):
    """The standard normal quantile: Acklam's approximation, then one Halley step."""
    if not 0.0 < p < 1.0:
        raise ValueError("p is between 0 and 1")
    a = (-3.969683028665376e01, 2.209460984245205e02, -2.759285104469687e02,
         1.383577518672690e02, -3.066479806614716e01, 2.506628277459239e00)  # fmt: skip
    b = (-5.447609879822406e01, 1.615858368580409e02, -1.556989798598866e02,
         6.680131188771972e01, -1.328068155288572e01)  # fmt: skip
    c = (-7.784894002430293e-03, -3.223964580411365e-01, -2.400758277161838e00,
         -2.549732539343734e00, 4.374664141464968e00, 2.938163982698783e00)  # fmt: skip
    d = (7.784695709041462e-03, 3.224671290700398e-01, 2.445134137142996e00,
         3.754408661907416e00)  # fmt: skip
    lo = 0.02425
    if p < lo or p > 1 - lo:
        q = _m.sqrt(-2.0 * _math.log(p if p < lo else 1.0 - p))
        x = ((((c[0] * q + c[1]) * q + c[2]) * q + c[3]) * q + c[4]) * q + c[5]
        x /= (((d[0] * q + d[1]) * q + d[2]) * q + d[3]) * q + 1.0
        if p > 1 - lo:
            x = -x
    else:
        q = p - 0.5
        r = q * q
        x = (((((a[0] * r + a[1]) * r + a[2]) * r + a[3]) * r + a[4]) * r + a[5]) * q
        x /= ((((b[0] * r + b[1]) * r + b[2]) * r + b[3]) * r + b[4]) * r + 1.0
    e = norm_cdf(x) - p
    u = e * 2.5066282746310002 * _math.exp(x * x / 2.0)
    return x - u / (1.0 + x * u / 2.0)


def _moments(xs):
    n = len(xs)
    m = 0.0
    for v in xs:
        m += v
    m /= n
    m2 = m3 = m4 = 0.0
    for v in xs:
        d = v - m
        m2 += d * d
        m3 += d * d * d
        m4 += d * d * d * d
    m2, m3, m4 = m2 / n, m3 / n, m4 / n
    return m, m2, m3, m4


def deflated_sharpe(report, trials, *, benchmark=0.0):
    """How likely the report's Sharpe beats what the best of `trials` tries would show by
    luck (Bailey and Lopez de Prado, 2014), given its days, skew and fat tails.
    `trials` are the annualised Sharpes of every setting tried (or sweep results, whose
    "sharpe" is read), the chosen one included. Returns {deflated_sharpe: the probability,
    probabilistic_sharpe: the same against `benchmark` (annualised) instead of the
    trials' luck, sharpe, threshold (annualised), trials, days}. Above 0.95 is the
    usual bar."""
    rets = report.daily_returns if not isinstance(report, list) else report
    n = len(rets)
    if n < 3:
        raise ValueError("needs at least three daily returns")
    mean, m2, m3, m4 = _moments(rets)
    sd = _m.sqrt(m2 * n / (n - 1))
    if sd == 0:
        raise ValueError("the returns don't vary")
    sr = mean / sd
    skew = m3 / (m2 * _m.sqrt(m2))
    kurt = m4 / (m2 * m2)
    year = _m.sqrt(365.0)
    srs = [(t.get("sharpe") if isinstance(t, dict) else t) for t in trials]
    srs = [v / year for v in srs if v is not None]
    k = len(srs)
    if k >= 2:
        var = Series(dict(enumerate(srs))).std()
        var = var * var
        sr0 = _m.sqrt(var) * ((1 - _EULER) * norm_ppf(1 - 1 / k) + _EULER * norm_ppf(1 - 1 / (k * _m.e)))
    else:
        sr0 = 0.0
    denom = _m.sqrt(max(1e-300, 1 - skew * sr + (kurt - 1) / 4 * sr * sr))

    def psr(threshold):
        return norm_cdf((sr - threshold) * _m.sqrt(n - 1) / denom)

    return {"deflated_sharpe": psr(sr0), "probabilistic_sharpe": psr(benchmark / year),
            "sharpe": sr * year, "threshold": sr0 * year, "trials": k, "days": n}  # fmt: skip


# The gauntlet: every check above on one strategy, and a verdict read off fixed rules.

LADDER = (
    "loses after costs",
    "fails nulls",
    "in-sample only",
    "holds out of sample",
    "passes deflated Sharpe",
    "passes sealed holdout",
)
NULL_SHARE = 0.05  # at most this share of a null's runs may match or beat the strategy
DEFLATED_BAR = 0.95  # the usual bar for the deflated Sharpe


def _sharpe(stats):
    return (stats or {}).get("sharpe")


def _neighbours(params, best, rows):
    """Sharpes of the grid points one step from `best` in one parameter."""
    out = []
    for name, values in params.items():
        i = values.index(best[name])
        for j in (i - 1, i + 1):
            if 0 <= j < len(values):
                want = {**best, name: values[j]}
                out.extend(_sharpe(r) for r in rows if r["params"] == want)
    return [v for v in out if v is not None]


def holding_period(report):
    """The median time between a market's trades in `report`, in ms, whole hours: fills of a
    market within an hour of the one before are one trade. None under three trades."""
    hour = 3_600_000
    last, gaps = {}, []
    for f in sorted(report.fills, key=lambda f: f.ts):
        prev = last.get(f.market)
        if prev is None or f.ts - prev[1] > hour:
            if prev is not None:
                gaps.append(f.ts - prev[0])
            last[f.market] = (f.ts, f.ts)
        else:
            last[f.market] = (prev[0], f.ts)
    if len(gaps) < 3:
        return None
    gaps.sort()
    mid = gaps[len(gaps) // 2]
    return max(hour, mid - mid % hour)


def gauntlet(make, params, start, end, source="data/hyperliquid", config=None, *, at=None,
             train="180d", test="60d", seeds=10, benchmark="BTC", base=None,
             max_leverage=None):  # fmt: skip
    """Runs every check on `make` (a Strategy, or a function of the grid `params`'
    names) over [start, end) and reads a verdict off fixed rules:

    1. The grid over the whole window, as trials; the best Sharpe is the candidate.
    2. Costs: the candidate's own stats, with fees, funding and the builder fee in.
    3. Nulls: flipped, `seeds` random and `seeds` stale signals (null_test), redrawn once
       per the candidate's own holding period (holding_period), so they trade about as
       often as it does rather than at every tick.
    4. Out of sample: split at `at` (default two thirds in), then walk-forward
       (`train`/`test`), when the window holds at least a train and two tests.
    5. The deflated Sharpe over every trial in the active study's ledger, or over this
       grid alone without a study.
    6. Robustness: the share of the grid with a positive Sharpe, the neighbours of the
       best point, and how much of the PnL one market made.

    Returns a dict: verdict {rung, level (1 to 6, LADDER), reasons, notes}, candidate,
    nulls, split, walk_forward, deflated, robustness, vs (against `benchmark`), holdout and
    trials. The verdict is the first rung whose rule fails, in LADDER order; the last rung
    needs the study's committed candidate to be this one and its holdout Sharpe above 0.
    """
    params = dict(params or {})
    base = dict(base or {})
    t0, t1 = when(start), when(end)
    run = {"source": source, "config": config, "max_leverage": max_leverage}
    st = _study.active()
    notes = []

    rows = sweep(make, t0, t1, params, base=base, **run)
    best = max(rows, key=lambda r: _score(r, "sharpe"))
    p = {**base, **best["params"]}
    with _study.kind("check"):
        report = backtest(make, t0, t1, params=p or None, benchmark=benchmark, **run)
    stats = report.stats or {}
    sharpe = stats.get("sharpe")

    every = holding_period(report)
    try:
        nulls = null_test(make, t0, t1, seeds=seeds, params=p or None, every=every, **run)
        nulls["redrawn_every_ms"] = every
    except ValueError as e:
        if "needs a strategy with a signal" not in str(e):
            raise
        nulls = None
        notes.append(f"nulls not run: {e} (a strategy written as one `step` has no signal "
                     "to flip or randomise)")  # fmt: skip

    cut = when(at) if at is not None else t0 + (t1 - t0) * 2 // 3 // 86_400_000 * 86_400_000
    sp = split(make, params, t0, t1, cut, base=base, **run)
    split_out = {"at": cut, "params": sp["params"], "in_sample": _sharpe(sp["in_sample"]),
                 "out_of_sample": sp["out_of_sample"]}  # fmt: skip

    wf = None
    if t1 - t0 >= duration_ms(train) + 2 * duration_ms(test):
        w = walk_forward(make, params, t0, t1, train=train, test=test, base=base, **run)
        wf = {"folds": len(w["folds"]), "params": [f["params"] for f in w["folds"]],
              "stats": w["stats"]}  # fmt: skip
    else:
        notes.append(f"walk-forward not run: the window is shorter than {train} plus two {test} tests")

    if st is not None:
        deflated = st.deflated_sharpe(report)
        deflated["over"] = "study"
    else:
        deflated = deflated_sharpe(report, rows)
        deflated["over"] = "grid"
        notes.append("no study: the deflated Sharpe counts this grid only, not earlier tries "
                     "(`0xbt study init` keeps a ledger)")  # fmt: skip
    if deflated["days"] < 365:
        notes.append(f"{deflated['days']} days in sample: under a year, so every test here is weak")

    attribution = report.attribution or {}
    pnl = {m: a["pnl"] for m, a in attribution.items()}
    total = 0.0
    for v in pnl.values():
        total += v
    top = max(pnl, key=pnl.get) if pnl else None
    gains = 0.0
    for v in pnl.values():
        if v > 0:
            gains += v
    near = _neighbours(params, best["params"], rows)
    positive = [r for r in rows if (_sharpe(r) or 0) > 0]
    robustness = {
        "grid_points": len(rows),
        "grid_positive_share": len(positive) / len(rows),
        "neighbour_sharpes": near,
        "neighbour_min_ratio": (min(near) / sharpe if near and sharpe else None),
        "top_market": top,
        "top_market_share": (pnl[top] / gains if top and gains > 0 else None),
        "pnl_without_top_market": (total - pnl[top] if top else None),
    }
    if robustness["neighbour_min_ratio"] is not None and robustness["neighbour_min_ratio"] < 0.5:
        notes.append("a peak, not a plateau: a neighbouring setting has under half the best Sharpe")
    if robustness["pnl_without_top_market"] is not None and robustness["pnl_without_top_market"] <= 0:
        notes.append(f"one market: without {top} the strategy made nothing")

    holdout = None
    if st is not None:
        what = _study._WHAT.get() or {}
        c = st.commit_record
        holdout = {"sealed": st.sealed, "starts": st.holdout_ms, "commit": c,
                   "result": st.holdout_result,
                   "this_candidate": bool(c and what.get("source_sha256")
                                          and c["source_sha256"] == what["source_sha256"]
                                          and c["params"] == p)}  # fmt: skip
        if not st.sealed:
            notes.append("the holdout is open: runs after the commit were not tested on unseen data")

    reasons, level = [], 6
    if sharpe is None or sharpe <= 0:
        level = 1
        reasons.append(f"Sharpe {sharpe} after costs")
    elif nulls is not None and (
        (nulls["flipped"] is not None and nulls["flipped"] >= sharpe)
        or (nulls.get("random_share_above") or 0) > NULL_SHARE
        or (nulls.get("stale_share_above") or 0) > NULL_SHARE
    ):
        level = 2
        reasons.append(f"nulls: flipped {nulls['flipped']}, random at or above "
                       f"{nulls.get('random_share_above')}, stale at or above "
                       f"{nulls.get('stale_share_above')} (bar {NULL_SHARE})")  # fmt: skip
    elif (_sharpe(split_out["out_of_sample"]) or 0) <= 0 or (wf and (_sharpe(wf["stats"]) or 0) <= 0):
        level = 3
        reasons.append(f"out of sample: split Sharpe {_sharpe(split_out['out_of_sample'])}, "
                       f"walk-forward Sharpe {_sharpe(wf['stats']) if wf else None}")  # fmt: skip
    elif deflated["deflated_sharpe"] < DEFLATED_BAR:
        level = 4
        reasons.append(f"deflated Sharpe {deflated['deflated_sharpe']:.2f} over "
                       f"{deflated['trials']} trial(s), under {DEFLATED_BAR}")  # fmt: skip
    elif not (holdout and holdout["this_candidate"] and (holdout["result"] or {}).get("passed")):
        level = 5
        reasons.append("no passed holdout for this candidate"
                       + ("" if st is not None else " (no study)"))  # fmt: skip
    return {
        "verdict": {"rung": LADDER[level - 1], "level": level, "reasons": reasons, "notes": notes},
        "window": {"start": t0, "end": t1},
        "candidate": {
            "params": best["params"],
            "stats": stats,
            "result_hash": report.result_hash,
            "attribution": attribution,
        },
        "nulls": nulls,
        "split": split_out,
        "walk_forward": wf,
        "deflated": deflated,
        "robustness": robustness,
        "vs": report.vs,
        "holdout": holdout,
        "grid": rows,
        "report": report,
    }
