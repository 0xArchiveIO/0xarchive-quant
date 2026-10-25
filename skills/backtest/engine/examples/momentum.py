# 7-day momentum across the top 25 perps: the factor in our research that
# held up on Hyperliquid.
#
# Point in time: the 25 perps with the most dollar volume over the last 30 days, re-picked
# every 30 days from data before each pick, delisted ones included. Every week, rank them
# by 7-day return over volatility; long the top fifth and short the bottom fifth (6 and 5
# of 25), each leg weighted by inverse volatility.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(lookback="7d", quantile=0.2):
    # our factor research's volatility: hourly returns over the 30 days before this hour.
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.ret(lookback) / vol(h),
        weights=x.long_short(quantile, by=lambda h: 1 / vol(h)),
    )
