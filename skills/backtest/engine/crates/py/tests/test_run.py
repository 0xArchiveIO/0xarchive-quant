"""Runs: portfolios, the window and its warm-up, paper sessions, sweeps and data sources.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import json
import threading

import pytest

import oxarchive_backtest as oxb
from pairs import pairs
from refdata import CONFIG, MIN, T0, end, mean_cross, rows, write


@pytest.fixture
def two(tmp_path):
    write(tmp_path, "A", 4000, "hyperliquid:BTC")
    write(tmp_path, "B", 4000, "hyperliquid:ETH", 0.37)
    return str(tmp_path)


def test_portfolio_trades_both_markets_and_names_them(two):
    fills = []
    r = oxb.backtest(pairs(fills=fills), T0, end(4000), two, CONFIG)
    assert {m for m, _ in fills} == {"A", "B"} == {f.market for f in r.fills}
    assert set(r.final_positions) == {"A", "B"}
    body = json.loads(r.receipt("src").json)
    # Default limits come from the dataset's market: BTC 40x, ETH 25x.
    assert [(m["name"], m["max_leverage"]) for m in body["markets"]] == [("A", 40.0), ("B", 25.0)]
    assert [d["market"] for d in body["datasets"]] == ["hyperliquid:BTC", "hyperliquid:ETH"]
    assert oxb.backtest(pairs(), T0, end(4000), two, CONFIG).result_hash == r.result_hash
    lower = oxb.backtest(pairs(), T0, end(4000), two, CONFIG, max_leverage={"B": 2})
    assert json.loads(lower.receipt("src").json)["markets"][1]["max_leverage"] == 2
    with pytest.raises(ValueError, match="not a market of this run"):
        oxb.backtest(pairs(), T0, end(4000), two, max_leverage={"C": 3})


def test_ctx_orders_name_their_market(two):
    seen = []

    def step(ctx, h):
        if seen or ctx.time < T0 + 10 * MIN:  # the check before the run has time 0
            return
        with pytest.raises(ValueError) as e:
            ctx.market("C", 1.0)
        seen.append(str(e.value))
        ctx.limit("B", 1.0, 1.0)
        (o,) = ctx.open_orders
        seen.append((o.market, o.kind, o.price, ctx.price("A") is not None, ctx.universe))

    s = oxb.Strategy(universe=["A", "B"], clock="1m", bars="1m", step=step)
    oxb.backtest(s, T0, end(100), two)
    assert 'unknown market "C"' in seen[0]
    assert seen[1] == ("B", "limit", 1.0, True, ["A", "B"])


def test_the_window_warms_up_and_trades_from_start(two):
    """A run from the middle of the data reads the bars before it, so it trades at its
    first tick, as a run from the beginning would hold by then; and it counts only from
    its start."""
    start = T0 + 2000 * MIN
    r = oxb.backtest(mean_cross("A"), start, end(4000), two, CONFIG)
    assert r.fills[0].ts == start  # its first tick's order, filled on the next minute
    # The curve runs from the window's start (the close of the minute before) to its end.
    assert r.stats["start"] == start and r.stats["start_equity"] == 10_000.0
    assert r.equity[-1][0] == end(4000) and len(r.equity) == 2001
    assert json.loads(r.receipt("x").json)["window"] == [start, end(4000)]
    with pytest.raises(ValueError, match="end must be after start"):
        oxb.backtest(mean_cross("A"), start, start, two)


def test_session_fed_live_rows_equals_the_backtest(two):
    bt = oxb.backtest(mean_cross("A"), T0, end(4000), two, CONFIG)
    s = oxb.Session(mean_cross("A"), {"A": "hyperliquid:BTC"}, config=CONFIG)
    for d in rows(two, "A"):
        s.step({"A": d})
    assert s.report().result_hash == bt.result_hash
    assert s.ctx.positions["A"] == bt.final_positions["A"]
    assert json.loads(s.report().receipt("x").json)["datasets"][0]["month"] == "live"


def test_a_deploy_warmed_up_on_history_holds_what_the_backtest_holds(two):
    """A session that starts trading at `start`, warmed up on the history before it, then
    fed the rest as live rows, is the backtest of the same window."""
    start = T0 + 2000 * MIN
    bt = oxb.backtest(pairs(), start, end(4000), two, CONFIG)
    s = oxb.Session(pairs(), {"A": "hyperliquid:BTC", "B": "hyperliquid:ETH"}, start=start,
                    config=CONFIG)  # fmt: skip
    cut = T0 + 3000 * MIN
    s.warm_up(two, until=cut)
    assert s.last_ts == cut - MIN
    for a, b in zip(rows(two, "A"), rows(two, "B")):
        if a["ts"] >= cut:
            s.step({"A": a, "B": b})
    assert s.report().result_hash == bt.result_hash


def test_session_refuses_bad_rows():
    s = oxb.Session(mean_cross("A"), ["A"])
    s.step({"A": {"ts": T0, "close": 1.0}})
    with pytest.raises(ValueError, match="increasing ts"):
        s.step({"A": {"ts": T0, "close": 1.0}})
    with pytest.raises(KeyError, match="clsoe"):
        s.step({"A": {"ts": T0 + MIN, "clsoe": 1.0}})
    p = oxb.Session(pairs(), ["A", "B"])
    with pytest.raises(ValueError, match="share ts"):
        p.step({"A": {"ts": T0}, "B": {"ts": T0 + MIN}})
    with pytest.raises(KeyError, match="unknown market"):
        p.step({"C": {"ts": T0}})


def test_a_session_can_be_used_from_another_thread(two):
    # A runner steps sessions on its feed thread while web requests read them.
    s = oxb.Session(mean_cross("A"), ["A"], config=CONFIG)
    rs = rows(two, "A")[:300]
    t = threading.Thread(target=lambda: [s.step({"A": d}) for d in rs])
    t.start()
    t.join()
    assert s.report().result_hash == oxb.backtest(mean_cross("A"), T0, end(300), two, CONFIG).result_hash


def test_sweep_shards_cover_the_grid_once(two):
    params = {"lookback": [5, 10], "size": [1.0, 2.0, 3.0]}
    assert oxb.grid(params)[1] == {"lookback": 5, "size": 2.0}
    full = oxb.sweep(pairs, T0, end(1000), params, two, CONFIG, code="src")
    assert [r["index"] for r in full] == list(range(6))
    parts = [oxb.sweep(pairs, T0, end(1000), params, two, CONFIG, code="src", shard=(i, 4))
             for i in range(4)]  # fmt: skip
    merged = sorted((r for p in parts for r in p), key=lambda r: r["index"])
    assert merged == full
    assert all(len(r["receipt_id"]) == 64 and "sharpe" in r for r in full)
    with pytest.raises(ValueError):
        oxb.sweep(pairs, T0, end(1000), {"lookback": []}, two)


def test_pairs_check_backtest_equals_session(tmp_path):
    import pairs as p

    bt, session = p.run(oxb, str(tmp_path))
    assert bt == session


def test_a_source_reads_only_the_months_and_columns_needed(tmp_path):
    root = tmp_path / "d"
    write(root, "A", 100)
    (root / "A" / "2026-10.parquet").rename(root / "A" / "2026-09.parquet")
    write(root, "A", 100, scale=1.1)
    src = oxb.Source(str(root))
    assert src.markets() == ["A"] and not src.has_hourly()
    (ds,) = [d for _, d in src.datasets(["A"], T0, end(100))]
    assert [f[1] for f in ds.files] == ["2026-10"]
    # Listed but not on disk: the run says which files to fetch.
    browser = oxb.Source(str(root), index={"A": ["2026-09", "2026-10", "2026-11"]})
    with pytest.raises(oxb.MissingData) as e:
        browser.datasets(["A"], T0, T0 + 40 * 86_400_000)
    assert e.value.files == [("A/2026-11.parquet", False, None)]
    # A file fetched by range holds some columns only: asking for others names them.
    part = oxb.Source(str(root), have={"minute/A/2026-10.parquet": ["close"]})
    part.datasets(["A"], T0, end(100), columns=["close"])
    with pytest.raises(oxb.MissingData) as e:
        part.datasets(["A"], T0, end(100), columns=["close", "open"])
    assert e.value.files == [("A/2026-10.parquet", False, ["close", "open"])]


def test_column_ranges_and_written_rows(tmp_path):
    from oxarchive_backtest import _native

    rows = [{"ts": T0 + i * 3_600_000 + 59 * MIN, "open": 1.0 + i, "high": 2.0 + i,
             "low": 0.5 + i, "close": 1.5 + i, "quote_volume": 10.0} for i in range(30)]  # fmt: skip
    _native._write_rows(str(tmp_path / "X"), "hyperliquid:X", "2026-10", "candles-1h", rows)
    got = _native.load(str(tmp_path / "X")).rows()
    assert [r["close"] for r in got] == [r["close"] for r in rows]
    data = (tmp_path / "X" / "2026-10.parquet").read_bytes()
    need = _native._column_ranges(data[-8:], len(data), ["close"])
    assert isinstance(need, int) and need > 8
    ranges = _native._column_ranges(data[-need:], len(data), ["close"])
    assert ranges[-1][1] == len(data) and sum(b - a for a, b in ranges) < len(data)
    with pytest.raises(ValueError, match="increasing"):
        _native._write_rows(str(tmp_path / "Y"), "hyperliquid:Y", "2026-10", "v", rows[::-1])


def test_a_step_that_reads_on_one_branch_only_says_what_to_declare(two):
    def step(ctx, h):
        if ctx.time > T0 + 10 * MIN:
            h.quote_volume.sum(10)

    s = oxb.Strategy(universe=["A"], clock="1m", bars="1m", step=step)
    with pytest.raises(ValueError, match=r'fields=\["quote_volume"\]'):
        oxb.backtest(s, T0, end(100), two)
    ok = oxb.Strategy(universe=["A"], clock="1m", bars="1m", step=step, fields=["quote_volume"])
    oxb.backtest(ok, T0, end(100), two)

    def deep(ctx, h):
        if ctx.time > T0 + 10 * MIN:
            h.close.mean(100)

    s = oxb.Strategy(universe=["A"], clock="1m", bars="1m", carry="0m", step=deep, fields=["close"])
    with pytest.raises(ValueError, match="history="):
        oxb.backtest(s, T0, end(200), two)
