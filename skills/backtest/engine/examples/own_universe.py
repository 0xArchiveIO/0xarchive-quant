# Your own universe rule, and a long-only book with its market risk hedged out.
#
# Every two weeks the rule picks the 12 most traded perps among those listed for at least
# 120 days whose hourly volatility over the last 30 days is below the median: established,
# calmer markets. The strategy holds the 4 with the best 30-day momentum, weighted by
# inverse volatility, then:
#   x.cap(0.35)        no market above 35% of the long book
#   x.hedge("BTC")     shorts BTC against the book's 60-day beta to it
#   x.vol_target(0.2)  scales the hedged book to 20% annual volatility (at most 3x)
#   x.turnover(0.5)    moves at most half the way to the new weights each week
#
# x.rule(select, every=) calls select(view) with data from before the pick only, so the
# rule is point in time by construction; view.listed, view.volatility, view.volume,
# view.open_interest, view.funding and view.history_hours are what it can read.
#
# Result (2025-03-01 to 2026-10-01, $100k): Sharpe 0.86, 14.1% a year, max drawdown 20%, beta to
# BTC 0.18 after the hedge, fees $1.2k.
import oxarchive_backtest as oxb
from oxarchive_backtest import x

DAYS = 24  # hours


def established_and_calm(view):
    old = [m for m in view.markets if view.history_hours(m) >= 120 * DAYS]
    vols = {m: view.volatility(m, 30 * DAYS) for m in old}
    known = sorted(v for v in vols.values() if v is not None)
    if not known:
        return None
    median = known[len(known) // 2]
    calm = [m for m in old if vols[m] is not None and vols[m] <= median]
    return sorted(calm, key=lambda m: -(view.volume(m, 30 * DAYS) or 0))[:12]


def strategy(top=4, lookback="30d"):
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.rule(established_and_calm, every="14d"),
        clock=x.weekly("mon", at="01:00"),
        signal=lambda h: h.close.ret(lookback) / vol(h),
        weights=x.long_only(top=top, by=lambda h: 1 / vol(h)),
        risk=[x.cap(0.35), x.hedge("BTC"), x.vol_target(0.2), x.turnover(0.5)],
        execute=x.rebalance(band=0.02),
    )
