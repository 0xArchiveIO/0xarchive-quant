# 7-day momentum re-ranked every day, held on weekdays only, and traded an hour after the
# signal.
#
# Shows calendar schedules and timing:
#   x.daily("00:00")         every day at midnight UTC (tz= for any other zone)
#   x.only(..., "flat")      at the Saturday and Sunday ticks, go flat instead of trading
#   delay="1h"               the weights are worked out at midnight and traded at 01:00,
#                            a check that the result doesn't hang on trading the instant
#                            the signal is known
#   x.rebalance(band=0.05)   with a daily clock, only trade weights off by 5% of equity
#
# Result (2025-03-01 to 2026-10-01, $100k, builder fee 1 bps): Sharpe -0.36, -12% in all. The same signal re-ranked
# weekly (examples/momentum.py) has Sharpe 2.09: trading it daily, and going flat and back
# every weekend, turns the book over 187 times and fees take $15.9k.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(lookback="7d", quantile=0.2, delay="1h"):
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.daily("00:00"),
        delay=delay,
        signal=lambda h: h.close.ret(lookback) / vol(h),
        weights=x.long_short(quantile, by=lambda h: 1 / vol(h)),
        risk=x.only(lambda now: not now.weekend, outside="flat"),
        execute=x.rebalance(band=0.05),
    )
