# Residual momentum: each perp's last 15 days of returns beyond what its 60-day beta
# to BTC explains, over its volatility. The book is long-short and scaled to zero beta to
# BTC, and stops (flat, for good) if it ever falls 25% from its peak. Sharpe 0.79 from
# June 2025 to September 2026, weaker here than plain 7-day momentum (0.92 without the
# beta-neutral scaling).
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(quantile=0.2):
    vol = lambda h: h.shift(1).close.vol("30d")
    return oxb.Strategy(
        universe=x.top_n(25),
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.resid("15d", to="BTC", fit="60d") / vol(h),
        weights=x.long_short(quantile, by=lambda h: 1 / vol(h)),
        risk=x.beta_neutral("BTC"),
        exits=[x.drawdown_stop(0.25)],
    )
