# Long whichever of BTC and ETH rose more over the last week and short the other, half
# the book each, checked daily. The weights step is a plain function; execution skips
# trades under 5% of equity, so the book isn't nudged back to 50/50 every day. (Checked
# hourly on 1-day returns it flips so often that fees take 20% of the book a quarter.)
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def leader_laggard(scores):
    s = scores.dropna()
    if len(s) < 2:
        return None
    (leader,), (laggard,) = s.top(1), s.bottom(1)
    return {leader: 0.5, laggard: -0.5}


strategy = oxb.Strategy(
    universe=["BTC", "ETH"],
    clock="1d",
    signal=lambda h: h.close.ret("7d"),
    weights=leader_laggard,
    execute=x.rebalance(band=0.05),
)
