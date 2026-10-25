"""The strategy model: a clock plus a chain of steps, each optional and each swappable
(references/strategy-api.md in the skill).

    import oxarchive_backtest as oxb
    from oxarchive_backtest import x

    vol = lambda h: h.shift(1).close.vol("30d")

    strategy = oxb.Strategy(
        universe=x.top_n(25),
        clock=x.every("7d", anchor="2025-03-22 01:00"),
        signal=lambda h: h.close.ret("7d") / vol(h),
        weights=x.long_short(0.2, by=lambda h: 1 / vol(h)),
    )

At each tick: signal(h) -> scores, weights(scores, h) -> target weights (None skips the
tick), each risk(weights, h, ctx) -> weights, then execute(ctx, weights, h), which by
default trades to the weights (x.rebalance()). `step=lambda ctx, h: ...` replaces all four
for a strategy that places its own orders.

Before a run the strategy is checked once: its steps are called on an empty history and
what they read is recorded. That gives the run its warm-up, the columns to read and the
bars to keep, so a strategy declares none of it.
"""

import datetime as _dt
import inspect
import re

from ._native import _columns, _columns_for
from .series import Series

_UNITS = {"m": 60_000, "h": 3_600_000, "d": 86_400_000, "w": 604_800_000}
COLUMNS, FILL_COLUMNS = (tuple(c) for c in _columns())


def duration_ms(d):
    """A duration such as "15m", "6h", "30d" or "2w", in ms."""
    if isinstance(d, _dt.timedelta):
        return int(d.total_seconds() * 1000)
    if isinstance(d, str):
        m = re.fullmatch(r"\s*(\d+)\s*([mhdw])\s*", d)
        if m:
            return int(m.group(1)) * _UNITS[m.group(2)]
    raise ValueError(f"not a duration: {d!r} (use a number and m, h, d or w, like '30d')")


def when(t):
    """A time as Unix ms: ms itself (a number, or digits), a datetime (naive means UTC), or
    "2025-07-01" / "2025-07-01 01:00" in UTC."""
    if isinstance(t, bool):
        raise TypeError(f"not a time: {t!r}")
    if isinstance(t, int):
        return t
    if isinstance(t, str):
        if t.strip().lstrip("-").isdigit():
            return int(t)
        t = _dt.datetime.fromisoformat(t.strip().replace(" ", "T"))
    if isinstance(t, _dt.date) and not isinstance(t, _dt.datetime):
        t = _dt.datetime(t.year, t.month, t.day)
    if isinstance(t, _dt.datetime):
        if t.tzinfo is None:
            t = t.replace(tzinfo=_dt.timezone.utc)
        return int(t.timestamp() * 1000)
    raise TypeError(f"not a time: {t!r}")


def _bars_of(op, n, params):
    """Bars a read spans, counting the latest (oxs_engine::bars::Op::bars)."""
    if op == "last":
        return 1
    if op in ("ago", "ret", "logret", "vol", "beta", "corr"):
        return n + 1
    if op == "resid":
        return n + params["fit"] + 1
    if op == "seasonal":
        return n * params["period"] + 2 - params["ahead"]
    return n


def _bar_size(bars):
    ms = duration_ms(bars)
    if ms % 60_000 or 86_400_000 % ms and ms % 86_400_000:
        raise ValueError(f"bars={bars!r}: use a size that divides a day, or whole days")
    return ms


