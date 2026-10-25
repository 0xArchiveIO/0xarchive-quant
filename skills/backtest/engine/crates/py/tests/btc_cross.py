"""An hourly EWMA cross over real BTC data, long only while the week's daily log return is
above -20%: check-pyodide.sh runs it natively and in Pyodide over the whole dataset and
compares the hashes. EWMA weights and log returns go through oxs_math, and the daily bars
through a second history."""

import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(fast="20h", slow="50h"):
    return oxb.Strategy(
        universe=["BTC"],
        clock="1h",
        signal=lambda h: (h.close.ewma(fast) - h.close.ewma(slow)).where(
            h.bars("1d").close.logret(7) > -0.2
        ),
        weights=x.sign(short=False),
    )


def run(oxb, root):
    """(fills, hash) over every month of `root`/BTC."""
    from oxarchive_backtest.run import month_start

    months = oxb.Source(root).index()["BTC"]
    y, m = map(int, months[-1].split("-"))
    end = month_start(f"{y + m // 12}-{m % 12 + 1:02d}")
    r = oxb.backtest(strategy(), month_start(months[0]), end, root,
                     oxb.Config(starting_cash=100_000, builder_fee_bps=1.0))  # fmt: skip
    return len(r.fills), r.result_hash
