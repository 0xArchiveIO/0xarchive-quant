# 7-day momentum (examples/momentum.py) with every kind of exit. Exits are checked every
# minute by the engine, whatever the clock, and close at the next minute's open with a
# reduce-only market order; the market then stays flat until the next weekly rebalance.
#
#   x.stop_loss(0.15)        a position 15% against its entry
#   x.take_profit(0.5)       a position 50% in its favour
#   x.trailing_stop(0.2)     a position 20% off its best price since entry
#   x.time_stop("5d")        a position held five days
#   x.drawdown_stop(0.15, cooldown="30d")   the whole book 15% off its peak: close
#                            everything and stop trading for 30 days
#
# report.exits lists each one: (time, market or None for the book, why).
#
# Result (2025-03-01 to 2026-10-01, $100k): Sharpe 0.52, 10.1% in all, against 2.12 and 80% for
# the same strategy without exits. 675 exits fired (all five kinds): stops cut momentum's
# winners as often as its losers, and a flat market earns nothing until the next week.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(stop=0.15, trail=0.2, target=0.5, max_hold="5d", drawdown=0.15):
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.ret("7d") / vol(h),
        weights=x.long_short(0.2, by=lambda h: 1 / vol(h)),
        exits=[x.stop_loss(stop), x.take_profit(target), x.trailing_stop(trail),
               x.time_stop(max_hold), x.drawdown_stop(drawdown, cooldown="30d")],
    )
