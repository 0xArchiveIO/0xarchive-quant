"""The 0xArchive backtest engine: a deterministic Rust engine, driven by a strategy
built from small swappable steps (references/strategy-api.md in the skill).

    import oxarchive_backtest as oxb
    from oxarchive_backtest import x

    vol = lambda h: h.shift(1).close.vol("30d")

    strategy = oxb.Strategy(
        universe=x.top_n(25),                                # point in time
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.ret("7d") / vol(h),         # a score per market
        weights=x.long_short(0.2, by=lambda h: 1 / vol(h)),  # target weights
    )

    report = oxb.backtest(strategy, "2025-07-01", "2025-10-01", source="data/hyperliquid")
    report.stats                       # return, sharpe, sortino, max_drawdown, turnover, ...
    report.attribution                 # each market's pnl, fees, funding, trades
    report.receipt(open(__file__).read())

The run works out its own warm-up, markets, columns and data from what the strategy reads,
and calls Python only at the strategy's clock ticks. `Session` runs the same loop on live
rows; `sweep` runs a parameter grid; `split`, `walk_forward`, `null_test` and
`deflated_sharpe` check a result isn't luck, and `gauntlet` runs them all for a verdict
(evaluate.py). A `Study` keeps a ledger of every run and a sealed holdout (study.py).

Importing this package makes the standard `math` module deterministic: its exp, log, sin, pow
and the rest become oxarchive_backtest.math, which gives the same bits on every machine and in
WebAssembly. `load_strategy` also makes `**` deterministic. See _determinism.py.

A strategy may import numpy, pandas, scipy and the standard library (`packages`), which is
the pinned, curated set. The `0xbt` command checks that and runs backtests (cli.py).
"""

from . import math, packages
from ._determinism import (
    DeterminismWarning,
    check_determinism,
    install_math,
    load_strategy,
    warn_determinism,
)
from ._native import (
    FILL_MODEL,
    Config,
    Ctx,
    Fill,
    Order,
    PickView,
    Receipt,
    Report,
    Universe,
    engine_version,
    reference,
)
from .run import MissingData, Session, Source, backtest, grid, picks, sweep
from .evaluate import LADDER, deflated_sharpe, gauntlet, null_test, split, walk_forward
from .study import Sealed, Study
from .calendar import Moment
from .series import Series
from .strategy import COLUMNS, Field, History, Strategy, duration_ms, when
from . import x

install_math()


def strategy_of(module, params=None):
    """The strategy a loaded file defines: its `strategy`, a Strategy or a function that
    returns one (called with `params`)."""
    s = getattr(module, "strategy", None)
    if s is None:
        raise ValueError("the file must define `strategy`: an oxb.Strategy, or a function returning one")
    if isinstance(s, Strategy):
        if params:
            raise ValueError("params need `strategy` to be a function that returns the Strategy")
        return s
    if callable(s):
        out = s(**(params or {}))
        if not isinstance(out, Strategy):
            raise TypeError("`strategy(...)` must return an oxb.Strategy")
        return out
    raise TypeError("`strategy` must be an oxb.Strategy, or a function returning one")


__all__ = [
    "COLUMNS",
    "LADDER",
    "FILL_MODEL",
    "Config",
    "Ctx",
    "DeterminismWarning",
    "Field",
    "Fill",
    "History",
    "MissingData",
    "Moment",
    "Order",
    "PickView",
    "Receipt",
    "Report",
    "Sealed",
    "Series",
    "Session",
    "Source",
    "Strategy",
    "Study",
    "Universe",
    "backtest",
    "check_determinism",
    "deflated_sharpe",
    "duration_ms",
    "engine_version",
    "gauntlet",
    "grid",
    "install_math",
    "load_strategy",
    "math",
    "null_test",
    "packages",
    "picks",
    "reference",
    "split",
    "strategy_of",
    "sweep",
    "walk_forward",
    "when",
    "x",
]
