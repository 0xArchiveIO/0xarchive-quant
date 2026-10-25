"""Calendar clocks, delay and held weights, calendar fields, sessions and seasonal reads.

The reference rows start on Monday 2026-10-05 at 14:00 UTC (T0).
"""

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import Series, x
from oxarchive_backtest.calendar import Moment
from refdata import CONFIG, HOUR, MIN, T0, end, write

DAY = 86_400_000


def ticks(root, clock, n, **kw):
    seen = []

    def step(ctx, h):
        if ctx.time:
            seen.append(ctx.time)

    oxb.backtest(oxb.Strategy(universe=["A"], clock=clock, step=step, **kw), T0, end(n), root)
    return seen


def test_calendar_and_session_clocks(tmp_path):
    root = write(tmp_path, "A", 3 * 1440)
    # 13:00 New York on Tuesday is 17:00 UTC (daylight time).
    assert ticks(root, x.weekly("tue", at="13:00", tz="America/New_York"), 3 * 1440) == [
        oxb.when("2026-10-06 17:00")
    ]
    # Fifteen minutes before each NYSE close: 19:45 UTC, Monday to Wednesday.
    closes = ticks(root, x.at_close("us", before="15m"), 3 * 1440)
    assert closes == [oxb.when(f"2026-10-0{d} 19:45") for d in (5, 6, 7)]
    assert ticks(root, x.daily("09:30", tz="America/New_York"), 3 * 1440) == [
        oxb.when(f"2026-10-0{d} 13:30") for d in (6, 7, 8)
    ]
    assert x.monthly(-1).last_tick("2026-10-07") == oxb.when("2026-09-30")
    assert x.clock("weekly").last_tick("2026-10-07 12:00") == oxb.when("2026-10-05")
    with pytest.raises(ValueError, match="mon, tue"):
        x.weekly("someday")
    with pytest.raises(ValueError, match="no zone"):
        x.daily(tz="Mars/Olympus")
    with pytest.raises(ValueError, match="day 1 to 28"):
        x.monthly(31)


def test_delay_trades_after_the_tick_with_the_ticks_weights(tmp_path):
    root = write(tmp_path, "A", 600)
    seen = []

    def weights(scores, h):
        seen.append(h.time)
        return Series({"A": 0.5 if (h.time // HOUR) % 2 else -0.5})

    s = oxb.Strategy(universe=["A"], clock="1h", delay="20m", weights=weights)
    r = oxb.backtest(s, T0, end(600), root, CONFIG)
    first_tick = T0 + HOUR
    assert seen[1] == first_tick  # seen[0] is the check before the run
    # Weights from the 15:00 tick (odd: long) trade at 15:20 and fill in the next minute.
    f = r.fills[0]
    assert f.ts == first_tick + 20 * MIN and f.size > 0
    with pytest.raises(ValueError, match="own orders"):
        oxb.Strategy(universe=["A"], step=lambda c, h: None, delay="1h")


def test_hold_keeps_weights_fixed_between_ticks(tmp_path):
    root = write(tmp_path, "A", 1200)
    w = lambda s, h: Series({"A": 1.0})
    drift = oxb.backtest(oxb.Strategy(universe=["A"], clock="1d", weights=w), T0, end(1200), root)
    held = oxb.backtest(oxb.Strategy(universe=["A"], clock="1d", hold="1h", weights=w), T0,
                        end(1200), root)  # fmt: skip
    # Drifting trades once, at the first daily tick; held re-trades every hour after it.
    assert len(drift.fills) == 1
    hours = sorted({f.ts // HOUR for f in held.fills})
    assert len(hours) > 5 and all(b - a == 1 for a, b in zip(hours, hours[1:]))


def test_moment_fields_sessions_and_events():
    m = Moment("2026-10-07 19:45")  # a Wednesday
    assert (m.year, m.month, m.day, m.hour, m.minute, m.weekday) == (2026, 10, 7, 19, 45, 2)
    assert m.local("America/New_York").hour == 15
    assert m.open("us") and m.minutes_to_close("us") == 15
    assert m.minutes_since_open("us") == 375
    assert not m.open("europe") and m.minutes_since_close("europe") == 255
    assert m.minutes_to_funding == 15 and not m.weekend
    assert m.days_to_month_end == 24 and m.quarter == 4 and m.days_to_quarter_end == 85
    assert Moment("2026-11-26 15:00").holiday("us") == "Thanksgiving Day"
    assert Moment("2026-11-26 15:00").minutes_to_open("us") == (14 * 60 + 30) + 9 * 60
    # Deribit's 08:00 UTC expiries: 2026-09-25 is the last Friday of a quarter.
    assert Moment("2026-09-25").expiry == "quarterly"
    assert Moment("2026-10-30").expiry == "monthly"
    assert Moment("2026-10-09").expiry == "weekly"
    assert Moment("2026-10-07").expiry == "daily"
    assert Moment("2026-10-07 08:00").hours_to_expiry("weekly") == 48
    assert Moment("2026-10-07 08:00").hours_to_expiry("monthly") == 23 * 24


def test_seasonal_read_and_only_filter(tmp_path):
    root = write(tmp_path, "A", 4 * 1440)
    got = []

    def step(ctx, h):
        typical = h.close.seasonal(2, period="1d", ahead=1)["A"]
        w = h.close.window(2 * 24 + 1)["A"]
        if ctx.time and typical is not None:
            # The coming hour's returns one and two days ago.
            rets = [w[k] / w[k - 1] - 1 for k in (1, 25)]
            got.append((typical, (rets[0] + rets[1]) / 2, h.now.hour))

    oxb.backtest(oxb.Strategy(universe=["A"], clock="1h", step=step), T0, end(4 * 1440), root)
    assert got and all(a == pytest.approx(b, abs=1e-15) for a, b, _ in got)

    # Only in US hours: the book trades only at ticks while NYSE is open.
    s = oxb.Strategy(universe=["A"], clock="1h", weights=lambda s, h: Series({"A": h.now.hour / 100}),
                     risk=x.only("us"))  # fmt: skip
    r = oxb.backtest(s, T0, end(2 * 1440), root, CONFIG)
    assert r.fills and all(Moment(f.ts).open("us") for f in r.fills)
