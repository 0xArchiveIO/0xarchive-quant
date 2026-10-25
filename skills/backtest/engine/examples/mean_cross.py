# Long BTC when its 20-hour average is above its 50-hour average, flat otherwise.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(fast="20h", slow="50h"):
    return oxb.Strategy(
        universe=["BTC"],
        clock="1h",
        signal=lambda h: h.close.mean(fast) - h.close.mean(slow),
        weights=x.sign(short=False),
    )
