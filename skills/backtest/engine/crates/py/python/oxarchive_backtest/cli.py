"""The `0xbt` command, installed with the wheel (also `python -m oxarchive_backtest`).

A strategy file defines `strategy`: an oxb.Strategy, or a function returning one whose
keyword arguments are its parameters (references/strategy-api.md in the skill).

    0xbt check STRATEGY.py
        Fails (exit 1) if the strategy imports something outside the curated set.
        Also lists what can give different results on another machine (determinism
        warnings, and curated packages installed here at other versions); those don't fail
        unless --strict.

    0xbt backtest STRATEGY.py --start DATE --end DATE [--data DIR] [-p NAME=VALUE ...]
                 [-c NAME=VALUE ...] [--max-leverage MARKET=X ...] [--receipt FILE]
        Loads the strategy with load_strategy, runs it over
        [start, end) with the -p params and the -c Config fields, and prints its stats and
        receipt id. DIR (default data/hyperliquid) holds <MARKET>/<YYYY-MM>.parquet; the
        run reads only the markets, months and columns it needs. --receipt writes the
        receipt's canonical JSON. A VALUE is read as JSON when it parses (10, 0.5, true),
        else as a string.

    0xbt sweep STRATEGY.py --start DATE --end DATE -g NAME=V1,V2,... [-g ...] [--workers N]
              [--top K] [--json FILE]  (plus backtest's --data, -p, -c, --max-leverage)
        One backtest per combination of the -g values, split across N local processes
        (default: one per CPU). Prints the top K by return; --json writes every point.

    0xbt gauntlet STRATEGY.py --start DATE --end DATE [-g NAME=V1,V2,... ...] [--at DATE]
                 [--seeds N] [--train D] [--test D] [--benchmark MARKET] [--json FILE]
                 [--card FILE]  (plus backtest's --data, -p, -c, --max-leverage)
        Every check on one strategy (oxb.gauntlet): the grid as trials, costs, nulls,
        split and walk-forward, the deflated Sharpe over the study's ledger, robustness,
        and a verdict read off fixed rules. --card writes the one-page report.

    0xbt study init [--holdout DATE] [--data DIR]
    0xbt study status
    0xbt study commit STRATEGY.py [-p NAME=VALUE ...] [-c ...] [--data DIR]
        A study in .0xarchive/ (study.py): every backtest and sweep below the directory
        goes into its ledger, and its holdout (default: the data's last three months)
        stays sealed until `commit` runs one candidate over it, once.

    0xbt data pull --markets BTC,ETH,... --from YYYY-MM --to YYYY-MM [--data DIR] [--refresh]
        Builds the minute dataset from the 0xArchive API with $OXARCHIVE_API_KEY: one Parquet
        file per market per month in DIR (default data/hyperliquid). Months already on disk
        are kept unless --refresh. Book and liquidation-level columns are null in files built
        this way: the API has no 1-minute form of them.

    0xbt data check [--data DIR]
        Checks every file in DIR: schema, row order, one row per minute.

    0xbt hourly [--data DIR]
        Writes hourly bars of every market to DIR-1h, which universe rules read instead
        of the minute files. Skips months already written.

    0xbt packages
        The curated package set and its pinned versions.
"""

import argparse
import json
import os
import pathlib
import subprocess
import sys
import warnings

from . import Config, backtest, load_strategy, packages, strategy_of
from . import evaluate as _evaluate
from . import run as _run
from . import study as _study
from ._determinism import DeterminismWarning
from ._native import _write_hourly


def _value(text):
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return text


def _pairs(items, flag, parse=_value):
    out = {}
    for item in items or ():
        name, sep, value = item.partition("=")
        if not sep or not name:
            raise SystemExit(f"{flag} wants NAME=VALUE, got {item!r}")
        out[name] = parse(value)
    return out


def _load(path):
    """Loads the strategy, collecting its determinism warnings instead of printing them."""
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always", DeterminismWarning)
        module = load_strategy(path, name="oxs_strategy")
    notes = [
        f"{w.filename}:{w.lineno}: {w.message}"
        for w in caught
        if issubclass(w.category, DeterminismWarning)
    ]
    return module, notes


