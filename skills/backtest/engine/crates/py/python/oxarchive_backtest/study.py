"""A study: the trial ledger and the sealed holdout of one search for an edge.

    0xbt study init --holdout 2026-07-01     # writes .0xarchive/ in the current directory
    0xbt backtest ... / 0xbt sweep ...        # every run goes into .0xarchive/trials.jsonl
    0xbt gauntlet momentum.py --start 2025-03-01 --end 2026-07-01 -g lookback=3d,7d
    0xbt study commit momentum.py -p lookback=7d    # opens the holdout, once

Two things go wrong when a person or an agent searches for a strategy. Tries are forgotten:
the tenth variant is reported as if it were the first, and a Sharpe that ten tries would
show by luck looks like an edge. And the test data stops being unseen: every look at the
last months shapes the next try. A study fixes both in code rather than by discipline.

- **Every backtest is written to the ledger**, from Python and from `0xbt` alike, whether or
  not anyone mentions it. A trial is a distinct result hash among the runs a user asked for;
  the backtests that checks run internally (nulls, benchmarks, the re-selections inside
  `split` and `walk_forward`) are recorded as checks and don't count. The same strategy
  over another window is another trial: choosing the window is a try too.
  `deflated_sharpe()` deflates over every trial in the ledger.
- **The holdout is sealed**: while it is, any run that reads data from the holdout's start
  on raises Sealed, whatever reads it (backtests, sweeps, universe picks, Session warm-ups).
  `commit()` names one candidate, runs it over the holdout once and opens the study. Runs
  after that are marked `after_reveal`, because the holdout is no longer unseen.

The seal guards every read made through the engine. Reading the Parquet files some other
way gets past it, which is why the 0xArchive skill forbids it inside a study.

A study is found the way git finds .git: $OXS_STUDY (the directory holding study.json),
else the nearest .0xarchive/study.json at or above the current directory. Without one,
nothing is recorded and nothing is sealed. The ledger observes runs and never changes
their results.
"""

import contextlib
import contextvars
import datetime as _dt
import hashlib
import json
import os
import pathlib

DIR = ".0xarchive"
STUDY = "study.json"
TRIALS = "trials.jsonl"

# What the current backtests are: "trial" (asked for) or "check" (run by a check).
_KIND = contextvars.ContextVar("oxs_study_kind", default="trial")
# What is being run, when the caller knows: {"file", "source_sha256"}.
_WHAT = contextvars.ContextVar("oxs_study_what", default=None)
# True while commit() runs the holdout.
_REVEAL = contextvars.ContextVar("oxs_study_reveal", default=False)


class Sealed(Exception):
    """A run would read the study's sealed holdout."""


def _iso(ms):
    return _dt.datetime.fromtimestamp(ms / 1000, _dt.timezone.utc).strftime("%Y-%m-%d %H:%M")


def _now():
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sha256(data):
    if isinstance(data, str):
        data = data.encode("utf-8")
    return hashlib.sha256(data).hexdigest()


@contextlib.contextmanager
def kind(name):
    """Backtests inside are recorded as `name` ("check" for a check's own runs)."""
    token = _KIND.set(name)
    try:
        yield
    finally:
        _KIND.reset(token)


@contextlib.contextmanager
def running(file=None, source=None):
    """Backtests inside run this strategy file (its text, for the hash)."""
    token = _WHAT.set({"file": None if file is None else str(file),
                       "source_sha256": None if source is None else sha256(source)})  # fmt: skip
    try:
        yield
    finally:
        _WHAT.reset(token)


