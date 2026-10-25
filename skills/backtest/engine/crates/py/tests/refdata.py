"""Synthetic markets for the tests: the reference rows (oxs_engine::reference) written as a
source directory, <MARKET>/2026-10.parquet."""

import os

import oxarchive_backtest as oxb
from oxarchive_backtest import _native

T0 = 1_791_208_800_000  # the reference rows' first minute
MIN = 60_000
HOUR = 3_600_000
CONFIG = oxb.Config(builder_fee_bps=1.0)


def write(root, name, n, market=None, scale=1.0, quote_volume=None, seed=None):
    """One market of n minutes from T0. Returns the source root."""
    _native._write_reference(os.path.join(str(root), name), n, market or "synthetic:reference",
                             scale, quote_volume, seed)  # fmt: skip
    return str(root)


def end(n):
    return T0 + n * MIN


def rows(root, name):
    """A market's rows as live-stream dicts."""
    return _native.load(os.path.join(str(root), name)).rows()


def mean_cross(market="REF", fills=None):
    """The Rust reference strategy (oxs_engine::reference::MeanCross): long one unit when
    the 10-bar mean is above the 30-bar mean, flat otherwise."""

    def step(ctx, h):
        fast, slow = h.close.mean(10)[market], h.close.mean(30)[market]
        if slow is None:
            return
        delta = (1.0 if fast > slow else 0.0) - ctx.positions[market]
        if delta != 0.0:
            try:
                ctx.market(market, delta)
            except ValueError:
                pass

    on_fill = (lambda ctx, f: fills.append(f)) if fills is not None else None
    return oxb.Strategy(universe=[market], clock="1m", bars="1m", step=step, on_fill=on_fill)
