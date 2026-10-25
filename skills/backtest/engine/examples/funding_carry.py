# Funding carry: short the perps whose longs have been paying the most funding and long
# the ones paying the least, dollar neutral, re-weighted daily.
#
# Shows the universe filters and the book pieces:
#   x.top_n(by="open_interest", min_volume=, exclude=, sectors=, per_sector=)
#   on_universe      close a market the day it leaves the universe, not at the next tick
#   h.funding_rate   any dataset column is a field; .mean("7d") is its 7-day mean
#   x.book           gross 1, net 0: both legs the same size whatever the scores
#   x.cap            no market above 15% of equity
#   x.rebalance      skip trades under 2% of equity, and the whole rebalance if it costs
#                    more than 0.2% of equity
#
# Result (2025-03-01 to 2026-10-01, $100k): Sharpe -0.67, -20% in all, max drawdown 29%. It
# collected $10.9k of funding and lost more than that on price: the perps paying the most
# funding kept rising. A good example of a carry that doesn't pay for its risk here.
import oxarchive_backtest as oxb
from oxarchive_backtest import x

# A rough, hand-made sector map for the dataset's markets; per_sector keeps the universe
# from filling up with one theme. Markets missing from it count as their own sector.
SECTORS = {
    **dict.fromkeys(["BTC", "ETH", "SOL", "AVAX", "ADA", "DOT", "NEAR", "SUI", "SEI", "TON",
                     "BNB", "XRP", "LTC", "BCH", "XLM", "XMR", "ZEC", "MON", "BERA", "HYPE",
                     "IP", "XPL"], "l1"),
    **dict.fromkeys(["ARB", "STRK", "MNT", "ZORA"], "l2"),
    **dict.fromkeys(["AAVE", "CRV", "UNI", "LDO", "MKR", "PENDLE", "JTO", "ENA", "ONDO",
                     "ASTER", "LIT", "TNSR", "WLFI", "PUMP", "LINK", "ZRO"], "defi"),
    **dict.fromkeys(["DOGE", "kPEPE", "kSHIB", "kBONK", "WIF", "POPCAT", "MOODENG", "FARTCOIN",
                     "PENGU", "SPX", "TRUMP", "LAUNCHCOIN", "CASHCAT"], "meme"),
    **dict.fromkeys(["TAO", "VIRTUAL", "VVV", "KAITO", "WLD", "BIO"], "ai"),
}


def close_leavers(ctx, joined, left):
    for m in left:
        if ctx.positions[m] != 0:
            ctx.market(m, -ctx.positions[m], reduce_only=True)


def strategy(n=20, quantile=0.3, lookback="7d"):
    return oxb.Strategy(
        universe=x.top_n(n, by="open_interest", min_volume=100e6, exclude=["PAXG"],
                         sectors=SECTORS, per_sector=6),
        clock="1d",
        signal=lambda h: -h.funding_rate.mean(lookback),
        weights=x.long_short(quantile),
        risk=[x.book(gross=1.0, net=0.0), x.cap(0.15)],
        execute=x.rebalance(band=0.02, max_cost=0.002),
        on_universe=close_leavers,
    )
