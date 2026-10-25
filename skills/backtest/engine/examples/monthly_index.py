# A crypto index: the 10 most traded perps, long only, weighted by inverse volatility,
# re-picked and re-weighted on the first of each month and held at those weights daily.
#
# Shows:
#   x.monthly(1)     the first day of each month, 00:00 UTC (x.monthly(-1): the last day)
#   hold="1d"        re-trade back to the month's weights every day, so winners don't
#                    drift into a bigger share (without it, weights drift with prices)
#   repair=True      a close more than 2.5x off the 48 hours before it is treated as a
#                    bad print and skipped, so one wick can't set a
#                    month's weights
#   x.long_only(top=, by=)  the top `top` scores held long, each weighted by `by`
#
# Compare it with holding BTC: 0xbt backtest examples/monthly_index.py ... or
# oxb.backtest(strategy, ..., benchmark="BTC").
#
# Result (2025-03-01 to 2026-10-01, $100k): -14% in all, Sharpe 0.13, max drawdown 68%, beta to
# BTC 1.06: a long-only crypto basket through a falling market. Fees $0.3k, funding $8.8k.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(n=10):
    inverse_vol = lambda h: 1 / h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.top_n(n),
        clock=x.monthly(1),
        hold="1d",
        repair=True,
        signal=inverse_vol,
        weights=x.long_only(top=n, by=inverse_vol),
        execute=x.rebalance(band=0.01),
    )