class Study:
    """One study directory: study.json (the holdout and its state) and trials.jsonl."""

    def __init__(self, path):
        self.path = pathlib.Path(path)
        self._config = json.loads((self.path / STUDY).read_text(encoding="utf-8"))

    @classmethod
    def find(cls, start=None):
        """The active study, or None."""
        env = os.environ.get("OXS_STUDY")
        if env:
            p = pathlib.Path(env)
            if not (p / STUDY).is_file():
                raise FileNotFoundError(f"$OXS_STUDY is {env}, which holds no {STUDY}")
            return cls(p)
        try:
            here = pathlib.Path(start or os.getcwd()).resolve()
        except OSError:
            return None
        for d in (here, *here.parents):
            if (d / DIR / STUDY).is_file():
                return cls(d / DIR)
        return None

    @classmethod
    def init(cls, root=".", holdout=None, source="data/hyperliquid", months=3):
        """Starts a study in `root`/.0xarchive. `holdout` is where the sealed months
        begin (default: the first day of the source's last `months` months)."""
        from .run import Source, month_start
        from .strategy import when

        path = pathlib.Path(root) / DIR
        if (path / STUDY).exists():
            raise FileExistsError(f"{path / STUDY} exists: a study is already running here")
        data_end = _data_end(Source(source))
        if holdout is None:
            if data_end is None:
                raise ValueError(f"the source {source} has no data; give the holdout's start")
            last = _dt.datetime.fromtimestamp(data_end / 1000 - 1, _dt.timezone.utc)
            y, m = last.year, last.month - (months - 1)
            while m < 1:
                y, m = y - 1, m + 12
            h = month_start(f"{y:04d}-{m:02d}")
        else:
            h = when(holdout)
        if data_end is not None and h >= data_end:
            raise ValueError(f"the holdout starts at or after the source's last data ({_iso(data_end)})")
        path.mkdir(parents=True, exist_ok=True)
        config = {"version": 1, "created": _now(), "source": str(source), "holdout_ms": h,
                  "holdout": _iso(h), "state": "sealed", "commit": None, "holdout_result": None}  # fmt: skip
        (path / STUDY).write_text(json.dumps(config, indent=1) + "\n", encoding="utf-8")
        (path / TRIALS).touch()
        return cls(path)

    @property
    def holdout_ms(self):
        return self._config["holdout_ms"]

    @property
    def sealed(self):
        return self._config["state"] == "sealed"

    @property
    def commit_record(self):
        return self._config.get("commit")

    @property
    def holdout_result(self):
        return self._config.get("holdout_result")

    @property
    def source(self):
        return self._config.get("source")

    def _save(self):
        (self.path / STUDY).write_text(json.dumps(self._config, indent=1) + "\n", encoding="utf-8")

    def guard(self, t1):
        """Raises Sealed if reading up to `t1` (exclusive) would read the holdout."""
        if self.sealed and not _REVEAL.get() and t1 > self.holdout_ms:
            # Open-ended reads (a Session's warm-up) pass a far-future end.
            to = f"to {_iso(t1)} UTC" if t1 < 253_402_300_800_000 else "with no end"
            raise Sealed(
                f"this study's holdout from {self._config['holdout']} UTC is sealed, and this run "
                f"reads {to}. Run up to {self._config['holdout']}, or commit one candidate "
                "(`0xbt study commit`) to open it."
            )

    def record(self, report, start, end, params=None):
        """Appends one backtest to the ledger."""
        s = report.stats or {}
        what = _WHAT.get() or {}
        entry = {
            "at": _now(),
            "kind": "holdout" if _REVEAL.get() else _KIND.get(),
            "result_hash": report.result_hash,
            "start": _iso(start),
            "end": _iso(end),
            "start_ms": start,
            "end_ms": end,
            "params": params or {},
            "file": what.get("file"),
            "source_sha256": what.get("source_sha256"),
            "sharpe": s.get("sharpe"),
            "return": s.get("return"),
            "max_drawdown": s.get("max_drawdown"),
            "fills": s.get("fills"),
            "days": len(report.daily_returns),
            "after_reveal": not self.sealed and not _REVEAL.get(),
        }
        with open(self.path / TRIALS, "a", encoding="utf-8") as f:
            f.write(json.dumps(entry, sort_keys=True) + "\n")

    def entries(self):
        out = []
        with open(self.path / TRIALS, encoding="utf-8") as f:
            for line in f:
                if line.strip():
                    out.append(json.loads(line))
        return out

    def trials(self):
        """Every distinct run asked for (first of each result hash), in ledger order."""
        seen, out = set(), []
        for e in self.entries():
            if e["kind"] != "trial" or e["result_hash"] in seen:
                continue
            seen.add(e["result_hash"])
            out.append(e)
        return out

    def deflated_sharpe(self, report):
        """oxb.deflated_sharpe over every trial in the ledger."""
        from .evaluate import deflated_sharpe

        return deflated_sharpe(report, [t["sharpe"] for t in self.trials()])

    def commit(self, strategy, *, file=None, source_text=None, params=None, config=None,
               data=None, max_leverage=None):  # fmt: skip
        """Names `strategy` (with `params`) the study's one candidate, runs it over the
        holdout, from its start to the end of the data, and opens the study. Returns the
        holdout's Report. Works once."""
        from .run import Source, backtest

        if not self.sealed:
            raise ValueError("this study's holdout is already open: it was committed "
                             f"to {self.commit_record}")  # fmt: skip
        src = Source(data or self.source)
        t1 = _data_end(src)
        if t1 is None or t1 <= self.holdout_ms:
            raise ValueError("the source has no data in the holdout")
        digest = None if source_text is None else sha256(source_text)
        tried = any(t.get("source_sha256") == digest and t.get("params") == (params or {})
                    for t in self.trials()) if digest else None  # fmt: skip
        token = _REVEAL.set(True)
        try:
            with running(file, source_text):
                report = backtest(strategy, self.holdout_ms, t1, src, config,
                                  max_leverage=max_leverage, params=params)  # fmt: skip
        finally:
            _REVEAL.reset(token)
        s = report.stats or {}
        self._config["state"] = "opened"
        self._config["commit"] = {"at": _now(), "file": None if file is None else str(file),
                                  "source_sha256": digest, "params": params or {},
                                  "trials_before": len(self.trials()), "was_tried": tried}  # fmt: skip
        self._config["holdout_result"] = {
            "start": _iso(self.holdout_ms), "end": _iso(t1), "result_hash": report.result_hash,
            "sharpe": s.get("sharpe"), "return": s.get("return"),
            "max_drawdown": s.get("max_drawdown"), "fills": s.get("fills"),
            "days": len(report.daily_returns),
            "passed": s.get("sharpe") is not None and s["sharpe"] > 0,
        }  # fmt: skip
        self._save()
        return report


def _data_end(src):
    """The end (exclusive) of the source's last month, or None."""
    from .run import month_start

    idx = src.index()
    if not idx:
        return None
    last = max(months[-1] for months in idx.values())
    y, m = map(int, last.split("-"))
    y, m = (y + 1, 1) if m == 12 else (y, m + 1)
    return month_start(f"{y:04d}-{m:02d}")


def active():
    """The active study, or None. A broken one (a bad $OXS_STUDY, an unreadable
    study.json) raises rather than leaving the holdout silently unsealed."""
    return Study.find()