class _Run:
    """What backtest and sweep share: the loaded strategy, window, data and config."""

    def __init__(self, args, quiet=False):
        path = pathlib.Path(args.strategy)
        self.source = path.read_bytes()
        say = (lambda _m: None) if quiet else (lambda m: print(f"warning: {m}", file=sys.stderr))
        for p in packages.check_imports(self.source.decode("utf-8"), str(path)):
            say(p)
        # The study's ledger records which file each run came from.
        _study._WHAT.set({"file": str(path), "source_sha256": _study.sha256(self.source)})
        self.module, notes = _load(path)
        for n in notes:
            say(n)
        self.params = _pairs(args.param, "-p")
        try:
            self.config = Config(**_pairs(args.config, "-c"))
        except TypeError as e:
            raise SystemExit(f"-c: {e}") from None
        self.max_leverage = _pairs(args.max_leverage, "--max-leverage", float) or None
        self.start, self.end, self.data = args.start, args.end, args.data
        try:
            strategy_of(self.module, self.params)
        except (ValueError, TypeError) as e:
            raise SystemExit(str(e)) from None

    def make(self, **params):
        return strategy_of(self.module, params)

    def backtest(self, **params):
        with warnings.catch_warnings():
            # Already reported, from loading.
            warnings.simplefilter("ignore", DeterminismWarning)
            try:
                return backtest(self.make, self.start, self.end, self.data, self.config,
                                max_leverage=self.max_leverage, params=params)  # fmt: skip
            except (ValueError, _run.MissingData, _study.Sealed) as e:
                raise SystemExit(str(e)) from None


def _pct(x):
    return "n/a" if x is None else f"{x * 100:+.2f}%"


def cmd_check(args):
    path = pathlib.Path(args.strategy)
    source = path.read_text(encoding="utf-8")
    imports = packages.check_imports(source, str(path))
    for p in imports:
        print(f"error: {p}")
    if imports:
        # Loading would run the failing imports; report the rest only once they're fixed.
        print(f"{len(imports)} import problem(s): this strategy is outside the curated set")
        return 1
    _, notes = _load(path)
    versions = packages.check_installed(source, str(path))
    for n in notes + versions:
        print(f"warning: {n}")
    if notes or versions:
        print(
            f"{len(notes) + len(versions)} warning(s): results may differ on another machine, "
            "so compare runs trade by trade rather than by hash"
        )
        return 1 if args.strict else 0
    print("OK: imports are in the curated set and nothing machine-dependent was found")
    return 0


def cmd_backtest(args):
    run = _Run(args)
    report = run.backtest(**run.params)
    receipt = report.receipt(run.source, run.params)
    s = report.stats or {}
    sharpe = s.get("sharpe")
    print(
        f"{len(report.final_positions)} markets; return {_pct(s.get('return'))}, "
        f"sharpe {'n/a' if sharpe is None else f'{sharpe:.2f}'}, "
        f"max drawdown {_pct(s.get('max_drawdown'))}; {len(report.fills)} fills, "
        f"fees {s.get('fees', 0.0):.2f}, funding paid {report.funding_paid:.2f}, "
        f"{report.liquidations} liquidation(s); final equity "
        f"{'n/a' if report.final_equity is None else f'{report.final_equity:.2f}'}"
    )
    print(f"result_hash={report.result_hash} receipt_id={receipt.receipt_id}")
    if args.receipt:
        pathlib.Path(args.receipt).write_text(receipt.json + "\n", encoding="utf-8")
    return 0


def _grid(items):
    return _pairs(items, "-g", lambda v: [_value(x) for x in v.split(",")])


