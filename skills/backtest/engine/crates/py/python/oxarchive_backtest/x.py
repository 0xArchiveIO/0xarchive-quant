"""Pieces to build a strategy from: universes, clocks, weights, risk and execution. Each is
one small callable with one job, so swapping one is changing one line; a plain function with
the same arguments works anywhere a piece does.

    universe   ["BTC", "ETH"], top_n(25), rule(select)
    signal     blend(a, b): rank blend
    clock      "1h", every("7d", anchor="2025-03-22 01:00"), daily("08:00"),
               weekly("mon", at="08:00", tz="America/New_York"), monthly(-1),
               at_open("us", after="15m"), at_close("us", before="15m")
    weights    long_short(0.2, by=...), long_only(top=5), sign()
    risk       cap(0.1, gross=1.5), gross(1.0), book(gross=1, net=0), beta_neutral(),
               hedge("BTC"), vol_target(0.2), turnover(0.25), only("us")
    execute    rebalance(band=0.02, max_cost=0.001)
    exits      stop_loss(0.05), take_profit(0.1), trailing_stop(0.05), time_stop("3d"),
               drawdown_stop(0.2)
"""

from math import sqrt as _sqrt

from . import _native
from .series import Series
from .strategy import duration_ms, when

# our factor research's clock: 2025-03-22 00:00 UTC, the first clean hour of Hyperliquid candles.
STUDY_ANCHOR = "2025-03-22"


_WEEKDAYS = ("mon", "tue", "wed", "thu", "fri", "sat", "sun")


def _minute_of_day(at):
    """"08:00" or "8:30" as ms after midnight."""
    h, sep, m = str(at).partition(":")
    if not sep or not h.isdigit() or not m.isdigit() or int(h) > 23 or int(m) > 59:
        raise ValueError(f"at={at!r}: use HH:MM, like '08:00'")
    return (int(h) * 60 + int(m)) * 60_000


class _Clock:
    """When a strategy runs. Ticks are minute closes: a tick at 08:00 sees every bar that
    closed by 08:00, and its orders fill from the next minute."""

    def _spec(self):
        raise NotImplementedError

    def last_tick(self, t):
        """The latest tick at or before t (ms or a date)."""
        return _native._last_tick(self._spec(), when(t))

    def is_tick(self, t):
        return _native._is_tick(self._spec(), when(t))


class every(_Clock):
    """Ticks every `duration` from `anchor`."""

    def __init__(self, duration, anchor=0):
        self.every_ms = duration_ms(duration)
        self.anchor_ms = when(anchor)
        self._text = duration if isinstance(duration, str) else f"{self.every_ms}ms"

    def _spec(self):
        return {"kind": "every", "every_ms": self.every_ms, "anchor_ms": self.anchor_ms}

    def __repr__(self):
        return f"every({self._text!r}, anchor={self.anchor_ms})"


class _Calendar(_Clock):
    def __init__(self, days, day, at, tz):
        self._s = {"kind": "calendar", "days": days, "day": day, "at_ms": _minute_of_day(at),
                   "zone": tz}  # fmt: skip
        _native._last_tick(self._s, 0)  # checks the arguments now

    def _spec(self):
        return dict(self._s)

    def __repr__(self):
        s = self._s
        return f"{type(self).__name__}({s['days']}, day={s['day']}, at_ms={s['at_ms']}, tz={s['zone']!r})"


class daily(_Calendar):
    """Every day at `at` in `tz` (UTC, America/New_York, America/Chicago, Europe/London,
    Asia/Tokyo; daylight saving included); Monday to Friday only with weekdays=True."""

    def __init__(self, at="00:00", tz="UTC", weekdays=False):
        super().__init__("weekdays" if weekdays else "daily", 0, at, tz)


class weekly(_Calendar):
    """Once a week, on `day` ("mon" ... "sun", or 0 for Monday) at `at` in `tz`."""

    def __init__(self, day="mon", at="00:00", tz="UTC"):
        if isinstance(day, str):
            key = day.strip().lower()[:3]
            if key not in _WEEKDAYS:
                raise ValueError(f"day={day!r}: use mon, tue, wed, thu, fri, sat or sun")
            day = _WEEKDAYS.index(key)
        super().__init__("weekly", day, at, tz)


class monthly(_Calendar):
    """Once a month, on `day` (1 to 28, or -1 for the last day, -2 the one before) at `at`
    in `tz`."""

    def __init__(self, day=1, at="00:00", tz="UTC"):
        super().__init__("monthly", day, at, tz)


