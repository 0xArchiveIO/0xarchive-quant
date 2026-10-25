"""Running a strategy: over history (`backtest`), on rows as they arrive (`Session`), or
over a grid of parameters (`sweep`). A run works out its own data: which markets (the
universe over the window), which months (the window plus the strategy's warm-up) and
which columns (what the strategy reads plus what the fill model needs).

    report = oxb.backtest(strategy, "2025-07-01", "2025-10-01", source="data/hyperliquid")
    report.stats, report.equity, report.receipt(source_text)
"""

import calendar
import datetime as _dt
import itertools
import os

from . import _native
from . import study as _study
from .strategy import Strategy, when


class MissingData(Exception):
    """The run needs data the source doesn't have yet: `files` lists it as (path, hourly,
    columns): a path relative to the source's root (minute files) or hourly root
    (`hourly=True`), and the columns the run reads from it (None: all). A remote reader fetches
    those columns' bytes (`_native._column_ranges`) and runs again."""

    def __init__(self, files):
        self.files = files
        super().__init__(f"{len(files)} data file(s) missing, e.g. {files[0][0]}")


def _month(ms):
    d = _dt.datetime.fromtimestamp(ms / 1000, _dt.timezone.utc)
    return f"{d.year:04d}-{d.month:02d}"


def _month_range(t0, t1):
    """The months that overlap [t0, t1)."""
    if t1 <= t0:
        return []
    out, (y, m) = [], map(int, _month(t0).split("-"))
    last = _month(t1 - 1)
    while True:
        out.append(f"{y:04d}-{m:02d}")
        if out[-1] >= last:
            return out
        y, m = (y + 1, 1) if m == 12 else (y, m + 1)


def month_start(month):
    y, m = map(int, month.split("-"))
    return calendar.timegm((y, m, 1, 0, 0, 0)) * 1000


class Source:
    """Where a run reads data: `root`/<MARKET>/<YYYY-MM>.parquet minute files and, for
    universe rules, `hourly` (default `root` + "-1h") with the same layout of hourly bars
    (`0xbt hourly` writes them; without them a rule reads the minute files). `index`
    ({market: [months]}) lists what exists when the files aren't all on disk yet; a run that needs one that isn't raises MissingData. `have` ({"minute/<path>"
    or "hourly/<path>": [columns]}) lists files on disk that hold only some columns' bytes
    (fetched by range); a file not in it is whole."""

    def __init__(self, root, hourly=None, index=None, hourly_index=None, have=None):
        self.root = str(root).rstrip("/")
        self.hourly_root = str(hourly).rstrip("/") if hourly else self.root + "-1h"
        self._index = index
        self._hourly_index = hourly_index
        self._have = {k: set(v) for k, v in (have or {}).items()}

    @staticmethod
    def _scan(root):
        out = {}
        if not os.path.isdir(root):
            return out
        for m in sorted(os.listdir(root)):
            d = os.path.join(root, m)
            if m.startswith(".") or not os.path.isdir(d):
                continue
            months = sorted(f[:-8] for f in os.listdir(d) if f.endswith(".parquet"))
            if months:
                out[m] = months
        return out

    def index(self, hourly=False):
        if hourly:
            if self._hourly_index is None:
                self._hourly_index = self._scan(self.hourly_root)
            return self._hourly_index
        if self._index is None:
            self._index = self._scan(self.root)
        return self._index

    def markets(self):
        return sorted(self.index())

    def has_hourly(self):
        return bool(self.index(hourly=True))

    def datasets(self, markets, t0, t1, columns=None, hourly=False):
        """[(market, Dataset)] over the months overlapping [t0, t1), streamed. Raises
        MissingData listing every file that's listed but not on disk, and Sealed when an
        active study's holdout is sealed and the range reaches into it (study.py)."""
        st = _study.active()
        if st is not None:
            st.guard(t1)
        root, idx = (self.hourly_root, self.index(True)) if hourly else (self.root, self.index())
        want, missing = [], []
        span = set(_month_range(t0, t1))
        for m in markets:
            if m not in idx:
                raise ValueError(f"the source has no market {m!r}")
            months = [x for x in idx[m] if x in span]
            want.append((m, months))
            for mo in months:
                rel = f"{m}/{mo}.parquet"
                part = self._have.get(f"{'hourly' if hourly else 'minute'}/{rel}")
                if not os.path.exists(os.path.join(root, rel)) or (
                    part is not None and (columns is None or not set(columns) <= part | {"ts"})
                ):
                    missing.append((rel, hourly, None if columns is None else list(columns)))
        if missing:
            raise MissingData(missing)
        return [
            (m, _native.load(os.path.join(root, m), columns=columns, stream=True,
                             months=(months[0], months[-1]) if months else ("0", "0")))
            for m, months in want
        ]  # fmt: skip

    def rows(self, market, start, end, columns=None):
        """One market's minute rows over [start, end) as dicts, for research outside a
        strategy (`pandas.DataFrame(src.rows(...))`). Reads through the same checks as a
        run, a study's seal included; `columns` limits what is decoded."""
        t0, t1 = when(start), when(end)
        ((_, ds),) = self.datasets([market], t0, t1, columns=columns)
        return [r for r in ds.rows() if t0 <= r["ts"] < t1]

    def __repr__(self):
        return f"Source({self.root!r})"