def cmd_sweep(args):
    grid = _grid(args.grid)
    if not grid:
        raise SystemExit("sweep: give at least one -g NAME=V1,V2,...")
    if args.shard:
        # A worker: run one shard and print its points as JSON.
        i, n = (int(x) for x in args.shard.split("/"))
        run = _Run(args, quiet=True)
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeterminismWarning)
            out = _run.sweep(run.make, run.start, run.end, grid, run.data, run.config,
                             max_leverage=run.max_leverage, base=run.params, code=run.source,
                             shard=(i, n))  # fmt: skip
        print(json.dumps(out))
        return 0

    total = len(_run.grid(grid))
    workers = max(1, min(args.workers or os.cpu_count() or 1, total))
    # Check the strategy once, with warnings shown, before starting workers, and that the
    # window stays out of a sealed holdout.
    _Run(args)
    st = _study.active()
    if st is not None:
        try:
            st.guard(_run.when(args.end))
        except _study.Sealed as e:
            raise SystemExit(str(e)) from None
    base = [sys.executable, "-m", "oxarchive_backtest", *args.argv]
    procs = [
        subprocess.Popen([*base, "--shard", f"{i}/{workers}"], stdout=subprocess.PIPE, text=True)
        for i in range(workers)
    ]
    results = []
    for p in procs:
        out, _ = p.communicate()
        if p.returncode != 0:
            raise SystemExit(f"a sweep worker failed (exit {p.returncode})")
        results.extend(json.loads(out))
    results.sort(key=lambda r: r["index"])
    print(f"{len(results)} runs on {workers} process(es)")
    ranked = sorted(results, key=lambda r: -(r.get("return") or 0.0))
    for r in ranked[: args.top]:
        sharpe = r.get("sharpe")
        print(
            f"  {_pct(r.get('return')):>9}  sharpe {'n/a' if sharpe is None else f'{sharpe:5.2f}'}"
            f"  fills={r.get('fills', 0):<6} {json.dumps(r['params'])}  {r['result_hash']}"
        )
    if args.json:
        pathlib.Path(args.json).write_text(json.dumps(results, indent=1) + "\n", encoding="utf-8")
    return 0


def _num(x, fmt="{:.2f}"):
    return "n/a" if x is None else fmt.format(x)


def card(result, file, args=None):
    """The gauntlet's one-page report, as Markdown."""
    v, c, s = result["verdict"], result["candidate"], result["candidate"]["stats"] or {}
    n, sp, wf, d, rb = (result["nulls"], result["split"], result["walk_forward"],
                        result["deflated"], result["robustness"])  # fmt: skip
    day = lambda ms: _study._iso(ms)[:10]
    lines = [
        f"# {pathlib.Path(file).name}: {v['rung']}",
        "",
        f"**Verdict: {v['rung']}** (rung {v['level']} of {len(_evaluate.LADDER)}: "
        + " / ".join(_evaluate.LADDER) + ").",
    ]
    lines += [f"- {r}" for r in v["reasons"]]
    span = f"{day(result['window']['start'])} to {day(result['window']['end'])}"
    lines += ["", f"{span}, params {json.dumps(c['params'])}.", "", "| Check | Result |", "|---|---|"]
    lines.append(f"| After costs | Sharpe {_num(s.get('sharpe'))}, return {_pct(s.get('return'))}, "
                 f"max drawdown {_num(s.get('max_drawdown'), '{:.2%}')}, fees {_num(s.get('fees'))}, "
                 f"funding paid {_num(s.get('funding_paid'))} |")  # fmt: skip
    vs = result["vs"]
    if vs:
        lines.append(f"| Against {args.benchmark if args else 'benchmark'} | alpha "
                     f"{_pct(vs.get('alpha'))} a year, beta {_num(vs.get('beta'))}, information "
                     f"ratio {_num(vs.get('information_ratio'))} |")  # fmt: skip
    if n:
        lines.append(f"| Nulls | flipped Sharpe {_num(n['flipped'])}; random mean "
                     f"{_num(n.get('random_mean'))}, {_pct(n.get('random_share_above'))} at or above; "
                     f"stale mean {_num(n.get('stale_mean'))}, {_pct(n.get('stale_share_above'))} "
                     "at or above; redrawn "
                     + (f"every {n['redrawn_every_ms'] / 3_600_000:g} h" if n.get("redrawn_every_ms")
                        else "every tick") + " |")  # fmt: skip
    lines.append(f"| Split at {day(sp['at'])} | picked {json.dumps(sp['params'])}: Sharpe "
                 f"{_num(sp['in_sample'])} before, {_num(_evaluate._sharpe(sp['out_of_sample']))} after |")  # fmt: skip
    if wf:
        lines.append(f"| Walk-forward | {wf['folds']} folds, Sharpe {_num(_evaluate._sharpe(wf['stats']))}, "
                     f"return {_pct((wf['stats'] or {}).get('return'))} |")  # fmt: skip
    lines.append(f"| Deflated Sharpe | {_num(d['deflated_sharpe'])} over {d['trials']} trial(s) "
                 f"({'the study ledger' if d['over'] == 'study' else 'this grid only'}); luck's "
                 f"best: Sharpe {_num(d['threshold'])} |")  # fmt: skip
    lines.append(f"| Robustness | {_pct(rb['grid_positive_share'])} of {rb['grid_points']} grid "
                 f"points positive; weakest neighbour {_num(rb['neighbour_min_ratio'])}x the best; "
                 f"top market {rb['top_market']} made {_pct(rb['top_market_share'])} of gains |")  # fmt: skip
    h = result["holdout"]
    if h:
        if h["sealed"]:
            lines.append(f"| Holdout | sealed from {day(h['starts'])} |")
        else:
            r = h["result"] or {}
            lines.append(f"| Holdout from {day(h['starts'])} | committed "
                         f"{json.dumps((h['commit'] or {}).get('params'))}"
                         f"{'' if h['this_candidate'] else ' (another candidate)'}: Sharpe "
                         f"{_num(r.get('sharpe'))}, return {_pct(r.get('return'))} |")  # fmt: skip
    if v["notes"]:
        lines += ["", "Notes:", ""] + [f"- {x}" for x in v["notes"]]
    lines += ["", f"Result hash `{c['result_hash']}`"
              + (f", receipt `{result['receipt_id']}`" if result.get("receipt_id") else "") + "."]
    return "\n".join(lines) + "\n"