class History:
    """What a step reads: the last bars of every market in the universe, closed on their
    boundary. `h.close`, `h.quote_volume`, ... are fields; `h.shift(k)` is the same history
    as of k bars earlier; `h.bars("1d")` the same markets on other bars; `h.markets`,
    `h.time` (the latest bar's close, ms) and `h.now` (that time as a Moment)."""

    __slots__ = ("_b", "_shift", "_bar")

    def __init__(self, backend, shift=0, bar=None):
        self._b = backend
        self._shift = shift
        # None: the strategy's own bars.
        self._bar = bar

    @property
    def markets(self):
        return self._b.markets

    @property
    def time(self):
        t = self._b.time if self._bar is None else self._b.time_of(self._bar)
        return None if t is None else t - self._shift * self.bar_ms

    @property
    def bar_ms(self):
        return self._b.bar_ms if self._bar is None else self._bar

    @property
    def now(self):
        """`time` as a Moment: its calendar fields, exchange sessions and venue events
        (calendar.py). None before the first bar."""
        from .calendar import Moment

        t = self.time
        return None if t is None else Moment(t)

    def shift(self, k=1):
        """As of k bars earlier."""
        return History(self._b, self._shift + k, self._bar)

    def bars(self, size):
        """The same markets on bars of `size` ("1d", "4h", any size that divides a day or
        whole days), each kept from the minute rows on its own, so a daily close is the
        last minute's close of the UTC day. The latest is the last bar to close: at a
        13:00 tick, yesterday's daily bar."""
        ms = _bar_size(size)
        return History(self._b, 0, None if ms == self._b.bar_ms else ms)

    def field(self, name):
        if name not in COLUMNS:
            raise AttributeError(f"no column {name!r}; columns: {', '.join(COLUMNS)}")
        return Field(self, name)

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        return self.field(name)


class Field:
    """One column of the history. Each read returns a Series, one value per market; `n` is
    a number of bars or a duration ("7d"). All None until the run has seen enough bars."""

    __slots__ = ("_h", "_name")

    def __init__(self, h, name):
        self._h = h
        self._name = name

    def _n(self, n):
        if isinstance(n, int) and not isinstance(n, bool):
            if n < 1:
                raise ValueError(f"a window is at least one bar, got {n}")
            return n
        ms, bar = duration_ms(n), self._h.bar_ms
        if ms % bar:
            raise ValueError(f"{n!r} is not a whole number of {bar // 60_000}-minute bars")
        return ms // bar

    def _read(self, op, n=1, **params):
        h = self._h
        return Series._of(h._b.markets, h._b.read(op, self._name, n, h._shift, h._bar, **params))

    def last(self):
        """The latest bar's value."""
        return self._read("last")

    def ago(self, n):
        """The value n bars before the latest."""
        return self._read("ago", self._n(n))

    def ret(self, n):
        """The return over the last n bars: latest / n bars ago - 1."""
        return self._read("ret", self._n(n))

    def logret(self, n):
        """The log return over the last n bars: log(latest / n bars ago)."""
        return self._read("logret", self._n(n))

    def ewma(self, halflife, window=None):
        """The exponentially weighted mean of the last `window` bars (default ten
        half-lives, which leaves out under 0.1% of the weight), the weight halving every
        `halflife` back from the latest. Missing values are left out."""
        hl = self._n(halflife)
        n = self._n(window) if window is not None else 10 * hl
        return self._read("ewma", n, halflife=float(hl))

    def beta(self, n, to="BTC", clip=0.5):
        """Each market's beta to `to` (a market, or "universe" for the equal-weight mean of
        the universe's returns) over the last n one-bar returns: covariance over `to`'s
        variance. Returns count as in vol: missing and bad prints (`clip` or more) are 0.
        The run loads `to` even when it isn't in the universe."""
        return self._read("beta", self._n(n), to=to, clip=clip)

    def corr(self, n, to="BTC", clip=0.5):
        """Each market's correlation with `to` over the last n one-bar returns."""
        return self._read("corr", self._n(n), to=to, clip=clip)

    def resid(self, n, to="BTC", fit="60d", clip=0.5):
        """The sum of the last n residual returns against `to`: each bar's return less its
        beta times `to`'s, the beta fitted on the `fit` returns before that bar.
        our factor research's resid_mom is `resid("15d", fit="60d") / vol`."""
        return self._read("resid", self._n(n), to=to, fit=self._n(fit), clip=clip)

    def vol(self, n, clip=0.5):
        """Sample standard deviation of the last n one-bar returns. A missing return counts
        as 0, and so does a move of `clip` or more (a bad print; None turns that off)."""
        return self._read("vol", self._n(n), clip=clip)

    def mean(self, n):
        return self._read("mean", self._n(n))

    def sum(self, n):
        return self._read("sum", self._n(n))

    def std(self, n):
        return self._read("std", self._n(n))

    def min(self, n):
        return self._read("min", self._n(n))

    def max(self, n):
        return self._read("max", self._n(n))

    def seasonal(self, n, period="1d", ahead=1, clip=0.5):
        """The typical one-bar return at this point of the period: the mean, over the last
        n periods, of the return of the bar at the same point in `period` as the bar
        `ahead` bars from now. With bars="1h", period="1d" and ahead=1, the coming hour's
        mean return over the last n days (a signal: trade the hour); with ahead=0, the hour
        that just closed, from the days before (a control: `ret(1) - seasonal(n, ahead=0)`
        is the move net of its usual size). Returns of `clip` or more are skipped as bad
        prints."""
        p = self._n(period)
        if not 0 <= ahead < p:
            raise ValueError(f"seasonal: ahead is 0 to {p - 1} bars for a {period} period")
        return self._read("seasonal", n, clip=clip, period=p, ahead=ahead)

    def window(self, n):
        """The last n bars' values, oldest first: {market: [value, ...]}."""
        h, n = self._h, self._n(n)
        return dict(zip(h._b.markets, h._b.window(self._name, n, h._shift, h._bar)))


