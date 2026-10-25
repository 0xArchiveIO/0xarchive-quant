# The same 7-day momentum on a fixed list of markets instead of the top 25: only the
# universe line differs. Fixed lists pick their markets with hindsight (these are today's
# big perps), so expect it to look better than the point-in-time version.
import oxarchive_backtest as oxb
from oxarchive_backtest import x

MARKETS = ["BTC", "ETH", "HYPE", "ZEC", "SOL", "NEAR", "XRP", "PUMP", "ENA", "LIT"]


def strategy(lookback="7d", quantile=0.2):
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=MARKETS,
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.ret(lookback) / vol(h),
        weights=x.long_short(quantile, by=lambda h: 1 / vol(h)),
    )