def _source(source):
    return source if isinstance(source, Source) else Source(source)


def _strategy(strategy, params):
    if isinstance(strategy, Strategy):
        if params:
            raise ValueError("params need a function that returns the Strategy")
        return strategy
    if callable(strategy):
        s = strategy(**(params or {}))
        if not isinstance(s, Strategy):
            raise TypeError("the strategy function must return an oxb.Strategy")
        return s
    raise TypeError("strategy is an oxb.Strategy, or a function returning one")


def _files(sets):
    out = []
    for _, ds in sets:
        out.extend(ds.files)
    return out


def picks(strategy, start, end, source="data/hyperliquid"):
    """The universe a backtest of `strategy` over [start, end) would use: its rule's picks
    from every market of `source`. None for a fixed list of markets."""
    rule = strategy.universe
    if isinstance(rule, list):
        return None, []
    src = _source(source)
    t1 = when(end)
    frm = rule._data_from()
    hourly = src.has_hourly()
    idx = src.index(hourly)
    first = min(month_start(v[0]) for v in idx.values())
    t0 = first if frm is None else max(first, frm)
    sets = src.datasets(sorted(idx), t0, t1, columns=rule._columns(), hourly=hourly)
    return _native._picks(sets, rule._rule(), t1), _files(sets)


def backtest(strategy, start, end, source="data/hyperliquid", config=None, *,
             max_leverage=None, params=None, benchmark=None):  # fmt: skip
    """Runs `strategy` (a Strategy, or a function returning one, called with `params`)
    over [start, end) and returns a Report. Times are ms, dates or "2025-07-01"; `source`
    is a directory or a Source. The strategy trades from `start`; the data before it, as
    far back as the strategy reads, only warms its history. `max_leverage` overrides a
    market's limit by name, e.g. {"ETH": 10}. `benchmark` also runs "BTC" (or any market:
    holding it), "equal" (the strategy's universe, equal weight, on its clock) or another
    Strategy over the same window, as `report.benchmark`, with `report.vs` comparing."""
    from ._determinism import warn_determinism

    s = _strategy(strategy, params)
    src = _source(source)
    t0, t1 = when(start), when(end)
    if t1 <= t0:
        raise ValueError("end must be after start")
    bench = None
    if benchmark is not None:
        from .evaluate import benchmark_of

        with _study.kind("check"):
            bench = backtest(benchmark_of(benchmark, s), t0, t1, src, config,
                             max_leverage=max_leverage)  # fmt: skip
    spec = s._spec(t0)
    frm = s.warm_from(t0)
    universe, extra = picks(s, t0, t1, src)
    if universe is None:
        markets, members = sorted(set(s.universe)), s.universe
    else:
        markets, members = universe.markets_between(t0, t1), None
        if not markets:
            raise ValueError("the universe has no members between start and end")
    # Markets read against (a beta to BTC) are loaded, and traded only if in the universe.
    markets = sorted(set(markets) | set(s.references))
    sets = src.datasets(markets, frm, t1, columns=s._columns())
    warn_determinism(s)
    report = _native._backtest(s, sets, spec, frm, t1, config, max_leverage, members=members,
                               universe=universe, extra_files=extra, benchmark=bench)  # fmt: skip
    # An active study's ledger gets every run (study.py).
    st = _study.active()
    if st is not None:
        st.record(report, t0, t1, params)
    return report


