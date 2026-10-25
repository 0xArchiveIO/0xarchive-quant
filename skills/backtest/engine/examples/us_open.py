# Overnight reversal at the US open: half an hour after the NYSE opens, long the perps that
# fell most since the previous US close and short the ones that rose most, then close
# everything five hours later, before the US close.
#
# Shows exchange-hours schedules and the time of day:
#   x.at_open("us", after="30m")  10:00 New York time on NYSE trading days only:
#                                 weekends, holidays and daylight saving are handled
#   h.now                         the tick's calendar: weekday, sessions, holidays...
#   x.only(condition)             skip Mondays, whose "overnight" is the whole weekend
#   x.time_stop("5h")             an exit checked every minute, whatever the clock
#
# Result (2025-03-01 to 2026-10-01, $100k, builder fee 1 bps): Sharpe -1.96, -25% in all. Fees ($27k) are most of the
# loss: in and out every trading day is 325x turnover for a small move.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(quantile=0.2, hold="5h"):
    # 16:00 to 10:00 New York time is 18 hours.
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.at_open("us", after="30m"),
        signal=lambda h: -h.close.ret("18h"),
        weights=x.long_short(quantile),
        risk=x.only(lambda now: now.weekday != 0),
        exits=[x.time_stop(hold)],
    )