class _SessionEdge(_Clock):
    def __init__(self, edge, session, before, after):
        if before is not None and after is not None:
            raise ValueError("give before= or after=, not both")
        offset = -duration_ms(before) if before is not None else duration_ms(after or "0m")
        self._s = {"kind": "session", "session": session, "edge": edge, "offset_ms": offset}
        _native._last_tick(self._s, 0)

    def _spec(self):
        return dict(self._s)

    def __repr__(self):
        s = self._s
        return f"at_{s['edge']}({s['session']!r}, offset_ms={s['offset_ms']})"


class at_open(_SessionEdge):
    """At each open of an exchange's session ("us": NYSE with its holidays and early
    closes; "europe": London; "asia": Tokyo), or `after`/`before` it ("15m"). Only on
    days it trades."""

    def __init__(self, session="us", after=None, before=None):
        super().__init__("open", session, before, after)


class at_close(_SessionEdge):
    """At each close of an exchange's session, or `before`/`after` it."""

    def __init__(self, session="us", before=None, after=None):
        super().__init__("close", session, before, after)


def clock(c):
    """A clock from what a Strategy accepts: a clock piece, a duration ("1h": every hour
    from midnight UTC), or "daily", "weekly" (Monday) or "monthly" (the 1st), at 00:00
    UTC."""
    if isinstance(c, _Clock):
        return c
    named = {"daily": daily, "weekly": weekly, "monthly": monthly}
    if isinstance(c, str) and c.strip().lower() in named:
        return named[c.strip().lower()]()
    return every(c)


# Universes


class UniverseRule:
    """A point-in-time universe rule: picks are made on a schedule from data before each
    pick only, from every market the source holds, delisted ones included."""

    def _rule(self):
        raise NotImplementedError

    def _data_from(self):
        """How far back the picks read (ms), or None for all history."""
        return None

    def _columns(self):
        """The columns the picks read."""
        return ["close", "quote_volume", "open_interest", "funding_rate"]


class top_n(UniverseRule):
    """The `n` markets with the most dollar volume over the trailing `window`, among those
    with `min_history` of prices, re-picked every `every` from `anchor`. The defaults are
    our factor research's : top 25 by 30-day volume, 60 days of history, every
    30 days from 2025-03-22.

    Filters: by="open_interest" ranks by open interest in dollars at the pick instead;
    `min_volume` drops markets that traded less than that over the window; `exclude`
    leaves out a list of names; `sectors` ({market: sector}) with `per_sector` keeps at
    most that many of each sector."""

    def __init__(self, n=25, window="30d", min_history="60d", every="30d", anchor=STUDY_ANCHOR,
                 *, by="volume", min_volume=0.0, exclude=(), sectors=None, per_sector=0):  # fmt: skip
        hour = 3_600_000
        if by not in ("volume", "open_interest"):
            raise ValueError("top_n ranks by 'volume' or 'open_interest'")
        if per_sector and not sectors:
            raise ValueError("per_sector needs sectors={market: sector}")
        self.n = n
        self.window_hours = duration_ms(window) // hour
        self.min_history_hours = duration_ms(min_history) // hour
        self.every_hours = duration_ms(every) // hour
        self.anchor_ms = when(anchor)
        self.by, self.min_volume = by, float(min_volume)
        self.exclude = sorted(exclude)
        self.sectors, self.per_sector = dict(sectors or {}), per_sector

    def _rule(self):
        return {"kind": "top_n", "n": self.n, "window_hours": self.window_hours,
                "min_history_hours": self.min_history_hours, "every_hours": self.every_hours,
                "anchor_ms": self.anchor_ms, "by": self.by, "min_volume": self.min_volume,
                "exclude": self.exclude, "sectors": self.sectors,
                "per_sector": self.per_sector}  # fmt: skip

    def _columns(self):
        return ["close", "quote_volume"] + (["open_interest"] if self.by == "open_interest" else [])

    def _data_from(self):
        # History is counted from the anchor; the day before it seeds the carried price.
        return self.anchor_ms - 86_400_000

    def __repr__(self):
        return f"top_n({self.n})"