class _Probe:
    """A history with no bars that records what is read: the check before a run."""

    def __init__(self, markets, bar_ms):
        self.markets = list(markets)
        self.bar_ms = bar_ms
        self.time = 0
        # {bar_ms: {field: bars spanned}}
        self.reads = {}
        # Markets read against (beta, corr, resid).
        self.refs = set()

    def time_of(self, bar_ms):
        return 0

    def _note(self, bar_ms, field, span):
        r = self.reads.setdefault(bar_ms or self.bar_ms, {})
        r[field] = max(r.get(field, 0), span)

    def read(self, op, field, n, shift=0, bar_ms=None, **params):
        self._note(bar_ms, field, shift + _bars_of(op, n, params))
        to = params.get("to")
        if to is not None and to != "universe":
            self.refs.add(to)
        return [None] * len(self.markets)

    def window(self, field, n, shift=0, bar_ms=None):
        self._note(bar_ms, field, shift + n)
        return [[None] * n for _ in self.markets]


class _ProbeCtx:
    """A context that trades nothing, for checking a step."""

    def __init__(self, markets):
        self.time = 0
        self.equity = self.cash = 10_000.0
        self.markets = self.universe = list(markets)
        self.positions = {m: 0.0 for m in markets}
        self.open_orders = []
        self.state = {}
        self._internal = {}

    def price(self, market):
        return None

    def _noop(self, *a, **k):
        return None

    market = limit = stop = cancel = cancel_all = rebalance = _noop

    def cost(self, *a, **k):
        return 0.0


def _bad(msg):
    raise ValueError(msg)