def cmd_gauntlet(args):
    run = _Run(args)
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeterminismWarning)
            result = _evaluate.gauntlet(run.make, _grid(args.grid), run.start, run.end, run.data,
                                        run.config, at=args.at, train=args.train, test=args.test,
                                        seeds=args.seeds, benchmark=args.benchmark,
                                        base=run.params, max_leverage=run.max_leverage)  # fmt: skip
    except (ValueError, _run.MissingData, _study.Sealed) as e:
        raise SystemExit(str(e)) from None
    report = result.pop("report")
    p = {**run.params, **result["candidate"]["params"]}
    result["receipt_id"] = report.receipt(run.source, p).receipt_id
    text = card(result, args.strategy, args)
    print(text, end="")
    if args.card:
        pathlib.Path(args.card).write_text(text, encoding="utf-8")
    if args.json:
        pathlib.Path(args.json).write_text(json.dumps(result, indent=1, default=str) + "\n",
                                           encoding="utf-8")  # fmt: skip
    return 0


def cmd_study(args):
    if args.action == "init":
        try:
            st = _study.Study.init(".", holdout=args.holdout, source=args.data)
        except (FileExistsError, ValueError) as e:
            raise SystemExit(str(e)) from None
        print(f"study started in {st.path}: holdout sealed from {st._config['holdout']} UTC; "
              f"every backtest from this directory down goes into {st.path / _study.TRIALS}")  # fmt: skip
        return 0
    st = _study.active()
    if st is None:
        raise SystemExit("no study here: `0xbt study init` starts one")
    if args.action == "status":
        trials, entries = st.trials(), st.entries()
        print(f"study {st.path}: holdout from {st._config['holdout']} UTC, "
              f"{'sealed' if st.sealed else 'open'}")  # fmt: skip
        print(f"{len(trials)} trial(s), {sum(1 for e in entries if e['kind'] == 'check')} check run(s)")
        for t in sorted(trials, key=lambda t: -(t["sharpe"] if t["sharpe"] is not None else -1e9))[:5]:
            print(f"  sharpe {_num(t['sharpe']):>6}  {t['start'][:10]} to {t['end'][:10]}  "
                  f"{t['file'] or '(python)'} {json.dumps(t['params'])}  {t['result_hash']}")  # fmt: skip
        if st.commit_record:
            r = st.holdout_result
            print(f"committed {st.commit_record['file']} {json.dumps(st.commit_record['params'])}: "
                  f"holdout Sharpe {_num(r['sharpe'])}, return {_pct(r['return'])}, "
                  f"{'passed' if r['passed'] else 'failed'}")  # fmt: skip
        return 0
    # commit
    if args.strategy is None:
        raise SystemExit("commit needs the strategy file")
    args.start = args.end = None
    run = _Run(args)
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", DeterminismWarning)
            report = st.commit(run.make, file=args.strategy, source_text=run.source,
                               params=run.params, config=run.config, data=args.data,
                               max_leverage=run.max_leverage)  # fmt: skip
    except (ValueError, _run.MissingData) as e:
        raise SystemExit(str(e)) from None
    r, c = st.holdout_result, st.commit_record
    if c["was_tried"] is False:
        print("warning: this file and these params are not in the ledger as a trial", file=sys.stderr)
    print(f"holdout {r['start'][:10]} to {r['end'][:10]}: Sharpe {_num(r['sharpe'])}, return "
          f"{_pct(r['return'])}, max drawdown {_pct(r['max_drawdown'])}: "
          f"{'passed' if r['passed'] else 'failed'}. The study is now open; this was its one look.")  # fmt: skip
    print(f"result_hash={report.result_hash} receipt_id={report.receipt(run.source, run.params).receipt_id}")
    return 0