class Session:
    """Runs a strategy on rows as they arrive, one minute at a time: paper trading on the
    live stream, with the backtest's loop and fill model.

        s = oxb.Session(strategy, markets=["BTC", "ETH"], start=now)
        s.warm_up("data/hyperliquid")                 # history before start, then up to now
        s.step({"BTC": {"ts": ..., "close": ...}, "ETH": {...}})   # each minute
        s.report()

    `markets` lists every market the session is fed (a dict maps each name to its dataset
    market, "hyperliquid:BTC", which sets its default leverage). A universe rule picks from
    them as the session reaches each pick time. The strategy trades from `start` (ms or a
    date; default the first row); earlier rows only warm up.
    """

    def __init__(self, strategy, markets, start=None, config=None, *, max_leverage=None,
                 params=None):  # fmt: skip
        from ._determinism import warn_determinism

        self.strategy = s = _strategy(strategy, params)
        if isinstance(markets, dict):
            pairs = sorted(markets.items())
        else:
            pairs = [(m, f"hyperliquid:{m}") for m in sorted(set(markets))]
        self.start = when(start) if start is not None else -(2**62)
        missing = [m for m in s.references if m not in dict(pairs)]
        if missing:
            raise ValueError(f"the strategy reads against {', '.join(missing)}; feed it to the session")
        rule = None if isinstance(s.universe, list) else s.universe._rule()
        members = s.universe if isinstance(s.universe, list) else None
        warn_determinism(s)
        self._s = _native._Session(s, pairs, s._spec(self.start), config, max_leverage,
                                   members=members, rule=rule)  # fmt: skip

    @property
    def warmup_from(self):
        """Where minute history must start for the strategy's first tick at `start`."""
        return self.strategy.warm_from(self.start)

    def warm_up(self, source, until=None):
        """Brings the session up to date from `source` (a directory or Source): a universe
        rule's hourly history first, then minute rows from `warmup_from` to `until`.
        Returns the minutes replayed."""
        src = _source(source)
        markets = self.markets
        frm = self.warmup_from
        rule = self.strategy.universe
        if not isinstance(rule, list):
            hourly = src.has_hourly()
            idx = src.index(hourly)
            have = [m for m in markets if m in idx]
            first = min(month_start(idx[m][0]) for m in have) if have else frm
            r0 = first if rule._data_from() is None else max(first, rule._data_from())
            sets = src.datasets(have, r0, frm, columns=rule._columns(), hourly=hourly)
            # Only the hours before the minute replay.
            self._s.preload_hourly(sets, frm)
        have = [m for m in markets if m in src.index()]
        end = when(until) if until is not None else 2**62
        sets = src.datasets(have, frm, min(end, 2**62), columns=self.strategy._columns())
        return self.replay(sets, until=until)

    def replay(self, data, until=None):
        """Steps stored rows: `data` is a list of (market, Dataset) or a dict, for some or
        all markets. Only minutes after the last one stepped, and before `until`."""
        items = sorted(data.items()) if isinstance(data, dict) else list(data)
        return self._s.replay(items, None if until is None else when(until))

    def step(self, rows):
        """One closed minute: {market: row dict}, leaving out markets with no row."""
        self._s.step(rows)

    def report(self):
        return self._s.report()

    @property
    def ctx(self):
        return self._s.ctx

    @property
    def last_ts(self):
        return self._s.last_ts

    @property
    def markets(self):
        return self._s.markets


def grid(params):
    """Every combination of `params` (name -> list of values), as dicts, in index order:
    the grid's keys in the order given, the last varying fastest."""
    names = list(params)
    for name in names:
        if not isinstance(params[name], (list, tuple)) or not params[name]:
            raise ValueError(f"grid: {name} needs a non-empty list of values")
    return [dict(zip(names, combo)) for combo in itertools.product(*(params[n] for n in names))]


def sweep(make, start, end, params, source="data/hyperliquid", config=None, *,
          max_leverage=None, base=None, code=None, shard=None):  # fmt: skip
    """Backtests `make(**base, **point)` for each point of the grid `params`. Returns one
    dict per point run: index, params, result_hash and the report's stats, plus receipt_id
    when `code` (the strategy's source) is given. `shard=(i, n)` runs only the points with
    index % n == i, so a sweep can be split across workers without changing any result."""
    points = grid(params)
    if shard is not None:
        i, n = shard
        if not 0 <= i < n:
            raise ValueError(f"shard {i} of {n} does not exist")
    out = []
    for index, point in enumerate(points):
        if shard is not None and index % shard[1] != shard[0]:
            continue
        p = {**(base or {}), **point}
        report = backtest(make, start, end, source, config, max_leverage=max_leverage, params=p)
        row = {"index": index, "params": point, "result_hash": report.result_hash,
               **(report.stats or {})}  # fmt: skip
        if code is not None:
            row["receipt_id"] = report.receipt(code, p).receipt_id
        out.append(row)
    return out
