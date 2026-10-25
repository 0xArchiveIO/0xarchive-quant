"""The curated package set, the import check, receipts and the `0xbt` command.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import importlib.metadata
import json
import pathlib
import sys
import textwrap

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import _native, packages
from oxarchive_backtest.cli import main

try:
    import tomllib
except ModuleNotFoundError:  # Python 3.10
    tomllib = None

HERE = pathlib.Path(__file__).parent
T0 = 1_791_208_800_000
END = T0 + 3000 * 60_000
WINDOW = ["--start", str(T0), "--end", str(END)]
STRATEGY = textwrap.dedent(
    """
    import math

    import oxarchive_backtest as oxb


    def strategy(fast=10, slow=30):
        def step(ctx, h):
            w = h.close.window(slow)["REF"]
            if w[0] is None:
                return
            xs = [math.log(c) for c in w]
            f = s = 0.0
            for x in xs[-fast:]:
                f += x
            for x in xs:
                s += x
            target = 1.0 if f / fast > s / slow else 0.0
            if target != ctx.positions["REF"]:
                ctx.market("REF", target - ctx.positions["REF"])

        return oxb.Strategy(universe=["REF"], clock="1m", bars="1m", step=step)
    """
)


@pytest.fixture
def data(tmp_path):
    d = tmp_path / "data"
    _native._write_reference(str(d / "REF"), 3000)
    return d


@pytest.fixture
def strategy(tmp_path):
    p = tmp_path / "cross.py"
    p.write_text(STRATEGY)
    return p




def test_imports_allowed_and_refused():
    src = textwrap.dedent(
        """
        from __future__ import annotations
        import math, numpy as np
        from collections import deque
        from scipy import stats
        import oxarchive_backtest
        import requests
        from urllib import request
        import urllib.parse
        import threading
        from . import helpers
        def f():
            import sklearn.linear_model
        """
    )
    problems = packages.check_imports(src, "s.py")
    assert [p.split(": ", 1)[0] for p in problems] == [
        "s.py:7", "s.py:8", "s.py:10", "s.py:11", "s.py:13",
    ]  # fmt: skip
    assert "requests: not in the curated package set" in problems[0]
    assert "urllib.request: no network access" in problems[1]
    assert "threading: no threads" in problems[2]
    assert "relative import" in problems[3]
    assert "sklearn.linear_model: not in the curated" in problems[4]


def test_installed_version_mismatch_is_reported(monkeypatch):
    versions = {"numpy": "2.3.0", "pandas": packages.PACKAGES["pandas"]}

    def fake(dist):
        if dist in versions:
            return versions[dist]
        raise importlib.metadata.PackageNotFoundError(dist)

    monkeypatch.setattr(importlib.metadata, "version", fake)
    assert packages.check_installed("import math\n") == []
    found = packages.check_installed("import pandas\n")
    assert len(found) == 1 and found[0].startswith("numpy 2.3.0 is installed; the curated set")




@pytest.mark.skipif(tomllib is None, reason="needs tomllib")
def test_curated_extra_matches_the_package_set():
    pyproject = tomllib.loads((HERE.parent / "pyproject.toml").read_text())
    pins = pyproject["project"]["optional-dependencies"]["curated"]
    assert pins == [f"{n}=={v}" for n, v in packages.PACKAGES.items()]


def run(path_or_src, data, name, **params):
    if isinstance(path_or_src, str):
        mod = oxb.load_strategy(source=path_or_src, name=name)
    else:
        mod = oxb.load_strategy(path_or_src, name=name)
    return oxb.backtest(mod.strategy, T0, END, str(data), oxb.Config(taker_fee_bps=5.0),
                        params=params)  # fmt: skip


def test_receipt_is_stable_and_names_the_data(data):
    r = run(STRATEGY, data, "cross_a")
    a = r.receipt(STRATEGY, {"slow": 30, "fast": 10})
    b = r.receipt(STRATEGY.encode(), {"fast": 10, "slow": 30})
    assert a.receipt_id == b.receipt_id and len(a.receipt_id) == 64
    body = json.loads(a.json)
    assert body["platform"] == ("pyodide" if sys.platform == "emscripten" else "python")
    assert body["datasets"] == [
        {"market": "synthetic:reference", "month": "2026-10", "dataset_version": "lcg-3000"}
    ]
    assert body["params"] == {"fast": 10, "slow": 30} and body["window"] == [T0, END]
    assert body["result"]["result_hash"] == r.result_hash == a.result_hash
    assert "window(slow)" not in a.json, "the source hash, never the source"
    assert r.receipt(STRATEGY, {"fast": 11}).receipt_id != a.receipt_id
    with pytest.raises(ValueError):
        r.receipt(STRATEGY, {"x": float("nan")})


def test_cli_backtest_matches_the_api(data, strategy, tmp_path, capsys):
    out = tmp_path / "receipt.json"
    code = main(["backtest", str(strategy), "--data", str(data), *WINDOW, "-p", "fast=5",
                 "-c", "taker_fee_bps=5", "--receipt", str(out)])  # fmt: skip
    assert code == 0
    printed = capsys.readouterr().out
    r = run(strategy, data, "cross_b", fast=5)
    receipt = r.receipt(strategy.read_bytes(), {"fast": 5})
    assert f"result_hash={r.result_hash} receipt_id={receipt.receipt_id}" in printed
    assert printed.startswith("1 markets; return ")
    assert out.read_text() == receipt.json + "\n"
    assert json.loads(out.read_text())["config"]["taker_fee_bps"] == 5.0


def test_cli_check(strategy, tmp_path, capsys):
    assert main(["check", str(strategy)]) == 0
    assert "OK" in capsys.readouterr().out

    bad = tmp_path / "bad.py"
    bad.write_text("import requests\n" + STRATEGY)
    assert main(["check", str(bad)]) == 1
    assert "bad.py:1: import requests" in capsys.readouterr().out

    warn = tmp_path / "warn.py"
    warn.write_text(STRATEGY.replace("math.log(c)", "math.erf(c)"))
    assert main(["check", str(warn)]) == 0
    printed = capsys.readouterr().out
    assert "warning:" in printed and "math.erf" in printed
    assert main(["check", "--strict", str(warn)]) == 1


def test_cli_needs_a_strategy(data, tmp_path):
    none = tmp_path / "none.py"
    none.write_text("x = 1\n")
    with pytest.raises(SystemExit, match="must define `strategy`"):
        main(["backtest", str(none), "--data", str(data), *WINDOW])


def test_cli_portfolio_backtest(tmp_path, capsys):
    _native._write_reference(str(tmp_path / "d" / "BTC"), 2000, "hyperliquid:BTC")
    _native._write_reference(str(tmp_path / "d" / "ETH"), 2000, "hyperliquid:ETH", 0.5)
    pairs = tmp_path / "pairs.py"
    pairs.write_text(textwrap.dedent(
        """
        import oxarchive_backtest as oxb

        def step(ctx, h):
            if ctx.positions["BTC"] == 0:
                ctx.market("BTC", 1.0)
                ctx.market("ETH", -2.0)

        strategy = oxb.Strategy(universe=["BTC", "ETH"], clock="1m", bars="1m", step=step)
        """
    ))  # fmt: skip
    out = tmp_path / "r.json"
    assert main(["backtest", str(pairs), "--data", str(tmp_path / "d"), "--start", str(T0),
                 "--end", str(T0 + 2000 * 60_000), "--max-leverage", "ETH=3",
                 "--receipt", str(out)]) == 0  # fmt: skip
    assert "2 markets; return" in capsys.readouterr().out
    body = json.loads(out.read_text())
    assert [(m["name"], m["max_leverage"]) for m in body["markets"]] == [("BTC", 40.0), ("ETH", 3.0)]
    assert body["result"]["fills"] == 2


def test_cli_sweep_across_processes_equals_one_process(data, strategy, tmp_path, capsys):
    out = tmp_path / "sweep.json"
    args = ["sweep", str(strategy), "--data", str(data), *WINDOW, "-g", "fast=3,5,8",
            "-g", "slow=20,30"]  # fmt: skip
    assert main([*args, "--workers", "3", "--json", str(out), "--top", "2"]) == 0
    printed = capsys.readouterr().out
    assert printed.startswith("6 runs on 3 process(es)") and printed.count("fills=") == 2
    got = json.loads(out.read_text())
    mod = oxb.load_strategy(strategy, name="cross_sweep")
    want = oxb.sweep(mod.strategy, T0, END, {"fast": [3, 5, 8], "slow": [20, 30]}, str(data),
                     code=strategy.read_bytes())  # fmt: skip
    assert got == json.loads(json.dumps(want))


def test_cli_hourly_writes_bars_the_universe_reads(tmp_path, capsys):
    d = tmp_path / "d"
    _native._write_reference(str(d / "BTC"), 3000, "hyperliquid:BTC", quote_volume=2.0)
    assert main(["hourly", "--data", str(d)]) == 0
    assert "1 hourly file(s)" in capsys.readouterr().out
    hourly = _native.load(str(tmp_path / "d-1h" / "BTC"))
    assert len(hourly) == 50  # 3000 minutes from 14:00
    rows = hourly.rows()
    assert rows[0]["quote_volume"] == 120.0 and rows[0]["ts"] % 3_600_000 == 0
    assert hourly.files[0][2] == "lcg-3000+1h"
    assert main(["hourly", "--data", str(d)]) == 0
    assert "0 hourly file(s)" in capsys.readouterr().out


def test_package_version_is_the_engine_version():
    # Receipts carry engine_version; the PyPI version names the same thing.
    import importlib.metadata

    assert importlib.metadata.version("0xarchive-backtest") == oxb.engine_version()