class rule(UniverseRule):
    """Your own universe rule: `select(view)` is called every `every` from `anchor` and
    returns the members in rank order (or None to keep the last pick). `view` (a PickView)
    holds each market's hourly data from before the pick only: view.markets, view.at,
    view.closes(m, hours), view.volume(m, hours), view.open_interest(m) (dollars),
    view.funding(m, hours) (mean rate), view.volatility(m, hours) (of hourly returns),
    view.history_hours(m), view.listed(m).

        def liquid(view):
            ok = [m for m in view.markets if view.history_hours(m) >= 60 * 24]
            return sorted(ok, key=lambda m: -view.volume(m, 30 * 24))[:10]

        universe = x.rule(liquid, every="7d")
    """

    def __init__(self, select, every="30d", anchor=STUDY_ANCHOR):
        self.select = select
        self.every_ms = duration_ms(every)
        self.anchor_ms = when(anchor)

    def _rule(self):
        return {"kind": "custom", "select": self.select, "every_ms": self.every_ms,
                "anchor_ms": self.anchor_ms}  # fmt: skip

    def __repr__(self):
        return f"rule({getattr(self.select, '__name__', 'select')})"


# Signals.


def blend(*scores, weights=None):
    """Signals combined by their cross-sectional ranks: the (weighted) mean of each one's
    pct rank, None where any is unknown. our factor research's rank blend is
    `blend(vwap_signal, mom7d)`."""
    if not scores:
        raise ValueError("blend takes at least one signal")
    w = list(weights) if weights is not None else [1.0] * len(scores)
    if len(w) != len(scores):
        raise ValueError("one weight per signal")
    total = 0.0
    for x in w:
        total += x
    out = None
    for s, wt in zip(scores, w):
        r = Series(s).rank() * (wt / total)
        out = r if out is None else out + r
    return out


# Weights: (scores, h) -> Series of target weights, a signed fraction of equity each.


def _known(scores):
    return Series() if scores is None else scores.dropna()


def _legs(legs, by):
    """Each (members, side) leg: weight `side` split across its members, by `by` (a Series)
    or equally. A market in two legs (a quantile of 0.5 or more) gets both, as our factor research's
    long leg less short leg does."""
    w = {}
    for members, side in legs:
        if not members:
            continue
        if by is None:
            for m in members:
                w[m] = w.get(m, 0.0) + side / len(members)
            continue
        total = 0.0
        for m in members:
            total += by[m]
        for m in members:
            w[m] = w.get(m, 0.0) + side * by[m] / total
    return Series(w)


class long_short:
    """Long the top `quantile` of scores and short the bottom, each leg half the book.
    `by` (a function of h, such as inverse volatility) weights each leg; without it the
    leg is equal weight. Ranks are pandas' pct ranks, so 25 markets at 0.2 give 6 longs
    and 5 shorts, as our factor research's quintiles do. Skips the tick with fewer than two scores."""

    def __init__(self, quantile=0.2, by=None, gross=1.0):
        self.quantile = quantile
        self.by = by
        self.gross = gross

    def __call__(self, scores, h):
        by = self.by(h) if self.by is not None else None
        s = _known(scores)
        if by is not None:
            s = s.where(by == by)
            s = s.dropna()
        if len(s) < 2:
            return None
        r = s.rank()
        longs = [m for m in s if r[m] >= 1 - self.quantile]
        shorts = [m for m in s if r[m] <= self.quantile]
        half = self.gross / 2
        return _legs(((longs, half), (shorts, -half)), by)


class long_only:
    """Long the `top` highest scores (or the top `quantile`), `by` weighting them as in
    long_short, the whole book long. Skips the tick with no scores."""

    def __init__(self, top=None, quantile=None, by=None, gross=1.0):
        if (top is None) == (quantile is None):
            raise ValueError("long_only takes top= or quantile=")
        self.top, self.quantile, self.by, self.gross = top, quantile, by, gross

    def __call__(self, scores, h):
        by = self.by(h) if self.by is not None else None
        s = _known(scores)
        if by is not None:
            s = s.where(by == by).dropna()
        if not len(s):
            return None
        if self.top is not None:
            members = list(s.top(self.top))
        else:
            r = s.rank()
            members = [m for m in s if r[m] >= 1 - self.quantile]
        return _legs(((members, self.gross),), by)


class sign:
    """`scale` long where the score is positive, `scale` short where negative (flat
    instead when short=False), flat where zero or unknown."""

    def __init__(self, scale=1.0, short=True):
        self.scale, self.short = scale, short

    def __call__(self, scores, h):
        if scores is None:
            return None

        def f(v):
            if v > 0:
                return self.scale
            if v < 0 and self.short:
                return -self.scale
            return 0.0

        return scores.fillna(0.0)._map(f)


# Risk: (weights, h, ctx) -> weights.


def _legs_of(w):
    long_, short = 0.0, 0.0
    for v in w.values():
        if v is None:
            continue
        if v > 0:
            long_ += v
        else:
            short -= v
    return long_, short