def _dataset_root(data):
    """`data` is <root>/hyperliquid: the builder writes <root>/hyperliquid/<MARKET>/."""
    path = pathlib.Path(data)
    if path.name != "hyperliquid":
        raise SystemExit(f"--data must end in /hyperliquid (got {data}); files go in DIR/<MARKET>/")
    return path.parent


def cmd_data(args):
    from ._native import _build_api

    root = _dataset_root(args.data)
    if args.action == "check":
        markets = _run.Source(args.data).markets()
        if not markets:
            raise SystemExit(f"no markets in {args.data}")
        bad = [m for m in markets
               if _build_api(["0xbt-build-api", "check", "--dir", os.path.join(args.data, m)])]  # fmt: skip
        print(f"{len(markets) - len(bad)} of {len(markets)} markets OK" + (f"; failed: {', '.join(bad)}" if bad else ""))
        return 1 if bad else 0
    if not (os.environ.get("OXARCHIVE_API_KEY") or os.environ.get("OXA_API_KEY")):
        raise SystemExit("set OXARCHIVE_API_KEY (https://www.0xarchive.io/signup)")
    if not args.markets or not args.start_month or not args.end_month:
        raise SystemExit("data pull needs --markets, --from and --to")
    failed = []
    for m in [x.strip().upper() for x in args.markets.split(",") if x.strip()]:
        argv = ["0xbt-build-api", "build", "--coin", m, "--from", args.start_month, "--to",
                args.end_month, "--out", str(root)]  # fmt: skip
        if not args.refresh:
            argv.append("--skip-existing")
        if _build_api(argv):
            failed.append(m)
    if failed:
        raise SystemExit(f"failed: {', '.join(failed)}")
    return 0


def cmd_hourly(args):
    root = args.data.rstrip("/")
    out = args.out or root + "-1h"
    markets = _run.Source(root).markets()
    if not markets:
        raise SystemExit(f"no markets in {root}")
    written = 0
    for m in markets:
        written += _write_hourly(os.path.join(root, m), os.path.join(out, m))
    print(f"{written} hourly file(s) written to {out} for {len(markets)} markets")
    return 0


def cmd_packages(_args):
    print(f"curated set {packages.PYODIDE_VERSION}")
    for name, version in packages.PACKAGES.items():
        print(f"  {name}=={version}")
    return 0


