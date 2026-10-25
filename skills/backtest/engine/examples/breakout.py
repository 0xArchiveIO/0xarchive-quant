# Your own orders: buy BTC when an hour closes above the previous day's high, with a
# stop 3% below and a time exit after two days. step= replaces signal and weights;
# ctx.state keeps what the strategy needs between ticks.
import oxarchive_backtest as oxb

DAY = 86_400_000


def step(ctx, h):
    close = h.close.last()["BTC"]
    high = h.shift(1).high.max("24h")["BTC"]
    if close is None or high is None:
        return
    held = ctx.positions["BTC"]
    if held == 0 and close > high:
        size = ctx.equity / close
        ctx.cancel_all()
        ctx.market("BTC", size)
        ctx.stop("BTC", -size, close * 0.97, reduce_only=True)
        ctx.state["entered"] = ctx.time
    elif held != 0 and ctx.time - ctx.state.get("entered", ctx.time) >= 2 * DAY:
        ctx.cancel_all()
        ctx.market("BTC", -held, reduce_only=True)


strategy = oxb.Strategy(universe=["BTC"], clock="1h", step=step)