def _uniform(seed, t, key):
    """A number in [0, 1) from (seed, t, key) alone: splitmix64 over integers, so every
    platform draws the same."""
    h = 0xCBF29CE484222325
    for b in str(key).encode():
        h = ((h ^ b) * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    z = (seed * 0x9E3779B97F4A7C15 + t * 0xBF58476D1CE4E5B9 + h) & 0xFFFFFFFFFFFFFFFF
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    z ^= z >> 31
    return (z >> 11) / 9007199254740992.0


def _bucket(every):
    """t, or the start of its `every`-long bucket."""
    if every is None:
        return lambda t: t
    ms = every if isinstance(every, int) else duration_ms(every)
    return lambda t: t - t % ms


def _adapt(fn, n):
    """`fn` called with the first arguments it takes, of n."""
    if fn is None:
        return None
    try:
        params = inspect.signature(fn).parameters.values()
    except (TypeError, ValueError):
        return fn
    if any(p.kind is p.VAR_POSITIONAL for p in params):
        return fn
    k = sum(1 for p in params if p.kind in (p.POSITIONAL_ONLY, p.POSITIONAL_OR_KEYWORD)
            and p.default is p.empty)  # fmt: skip
    k = max(1, min(k, n)) if n else 0
    if k == n:
        return fn
    return lambda *a: fn(*a[:k])


class Strategy:
    """A strategy: which markets (`universe`), when (`clock`), and what to do at each tick
    (`signal`, `weights`, `risk`, `execute`, or one `step`). See the module docstring.

    universe  a list of market names, or a rule (`x.top_n`, `x.rule`)
    clock     "1m", "1h", "1d"..., "daily", "weekly", "monthly", or a clock piece
              (`x.every`, `x.weekly("mon", at="08:00")`, `x.at_close("us")`...); default "1h"
    delay     trade this long after each tick instead of at it ("2h"); the weights are
              worked out at the tick
    hold      re-trade to the last target weights this often between ticks ("1h"), so
              they stay fixed instead of drifting with prices
    bars      the bar size `h` holds, default "1h"
    carry     how long a missing price carries forward, default "6h"
    on_fill   called with (ctx, fill) for every fill
    on_universe  called with (ctx, joined, left), lists of market names, when the
              universe's members change
    exits     checked every minute whatever the clock: x.stop_loss, x.take_profit,
              x.trailing_stop, x.time_stop per position, x.drawdown_stop for the book
    repair    drop bad prints as bars close: a price more than this many times (True: 2.5,
              our factor research's) from the median close of the 48 bars before it becomes
              missing, so the close carries forward. Off by default
    history   the longest read, if the check can't see it (a duration)
    fields    columns to keep, if the check can't see them
    """

    def __init__(self, *, universe, clock="1h", bars="1h", carry="6h", signal=None,
                 weights=None, risk=None, execute=None, step=None, on_fill=None,
                 on_universe=None, exits=None, delay=None, hold=None, repair=None,
                 history=None, fields=None):  # fmt: skip
        from . import x

        self._args = dict(universe=universe, clock=clock, bars=bars, carry=carry, signal=signal,
                          weights=weights, risk=risk, execute=execute, step=step,
                          on_fill=on_fill, on_universe=on_universe, exits=exits, delay=delay,
                          hold=hold, repair=repair, history=history, fields=fields)  # fmt: skip
        if step is not None and any(v is not None for v in (signal, weights, risk, execute)):
            raise ValueError("step= replaces signal, weights, risk and execute; give one or the other")
        if step is None and weights is None:
            raise ValueError("give weights= (scores to target weights) or step= (your own orders)")
        if isinstance(universe, str):
            universe = [universe]
        if isinstance(universe, (list, tuple)):
            if not universe or not all(isinstance(m, str) for m in universe):
                raise ValueError("universe is a list of market names, or a rule such as x.top_n(25)")
            universe = list(universe)
        elif not isinstance(universe, x.UniverseRule):
            raise ValueError("universe is a list of market names, or a rule such as x.top_n(25)")
        self.universe = universe
        self.clock = x.clock(clock)
        if step is not None and (delay is not None or hold is not None):
            raise ValueError("delay= and hold= act on target weights; a step places its own orders")
        self.delay_ms = duration_ms(delay) if delay is not None else 0
        self.hold = x.clock(hold) if hold is not None else None
        self.bar_ms = _bar_size(bars)
        self.carry_ms = duration_ms(carry)
        self.carry_bars = self.carry_ms // self.bar_ms
        if repair is True:
            repair = 2.5
        if repair is not None and (repair is False or not repair > 1):
            repair = None if repair is False else _bad(f"repair={repair!r}: a ratio above 1, or True")
        self.repair = repair
        self.signal = signal
        self.weights = weights
        self.risk = list(risk) if isinstance(risk, (list, tuple)) else [] if risk is None else [risk]
        self.execute = execute if execute is not None else (None if step else x.rebalance())
        self.step = step
        self.on_fill = on_fill
        self.on_universe = on_universe
        self.exits = list(exits or ())
        for e in self.exits:
            if not isinstance(e, (x._Exit, x.drawdown_stop)):
                raise ValueError(f"exits: {e!r} is not an exit (x.stop_loss, x.drawdown_stop, ...)")
        if sum(isinstance(e, x.drawdown_stop) for e in self.exits) > 1:
            raise ValueError("exits: one drawdown_stop")
        self.history_ms = duration_ms(history) if history is not None else 0
        for f in fields or ():
            if f not in COLUMNS:
                raise ValueError(f"fields: no column {f!r}")
        self.fields = list(fields or ())
        self._signal = _adapt(signal, 1)
        self._weights = _adapt(weights, 2)
        self._risk = [_adapt(r, 3) for r in self.risk]
        self._execute = _adapt(self.execute, 3)
        self._step = _adapt(step, 2)
        self._on_fill = _adapt(on_fill, 2)
        self._has_fill = on_fill is not None
        self._on_universe = _adapt(on_universe, 3)
        self._has_universe = on_universe is not None
        self._checked = None

    def replace(self, **changes):
        """A copy with some arguments changed: `s.replace(clock="1d")`,
        `s.replace(signal=other)`."""
        unknown = set(changes) - set(self._args)
        if unknown:
            raise TypeError(f"replace: no argument {', '.join(sorted(unknown))}")
        return Strategy(**{**self._args, **changes})

    def _signal_of(self, what):
        if self._signal is None:
            raise ValueError(f"{what} needs a strategy with a signal")
        return self._signal

    def flipped(self):
        """The same strategy on the negated signal: our factor research's sign check. A real signal
        should do clearly worse flipped; costs make both lose a little."""
        sig = self._signal_of("flipped()")
        return self.replace(signal=lambda h: -sig(h))

    def random_signal(self, seed=0, every=None):
        """The same strategy on random scores, uniform in [-1, 1) and drawn afresh per tick
        and market from `seed`: our factor research's construction-only null. Whatever this earns is
        the machinery's (universe, weighting, costs), not the signal's. `every` (a duration)
        draws afresh only once per that much time instead, so the null trades about as often
        as a strategy that ticks often but holds for longer."""
        self._signal_of("random_signal()")
        bucket = _bucket(every)
        return self.replace(signal=lambda h: Series(
            {m: _uniform(seed, bucket(h.time or 0), m) * 2 - 1 for m in h.markets}))  # fmt: skip

    def stale_signal(self, seed=0, within="30d", every=None):
        """The same strategy on its own signal as of a random earlier time, up to `within`
        back, drawn afresh per tick from `seed`: the scores keep their spread and
        persistence but lose their timing. A signal that only works on time does
        clearly better than this. `every` keeps each lag for that much time, so the
        stale scores move as often as the real ones."""
        sig = self._signal_of("stale_signal()")
        top = duration_ms(within)
        bucket = _bucket(every)

        def stale(h):
            bars = top // h.bar_ms
            # The check before a run sees the longest lag.
            k = bars if not h.time else 1 + int(_uniform(seed, bucket(h.time), "") * bars)
            return sig(h.shift(min(k, bars)))

        return self.replace(signal=stale)

    # Called by the run.

    def _tick(self, ctx, native, tick=True, act=False, hold=False):
        """One call from the run: at a clock tick (`tick`), `delay` after one (`act`), or at
        a `hold` tick between them."""
        h = History(native)
        if self._step is not None:
            self._step(ctx, h)
            return
        mem = ctx._internal
        if act:
            w = mem.pop("pending", None)
            if w is not None:
                self._execute(ctx, w, h)
                mem["held"] = w
        if tick:
            w = self._target(ctx, h)
            if w is not None:
                if self.delay_ms:
                    mem["pending"] = w
                else:
                    self._execute(ctx, w, h)
                    mem["held"] = w
        if hold and mem.get("held") is not None:
            self._execute(ctx, mem["held"], h)

    def _target(self, ctx, h):
        """Target weights now: signal, weights, then each risk step; None skips."""
        scores = self._signal(h) if self._signal is not None else None
        w = self._weights(scores, h)
        for r in self._risk:
            if w is None:
                return None
            w = r(w, h, ctx)
        return w

    def _probe(self, ctx, h):
        """Every step once, as the check before a run: a step that returns None (as most
        do on a history with no bars) hands the next one zero weights, so each is seen."""
        if self._step is not None:
            self._step(ctx, h)
            return
        zero = Series({m: 0.0 for m in h.markets})
        scores = self._signal(h) if self._signal is not None else None
        w = self._weights(scores, h)
        for r in self._risk:
            w = r(zero if w is None else w, h, ctx)
        self._execute(ctx, zero if w is None else w, h)

    def _fill(self, ctx, fill):
        self._on_fill(ctx, fill)

    def _universe(self, ctx, joined, left):
        self._on_universe(ctx, joined, left)

    # The check before a run.

    def _check(self):
        """What the strategy reads, from one tick on an empty history: ({bar_ms: {field:
        bars spanned}}, markets read against). The strategy's own bar size is always
        there."""
        if self._checked is None:
            markets = self.universe if isinstance(self.universe, list) else ["_a", "_b", "_c"]
            probe = _Probe(markets, self.bar_ms)
            try:
                self._probe(_ProbeCtx(markets), History(probe))
            except Exception as e:
                if not (self.history_ms and self.fields):
                    raise ValueError(
                        "couldn't check what the strategy reads: on a history with no bars "
                        f"yet it raised {type(e).__name__}: {e}. Make the steps return early "
                        "on None values, or declare history= and fields= on the Strategy"
                    ) from e
            reads = {b: dict(r) for b, r in probe.reads.items()}
            own = reads.setdefault(self.bar_ms, {})
            for f in self.fields:
                own.setdefault(f, 1)
            if self.history_ms:
                span = -(-self.history_ms // self.bar_ms)
                for f in own or {"close": 1}:
                    own[f] = max(own.get(f, 1), span)
            if self.repair:
                for r in reads.values():
                    r.setdefault("close", 1)
            self._checked = (reads, sorted(probe.refs))
        return self._checked

    def _histories(self):
        """One history per bar size read, the strategy's own first: its bar size, fields,
        bars kept and carry."""
        reads, _ = self._check()
        out = []
        for bar in [self.bar_ms] + sorted(b for b in reads if b != self.bar_ms):
            r = reads[bar]
            span = max(r.values(), default=1)
            if self.repair:
                span += 48  # the repair rule's median window, oxs_engine::bars::REPAIR_WINDOW
            carry = self.carry_ms // bar
            out.append({"bar_ms": bar, "fields": sorted(r), "carry_bars": carry,
                        "capacity": span + carry + 1, "span": span})  # fmt: skip
        return out

    @property
    def warmup_ms(self):
        """How much history the strategy reads before its first tick: its longest read, on
        any bar size, and the carry behind it."""
        return max((h["span"] + h["carry_bars"]) * h["bar_ms"] for h in self._histories())

    def warm_from(self, start_ms):
        """Where minute history must start for a first tick at `start_ms`: the warm-up
        before it, on the grid of the largest bar size read."""
        grid = max(h["bar_ms"] for h in self._histories())
        return (start_ms - self.warmup_ms) // grid * grid

    @property
    def references(self):
        """Markets the strategy reads against (beta, corr, resid), which a run loads even
        when they aren't in the universe."""
        return self._check()[1]

    def _columns(self):
        """The minute-row columns a run reads: the fields and what the fill model needs."""
        fields = sorted({f for h in self._histories() for f in h["fields"]})
        cols = list(FILL_COLUMNS)
        for c in _columns_for(fields):
            if c not in cols:
                cols.append(c)
        return cols

    def _spec(self, start_ms):
        from . import x

        return {
            "histories": [{k: h[k] for k in ("bar_ms", "fields", "carry_bars", "capacity")}
                          for h in self._histories()],  # fmt: skip
            "repair": self.repair,
            "exits": [e._e for e in self.exits if not isinstance(e, x.drawdown_stop)],
            "drawdown": next(((e.pct, e.cooldown_ms) for e in self.exits
                              if isinstance(e, x.drawdown_stop)), None),  # fmt: skip
            "clock": self.clock._spec(),
            "hold": None if self.hold is None else self.hold._spec(),
            "delay_ms": self.delay_ms,
            "start": start_ms,
        }

    def _functions(self):
        """The Python functions this strategy calls, for the determinism check."""
        import types

        out, seen = [], set()

        def walk(v, depth):
            if id(v) in seen or depth > 3:
                return
            seen.add(id(v))
            if isinstance(v, types.FunctionType):
                out.append(v)
                for c in v.__closure__ or ():
                    try:
                        walk(c.cell_contents, depth + 1)
                    except ValueError:
                        pass
            elif isinstance(v, (list, tuple)):
                for i in v:
                    walk(i, depth + 1)
            elif hasattr(v, "__dict__") and type(v).__module__.startswith("oxarchive_backtest"):
                for i in vars(v).values():
                    walk(i, depth + 1)
            elif hasattr(v, "__dict__") and callable(v):
                out.extend(f for f in vars(type(v)).values() if isinstance(f, types.FunctionType))

        for v in (self.universe, self.signal, self.weights, self.risk, self.execute, self.step,
                  self.on_fill, self.on_universe):  # fmt: skip
            walk(v, 0)
        return out

    def __repr__(self):
        u = self.universe if isinstance(self.universe, list) else self.universe
        return f"Strategy(universe={u!r}, clock={self.clock!r})"
