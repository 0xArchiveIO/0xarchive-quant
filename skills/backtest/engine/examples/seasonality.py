# Hour-of-day seasonality: every hour, long the majors whose coming hour has been the
# strongest over the last 30 days and short the weakest, only while the US session is open.
#
# Shows:
#   h.close.seasonal(30, period="1d", ahead=1)  the mean return of the coming hour of the
#                       day over the last 30 days (ahead=0: the hour just closed, a control)
#   x.only("us")        trade only while NYSE is open; flat the rest of the time
#   x.rebalance(band=)  leave weights within 2% of equity alone
#
# Result (2025-03-01 to 2026-10-01, $100k, builder fee 1 bps): Sharpe -15.8, -92%. There is no edge here even with
# zero fees (Sharpe -0.59), and an hourly signal turns the book over about 1,900 times, so
# fees ($65k) do the rest. It is here to show the read; use it as a control (ahead=0) or on
# slower clocks before trusting it as a signal.
import oxarchive_backtest as oxb
from oxarchive_backtest import x

MAJORS = ["BTC", "ETH", "SOL", "XRP", "DOGE", "HYPE", "SUI", "AVAX", "LINK", "BNB"]


def strategy(days=30, quantile=0.3):
    return oxb.Strategy(
        universe=MAJORS,
        clock="1h",
        signal=lambda h: h.close.seasonal(days, period="1d", ahead=1),
        weights=x.long_short(quantile),
        risk=x.only("us", outside="flat"),
        execute=x.rebalance(band=0.02),
    )