def _run_args(p):
    p.add_argument("strategy")
    p.add_argument("--start", required=True, help="first day, e.g. 2025-07-01 (UTC)")
    p.add_argument("--end", required=True, help="end, exclusive, e.g. 2025-10-01")
    p.add_argument("--data", default="data/hyperliquid",
                   help="directory of <MARKET>/<YYYY-MM>.parquet (default data/hyperliquid)")  # fmt: skip
    p.add_argument("-p", "--param", action="append", metavar="NAME=VALUE")
    p.add_argument("-c", "--config", action="append", metavar="NAME=VALUE")
    p.add_argument("--max-leverage", action="append", metavar="MARKET=X",
                   help="a market's leverage limit, over the venue's default")  # fmt: skip


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(prog="0xbt", description="0xArchive backtest engine")
    sub = parser.add_subparsers(dest="command", required=True)

    p = sub.add_parser("check", help="check imports and determinism")
    p.add_argument("strategy")
    p.add_argument("--strict", action="store_true", help="fail on warnings too")
    p.set_defaults(run=cmd_check)

    p = sub.add_parser("backtest", help="run a strategy over a window of history")
    _run_args(p)
    p.add_argument("--receipt", metavar="FILE", help="write the receipt's JSON here")
    p.set_defaults(run=cmd_backtest)

    p = sub.add_parser("sweep", help="run a parameter grid across local processes")
    _run_args(p)
    p.add_argument("-g", "--grid", action="append", metavar="NAME=V1,V2,...")
    p.add_argument("--workers", type=int, help="processes (default: one per CPU)")
    p.add_argument("--top", type=int, default=10, help="how many results to print")
    p.add_argument("--json", metavar="FILE", help="write every point's result here")
    p.add_argument("--shard", help=argparse.SUPPRESS)
    p.set_defaults(run=cmd_sweep)

    p = sub.add_parser("gauntlet", help="every check on one strategy, and a verdict")
    _run_args(p)
    p.add_argument("-g", "--grid", action="append", metavar="NAME=V1,V2,...")
    p.add_argument("--at", help="where the split's out-of-sample part starts (default: two thirds in)")
    p.add_argument("--seeds", type=int, default=10, help="random and stale nulls each (default 10)")
    p.add_argument("--train", default="180d", help="walk-forward training window (default 180d)")
    p.add_argument("--test", default="60d", help="walk-forward test window (default 60d)")
    p.add_argument("--benchmark", default="BTC", help='a market to hold, or "equal" (default BTC)')
    p.add_argument("--json", metavar="FILE", help="write the full result here")
    p.add_argument("--card", metavar="FILE", help="write the one-page Markdown report here")
    p.set_defaults(run=cmd_gauntlet)

    p = sub.add_parser("study", help="a trial ledger and a sealed holdout (.0xarchive/)")
    p.add_argument("action", choices=["init", "status", "commit"])
    p.add_argument("strategy", nargs="?", help="commit: the candidate's strategy file")
    p.add_argument("--holdout", help="init: where the sealed months start (default: the last three)")
    p.add_argument("--data", default="data/hyperliquid")
    p.add_argument("-p", "--param", action="append", metavar="NAME=VALUE")
    p.add_argument("-c", "--config", action="append", metavar="NAME=VALUE")
    p.add_argument("--max-leverage", action="append", metavar="MARKET=X")
    p.set_defaults(run=cmd_study)

    p = sub.add_parser("data", help="build the minute dataset from the 0xArchive API")
    p.add_argument("action", choices=["pull", "check"])
    p.add_argument("--markets", help="pull: comma-separated perps, e.g. BTC,ETH,SOL")
    p.add_argument("--from", dest="start_month", metavar="YYYY-MM")
    p.add_argument("--to", dest="end_month", metavar="YYYY-MM")
    p.add_argument("--data", default="data/hyperliquid")
    p.add_argument("--refresh", action="store_true", help="rebuild months already on disk")
    p.set_defaults(run=cmd_data)

    p = sub.add_parser("hourly", help="write hourly bars for universe rules")
    p.add_argument("--data", default="data/hyperliquid")
    p.add_argument("--out", help="default: DATA-1h")
    p.set_defaults(run=cmd_hourly)

    p = sub.add_parser("packages", help="list the curated package set")
    p.set_defaults(run=cmd_packages)

    args = parser.parse_args(argv)
    args.argv = argv
    return args.run(args)
