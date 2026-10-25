"""A strategy that leans on transcendental math and **: the cross-platform check for
deterministic math. Loaded with oxarchive_backtest.load_strategy by tests/test_math.py natively
and must give the same trace and hash on every machine.
"""

import math

import oxarchive_backtest as oxb


def strategy(market="REF"):
    """Long when the volatility-scaled 30-bar log return is positive, sized with tanh."""
    trace = []  # every computed target, to compare bit for bit across machines

    def step(ctx, h):
        closes = h.close.window(31)[market]
        if closes[0] is None:
            return
        rets = [math.log(closes[i] / closes[i - 1]) for i in range(1, 31)]
        mean = 0.0
        for r in rets:
            mean += r
        mean /= 30
        var = 0.0
        for r in rets:
            var += (r - mean) ** 2
        vol = math.sqrt(var / 29)
        z = mean / vol * 30**0.5 if vol > 0 else 0.0
        target = math.tanh(z) * math.exp(-vol * 100) * math.cos(math.atan2(mean, vol))
        trace.append(target)
        target = round(target, 2)
        delta = target - ctx.positions[market]
        if abs(delta) >= 0.05:
            try:
                ctx.market(market, delta)
            except ValueError:
                pass

    s = oxb.Strategy(universe=[market], clock="1m", bars="1m", step=step)
    s.trace = trace
    return s


def trace_hash(values):
    import hashlib
    import struct

    return hashlib.sha256(b"".join(struct.pack("<d", v) for v in values)).hexdigest()[:16]


def run(oxb, root, platform_math=False):
    """Loads this file through load_strategy and backtests it over the 5,000-minute REF
    market in `root`; returns (fills, hash, trace). With platform_math the module's `math`
    is the platform's own, as a control."""
    import pathlib
    import types

    mod = oxb.load_strategy(pathlib.Path(__file__), name="log_momentum_under_test")
    if platform_math:
        from oxarchive_backtest._determinism import _ORIGINAL

        mod.math = types.SimpleNamespace(**{**vars(math), **_ORIGINAL})
    s = mod.strategy()
    t0 = 1_791_208_800_000
    r = oxb.backtest(s, t0, t0 + 5_000 * 60_000, root, oxb.Config())
    return len(r.fills), r.result_hash, trace_hash(s.trace)