class cap:
    """Limits exposure, scaling down only: no market's weight beyond +-`market`, the sum of
    absolute weights at most `gross`, and the net (longs less shorts) within +-`net`, by
    scaling down the larger leg. `x.cap(0.1)` caps each market at 10% of equity."""

    def __init__(self, market=None, gross=None, net=None):
        if market is None and gross is None and net is None:
            raise ValueError("cap takes market=, gross= or net=")
        self.market, self.gross, self.net = market, gross, net

    def __call__(self, w, h, ctx):
        w = Series(w)
        if self.market is not None:
            w = w.clip(-self.market, self.market)
        if self.net is not None:
            long_, short = _legs_of(w)
            if long_ - short > self.net and long_ > 0:
                k = (short + self.net) / long_
                w = w._map(lambda v: v * k if v > 0 else v)
            elif short - long_ > self.net and short > 0:
                k = (long_ + self.net) / short
                w = w._map(lambda v: v * k if v < 0 else v)
        if self.gross is not None:
            total = abs(w).sum()
            if total > self.gross:
                w = w * (self.gross / total)
        return w


class gross:
    """Scales the weights so their absolute values sum to `target`."""

    def __init__(self, target=1.0):
        self.target = target

    def __call__(self, w, h, ctx):
        w = Series(w)
        total = abs(w).sum()
        return w if total == 0 else w * (self.target / total)


class book:
    """Sets the book's shape: `gross` (longs plus shorts) and `net` (longs less shorts), as
    fractions of equity, by scaling each leg. `book(gross=1, net=0)` is dollar neutral;
    leaving one out keeps the book's own. A leg with no members stays empty, so a
    long-only book can't be made neutral."""

    def __init__(self, gross=None, net=None):
        if gross is None and net is None:
            raise ValueError("book takes gross=, net= or both")
        self.gross, self.net = gross, net

    def __call__(self, w, h, ctx):
        w = Series(w)
        long_, short = _legs_of(w)
        g = long_ + short if self.gross is None else self.gross
        n = long_ - short if self.net is None else self.net
        want_long, want_short = (g + n) / 2, (g - n) / 2
        if long_ == 0 and short > 0:
            want_short = g
        if short == 0 and long_ > 0:
            want_long = g
        kl = want_long / long_ if long_ else 0.0
        ks = want_short / short if short else 0.0
        return w._map(lambda v: v * kl if v > 0 else v * ks)


class beta_neutral:
    """Scales the short leg so the book's beta to `to` (a market, or "universe") is zero,
    from each market's beta over `n`: sum(weight x beta) = 0. Markets with no beta are
    left out."""

    def __init__(self, to="BTC", n="60d"):
        self.to, self.n = to, n

    def __call__(self, w, h, ctx):
        w = Series(w)
        beta = h.close.beta(self.n, to=self.to)
        bl = bs = 0.0
        for m, v in w.items():
            b = beta.get(m)
            if v is None or b is None:
                continue
            if v > 0:
                bl += v * b
            else:
                bs -= v * b
        if bl == 0 or bs == 0:
            return w
        k = bl / bs
        return w._map(lambda v: v * k if v < 0 else v)


class hedge:
    """Adds a position in `to` (default BTC) that cancels the book's beta to it, from each
    market's beta over `n`. The hedge market is loaded and traded even when it isn't in the
    universe."""

    def __init__(self, to="BTC", n="60d"):
        self.to, self.n = to, n

    def __call__(self, w, h, ctx):
        w = Series(w)
        beta = h.close.beta(self.n, to=self.to)
        exposure = 0.0
        for m, v in w.items():
            b = 1.0 if m == self.to else beta.get(m)
            if v is not None and b is not None and m != self.to:
                exposure += v * b
        d = w.to_dict()
        d[self.to] = (d.get(self.to) or 0.0) - exposure
        return Series(d)


