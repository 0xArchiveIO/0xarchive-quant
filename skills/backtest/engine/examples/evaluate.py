"""Is examples/momentum.py real? Every check in oxb.evaluate on one strategy:

    python examples/evaluate.py [--data data/hyperliquid] [--seeds 10]

1. Against holding BTC: excess return, beta, alpha, information ratio (benchmark=).
2. Where the money came from: per-market PnL after fees and funding (attribution).
3. Nulls: the flipped signal, random signals (what the universe, weighting and costs earn
   on their own) and its own signal at random earlier times (null_test).
4. A sweep of 8 settings and the deflated Sharpe: the chance the best of them beats what
   8 tries would show by luck (sweep, deflated_sharpe).
5. Out of sample: pick the lookback on data before 2026-01-01, run it after (split), and
   walk-forward with 180-day training windows and 60-day test windows (walk_forward).

It runs about 50 backtests: 7 minutes on an M-series Mac with the full dataset, which gives
(2025-03-01 to 2026-10-01, $100k, builder fee 1 bps):

    momentum: Sharpe 2.09, 43.8% a year, max drawdown 20.4%, fees 8,512, funding 3,656
    vs BTC: beta -0.08, alpha 38.5% a year, information ratio 0.77
    worst markets: XRP -15,174, kPEPE -8,975, PUMP -8,342
    best markets:  FARTCOIN 23,067, ZEC 16,908, SUI 11,999
    flipped signal: Sharpe -2.47
    random signals: mean Sharpe -0.14 (sd 0.85), 0% at or above
    stale signals: mean Sharpe 0.08 (sd 0.82), 0% at or above
    deflated Sharpe over 8 settings: 0.86 (the best of 8 tries by luck: Sharpe 1.10; ...)
    split at 2026-01-01: picked {'lookback': '7d'}, Sharpe 3.17 before, 1.70 after
    walk-forward (7 folds, picked 7d, 7d, 7d, 7d, 7d, 7d, 7d): Sharpe 1.98, return 58.2%

The signal beats every null and holds out of sample, but over 8 tries its deflated Sharpe
is below the usual 0.95 bar: 19 months is a short sample.
The strategy is loaded the way every surface loads it (load_strategy), so the checks see
exactly what the browser and the runner would run.
"""

import argparse
import pathlib
import time

import oxarchive_backtest as oxb

HERE = pathlib.Path(__file__).parent
START, END = "2025-03-01", "2026-10-01"
CONFIG = oxb.Config(starting_cash=100_000, builder_fee_bps=1)


def main(data="data/hyperliquid", seeds=10, start=START, end=END, split_at="2026-01-01",
         train="180d", test="60d", out=print):  # fmt: skip
    make = oxb.load_strategy(HERE / "momentum.py", name="momentum").strategy
    t0 = time.time()

    r = oxb.backtest(make, start, end, data, CONFIG, benchmark="BTC")
    s, vs = r.stats, r.vs
    out(f"momentum: Sharpe {s['sharpe']:.2f}, {s['annual_return']:.1%} a year, "
        f"max drawdown {s['max_drawdown']:.1%}, fees {s['fees']:,.0f}, funding {s['funding_paid']:,.0f}")
    out(f"vs BTC: beta {vs['beta']:.2f}, alpha {vs['alpha']:.1%} a year, "
        f"information ratio {vs['information_ratio']:.2f}")

    pnl = sorted((a["pnl"], m) for m, a in r.attribution.items())
    out("worst markets: " + ", ".join(f"{m} {p:,.0f}" for p, m in pnl[:3]))
    out("best markets:  " + ", ".join(f"{m} {p:,.0f}" for p, m in pnl[-3:][::-1]))

    nulls = oxb.null_test(make, start, end, data, CONFIG, seeds=seeds)
    out(f"flipped signal: Sharpe {nulls['flipped']:.2f}")
    for kind in ("random", "stale"):
        out(f"{kind} signals: mean Sharpe {nulls[kind + '_mean']:.2f} "
            f"(sd {nulls[kind + '_sd']:.2f}), {nulls[kind + '_share_above']:.0%} at or above")

    grid = {"lookback": ["3d", "7d", "14d", "30d"], "quantile": [0.2, 0.3]}
    trials = oxb.sweep(make, start, end, grid, data, CONFIG)
    d = oxb.deflated_sharpe(r, trials)
    out(f"deflated Sharpe over {d['trials']} settings: {d['deflated_sharpe']:.2f} "
        f"(the best of {d['trials']} tries by luck: Sharpe {d['threshold']:.2f}; 0.95 is the usual bar)")

    sp = oxb.split(make, {"lookback": grid["lookback"]}, start, end, at=split_at, source=data,
                   config=CONFIG)  # fmt: skip
    out(f"split at {split_at}: picked {sp['params']}, Sharpe {sp['in_sample']['sharpe']:.2f} "
        f"before, {sp['out_of_sample']['sharpe']:.2f} after")

    wf = oxb.walk_forward(make, {"lookback": grid["lookback"]}, start, end, train=train,
                          test=test, source=data, config=CONFIG)  # fmt: skip
    picks = ", ".join(f["params"]["lookback"] for f in wf["folds"])
    out(f"walk-forward ({len(wf['folds'])} folds, picked {picks}): "
        f"Sharpe {wf['stats']['sharpe']:.2f}, return {wf['stats']['return']:.1%}")
    out(f"{time.time() - t0:.0f}s")


if __name__ == "__main__":
    a = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    a.add_argument("--data", default="data/hyperliquid")
    a.add_argument("--seeds", type=int, default=10)
    main(**vars(a.parse_args()))
