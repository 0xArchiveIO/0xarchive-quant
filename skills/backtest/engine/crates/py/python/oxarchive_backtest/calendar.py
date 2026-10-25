"""Time as a strategy reads it: calendar fields, exchange sessions and venue events, for
seasonality signals (trade the hour), filters (only in US hours) and controls.

    now = h.now                      # the latest bar's close, as a Moment
    now.hour, now.weekday            # UTC; weekday 0 is Monday
    now.local("America/New_York").hour
    now.open("us")                   # NYSE's regular session, with its holidays
    now.minutes_to_close("us")       # None while closed
    now.expiry                       # "quarterly", "monthly", "weekly" or "daily"

Zones, sessions and the NYSE calendar are the engine's (oxs_engine::calendar), written out
rather than read from the platform, so every machine agrees. Sessions:
"us" is NYSE 09:30 to 16:00 New York time (13:00 on early closes) with NYSE's holidays;
"europe" is London 08:00 to 16:30 and "asia" Tokyo 09:00 to 15:30 with its lunch break,
weekdays only: their holidays aren't listed.
"""

import calendar as _cal

from . import _native
from .strategy import when

MIN = 60_000
HOUR = 3_600_000
DAY = 86_400_000


class Local:
    """A wall clock: year, month, day, hour, minute, weekday (0 = Monday)."""

    __slots__ = ("year", "month", "day", "weekday", "hour", "minute", "_days")

    def __init__(self, t, zone):
        y, mo, d, wd, ms, days = _native._local(t, zone)
        self.year, self.month, self.day, self.weekday = y, mo, d, wd
        self.hour, self.minute = ms // HOUR, ms % HOUR // MIN
        self._days = days

    def __repr__(self):
        return (f"{self.year:04d}-{self.month:02d}-{self.day:02d} "
                f"{self.hour:02d}:{self.minute:02d}")  # fmt: skip


def _days_in_month(y, m):
    return _cal.monthrange(y, m)[1]


def _last_friday(y, m):
    """The day of the month of its last Friday."""
    n = _days_in_month(y, m)
    return n - (_cal.weekday(y, m, n) - 4) % 7


class Moment:
    """A moment (Unix ms) read as a calendar. Fields without a zone are UTC."""

    __slots__ = ("t", "_utc")

    def __init__(self, t):
        self.t = when(t)
        self._utc = Local(self.t, "UTC")

    def __repr__(self):
        return f"Moment({self._utc} UTC)"

    # UTC calendar fields.

    @property
    def year(self):
        return self._utc.year

    @property
    def month(self):
        return self._utc.month

    @property
    def day(self):
        return self._utc.day

    @property
    def hour(self):
        return self._utc.hour

    @property
    def minute(self):
        return self._utc.minute

    @property
    def weekday(self):
        """0 = Monday ... 6 = Sunday."""
        return self._utc.weekday

    @property
    def weekend(self):
        return self._utc.weekday >= 5

    @property
    def quarter(self):
        return (self._utc.month - 1) // 3 + 1

    @property
    def days_to_month_end(self):
        """0 on the last day of the month."""
        u = self._utc
        return _days_in_month(u.year, u.month) - u.day

    @property
    def days_to_quarter_end(self):
        """0 on the last day of March, June, September and December."""
        u = self._utc
        end_month = self.quarter * 3
        days = self.days_to_month_end
        for m in range(u.month + 1, end_month + 1):
            days += _days_in_month(u.year, m)
        return days

    def local(self, zone):
        """The wall clock in `zone`: UTC, America/New_York, America/Chicago,
        Europe/London or Asia/Tokyo."""
        return Local(self.t, zone)

    # Exchange sessions.

    def open(self, session="us"):
        """Whether `session` is open at this moment (open <= t < close)."""
        return _native._session(session, self.t)[0] is not None

    def minutes_since_open(self, session="us"):
        """Minutes since the open of the session in progress; None while closed."""
        cur = _native._session(session, self.t)[0]
        return None if cur is None else (self.t - cur[0]) // MIN

    def minutes_to_close(self, session="us"):
        """Minutes to the close of the session in progress; None while closed."""
        cur = _native._session(session, self.t)[0]
        return None if cur is None else (cur[1] - self.t) // MIN

    def minutes_to_open(self, session="us"):
        """Minutes to the next open (within 30 days); 0 never: while open, the next one."""
        nxt = _native._session(session, self.t)[1]
        return None if nxt is None else (nxt - self.t) // MIN

    def minutes_since_close(self, session="us"):
        """Minutes since the last close (within 30 days)."""
        last = _native._session(session, self.t)[2]
        return None if last is None else (self.t - last) // MIN

    def holiday(self, session="us"):
        """Why the exchange is shut today (its zone's date), or None. Only "us" has a
        holiday list."""
        return _native._session(session, self.t)[3]

    # Venue events.

    @property
    def minutes_to_funding(self):
        """Minutes to Hyperliquid's next funding settlement, which is on the hour; 0 at
        the hour itself."""
        return (60 - self._utc.minute) % 60

    @property
    def expiry(self):
        """Today's crypto options expiry at 08:00 UTC, by Deribit's schedule: "quarterly"
        (the last Friday of March, June, September and December), "monthly" (the last
        Friday of other months), "weekly" (other Fridays) or "daily"."""
        u = self._utc
        if u.weekday != 4:
            return "daily"
        if u.day != _last_friday(u.year, u.month):
            return "weekly"
        return "quarterly" if u.month % 3 == 0 else "monthly"

    def hours_to_expiry(self, kind="weekly"):
        """Hours to the next 08:00 UTC expiry of `kind` or larger ("daily", "weekly",
        "monthly", "quarterly"); 0 at the expiry itself."""
        order = ("daily", "weekly", "monthly", "quarterly")
        if kind not in order:
            raise ValueError(f"kind is one of {', '.join(order)}")
        day0 = self.t - self.t % DAY
        for k in range(0, 100):
            at = day0 + k * DAY + 8 * HOUR
            if at >= self.t and order.index(Moment(at).expiry) >= order.index(kind):
                return (at - self.t) / HOUR
        return None


def at(t):
    """A Moment at t (ms or a date)."""
    return Moment(t)