class vol_target:
    """Scales the book to an annualised volatility of `target`, estimated from what these
    weights would have returned over the last `n` bars (each market's one-bar returns,
    missing and bad prints as 0), at most `max_scale` times up. Correlations count, as the
    estimate is the book's own return series."""

    def __init__(self, target=0.2, n="30d", max_scale=3.0, clip=0.5):
        self.target, self.n, self.max_scale, self.clip = target, n, max_scale, clip

    def __call__(self, w, h, ctx):
        w = Series(w)
        closes = h.close.window(self.n)
        names = [m for m, v in w.items() if v]
        if not names:
            return w
        n = len(closes[names[0]]) - 1 if names[0] in closes else 0
        rets = [0.0] * n
        for m in names:
            c = closes.get(m)
            if c is None:
                continue
            wm = w[m]
            for i in range(n):
                r = 0.0
                if c[i] is not None and c[i + 1] is not None and c[i] != 0:
                    r = c[i + 1] / c[i] - 1
                    if self.clip is not None and abs(r) >= self.clip:
                        r = 0.0
                rets[i] += wm * r
        sd = Series(dict(enumerate(rets))).std()
        if not sd:
            return w
        per_year = 365 * 86_400_000 / h.bar_ms
        realised = sd * _sqrt(per_year)
        return w * min(self.max_scale, self.target / realised)


class turnover:
    """Trades at most `max` of equity per rebalance (the sum of weight changes): past that,
    every market moves the same fraction of the way from its current weight to its
    target."""

    def __init__(self, max=0.25):
        self.max = max

    def __call__(self, w, h, ctx):
        w = Series(w)
        eq = ctx.equity
        now = {}
        for m, pos in ctx.positions.items():
            px = ctx.price(m)
            if pos and px and eq:
                now[m] = pos * px / eq
        keys = list(dict.fromkeys(list(w.keys()) + list(now)))
        total = 0.0
        for m in keys:
            total += abs((w.get(m) or 0.0) - now.get(m, 0.0))
        if total <= self.max:
            return w
        k = self.max / total
        return Series({m: now.get(m, 0.0) + k * ((w.get(m) or 0.0) - now.get(m, 0.0)) for m in keys})


class only:
    """Trades only when `when` holds: a session name ("us", "europe", "asia": while it is
    open) or a function of h.now (a Moment), such as `lambda now: now.weekday < 5`.
    Outside it the tick is skipped and positions are kept (outside="hold"), or the book
    goes flat (outside="flat")."""

    def __init__(self, when, outside="hold"):
        if outside not in ("hold", "flat"):
            raise ValueError("outside is 'hold' or 'flat'")
        self.when, self.outside = when, outside

    def __call__(self, w, h, ctx):
        now = h.now
        ok = now.open(self.when) if isinstance(self.when, str) else self.when(now)
        if ok:
            return w
        return None if self.outside == "hold" else Series()


# Execution: (ctx, weights, h) -> places orders.


class rebalance:
    """Trades every market to its target weight with market orders: weight times
    `exposure` times equity, in coins at the last price. Markets not in the weights go
    flat, including any that left the universe. A market whose weight is within `band` of
    its target is left alone (`band=0.02`: only trade weights off by more than 2% of
    equity). `max_cost` skips the whole rebalance when the fill model's estimate of its
    fees and slippage is more than that fraction of equity."""

    def __init__(self, exposure=1.0, band=1e-6, max_cost=None):
        self.exposure, self.band, self.max_cost = exposure, band, max_cost

    def __call__(self, ctx, w, h):
        if self.max_cost is not None:
            if ctx.cost(w, self.exposure, self.band) > self.max_cost * ctx.equity:
                return
        ctx.rebalance(w, self.exposure, self.band)


# Exits, checked every minute by the run (oxs_engine::exits): Strategy(exits=[...]).


class _Exit:
    def __init__(self, kind, value):
        self._e = (kind, value)

    def __repr__(self):
        return f"{self._e[0]}({self._e[1]})"


class stop_loss(_Exit):
    """Closes a position once price moves `pct` against its entry (its average fill
    price), checked on each minute's high and low; the exit fills at the next minute's
    open."""

    def __init__(self, pct):
        super().__init__("stop_loss", pct)


class take_profit(_Exit):
    """Closes a position once price moves `pct` in its favour."""

    def __init__(self, pct):
        super().__init__("take_profit", pct)


class trailing_stop(_Exit):
    """Closes a position once price falls `pct` from its best since entry (rises, for a
    short)."""

    def __init__(self, pct):
        super().__init__("trailing_stop", pct)


class time_stop(_Exit):
    """Closes a position `after` its entry ("3d")."""

    def __init__(self, after):
        super().__init__("time_stop", duration_ms(after))


class drawdown_stop:
    """Cancels every order, closes every position and stops the strategy once equity falls
    `pct` from its peak: for good, or for `cooldown` ("7d"), after which it trades again
    and the peak starts afresh."""

    def __init__(self, pct, cooldown=None):
        self.pct = pct
        self.cooldown_ms = duration_ms(cooldown) if cooldown is not None else None

    def __repr__(self):
        return f"drawdown_stop({self.pct})"
