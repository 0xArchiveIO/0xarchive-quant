# A blended trend signal, cleaned of market and sector moves.
#
# Shows the reads and the cross-section:
#   h.bars("1d")          daily bars beside the hourly ones, in one strategy
#   .logret(20)           20-day log return, on the daily bars
#   .ewma("2d")           exponentially weighted mean price (halflife 2 days)
#   x.blend(a, b)         the mean of each signal's cross-sectional rank
#   .beta("60d", to=)     each market's beta to BTC over 60 days of hourly returns
#   .neutralize(beta)     what is left of the score after regressing it on beta
#   .demean(by=SECTORS)   less each sector's mean, so it ranks markets within sectors
#   .corr("30d", to=)     correlation with BTC; .where() drops the ones moving with it
#
# Result (2025-03-01 to 2026-10-01, $100k): Sharpe 0.79, 15.3% a year, max drawdown 23%, beta to
# BTC -0.05.
import oxarchive_backtest as oxb
from oxarchive_backtest import x

SECTORS = {
    **dict.fromkeys(["BTC", "ETH", "SOL", "AVAX", "ADA", "DOT", "NEAR", "SUI", "SEI", "TON",
                     "BNB", "XRP", "LTC", "BCH", "XLM", "XMR", "ZEC", "MON", "BERA", "HYPE",
                     "IP", "XPL"], "l1"),
    **dict.fromkeys(["ARB", "STRK", "MNT", "ZORA"], "l2"),
    **dict.fromkeys(["AAVE", "CRV", "UNI", "LDO", "MKR", "PENDLE", "JTO", "ENA", "ONDO",
                     "ASTER", "LIT", "TNSR", "WLFI", "PUMP", "LINK", "ZRO"], "defi"),
    **dict.fromkeys(["DOGE", "kPEPE", "kSHIB", "kBONK", "WIF", "POPCAT", "MOODENG", "FARTCOIN",
                     "PENGU", "SPX", "TRUMP", "LAUNCHCOIN", "CASHCAT"], "meme"),
    **dict.fromkeys(["TAO", "VIRTUAL", "VVV", "KAITO", "WLD", "BIO", "PAXG"], "other"),
}


def signal(h):
    slow = h.bars("1d").close.logret(20)
    fast = h.close.ewma("2d") / h.close.ewma("10d") - 1
    score = x.blend(slow, fast)
    score = score.neutralize(h.close.beta("60d", to="BTC")).demean(by=SECTORS)
    return score.where(h.close.corr("30d", to="BTC") < 0.9)


def strategy(quantile=0.2):
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.weekly("mon", at="01:00"),
        signal=signal,
        weights=x.long_short(quantile),
    )
