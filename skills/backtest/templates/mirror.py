# A trader's own book as a strategy: the positions `trader.py fetch` pulled for one wallet,
# held through the engine's fills, fees and funding.
#
#     0xbt backtest mirror.py -p path=trader.json --start 2025-06-01 --end 2026-07-01
#     0xbt gauntlet mirror.py -p path=trader.json --start ... --end ...
#
# The signal at each tick is the trader's signed position value in each market, divided by
# their largest gross position value over the file, so the book's gross never passes 1.
# That makes the gauntlet's nulls mean something for a trader:
#   flipped  - fading them;
#   random   - random positions in the same markets on the same clock (the machinery);
#   stale    - their own positions at random earlier times: their book without its timing.
# A trader whose record beats every stale run has timing, not just exposure.
#
# The book is sampled at each clock tick and traded from the next minute. The default
# clock="auto" picks it from their median holding time: the coarsest of 1h, 15m, 5m and 1m
# that is at most a tenth of it, so a fast trader is followed closely and a slow one
# doesn't pay for ticks it doesn't need. Pass clock="1h" (or any duration) to fix it. A weight is traded only when it moves more than `band` (a fraction of
# equity) from what is held, so price drift alone doesn't trade. Markets the dataset lacks
# are left out; trader.py profile lists them.
import bisect
import json

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x


CLOCKS = (("1h", 3_600_000), ("15m", 900_000), ("5m", 300_000), ("1m", 60_000))


def median_hold_ms(changes):
    """Median time from a position opening to closing or flipping, over every market."""
    holds, opened, prev = [], {}, {}
    for c in changes:
        m, pos = c["symbol"], c["end_position"]
        was = prev.get(m, 0.0)
        if was != 0 and (pos == 0 or was * pos < 0) and m in opened:
            holds.append(c["ts"] - opened.pop(m))
        if pos != 0 and (was == 0 or was * pos < 0):
            opened[m] = c["ts"]
        prev[m] = pos
    holds.sort()
    return holds[len(holds) // 2] if holds else None


def auto_clock(changes):
    hold = median_hold_ms(changes)
    if hold is None:
        return "1h"
    return next((name for name, ms in CLOCKS if ms <= hold / 10), "1m")


def strategy(path="trader.json", clock="auto", band=0.01):
    with open(path, encoding="utf-8") as f:
        book = json.load(f)
    if clock == "auto":
        clock = auto_clock(sorted(book["changes"], key=lambda c: c["ts"]))
    paths = {}
    for c in book["changes"]:
        ts, sizes = paths.setdefault(c["symbol"], ([], []))
        ts.append(c["ts"])
        sizes.append(c["end_position"])
    scale = book["max_gross_usd"]
    markets = sorted(m for m in paths if m in set(book.get("in_dataset", paths)))

    def held(market, t):
        ts, sizes = paths[market]
        i = bisect.bisect_right(ts, t) - 1
        return sizes[i] if i >= 0 else 0.0

    def signal(h):
        px = h.close.last()
        t = h.time
        out = {}
        for m in h.markets:
            p = px[m]
            out[m] = None if t is None or p is None else held(m, t) * p / scale
        return Series(out)

    return oxb.Strategy(
        universe=markets,
        clock=clock,
        signal=signal,
        weights=lambda s: s,
        # Random nulls draw each market's weight from [-1, 1); keep their book at 1x too.
        risk=[x.cap(1.0, gross=1.0)],
        execute=x.rebalance(band=band),
    )
