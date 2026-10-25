"""Every file in examples/ loads with load_strategy, passes the import check,
and (with the dataset in data/hyperliquid) trades over a month; evaluate.py runs end to
end on a short window. Keeps the examples from going stale as the pieces change."""

import pathlib
import warnings

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import packages
from oxarchive_backtest._determinism import DeterminismWarning

ROOT = pathlib.Path(__file__).resolve().parents[3]
EXAMPLES = ROOT / "examples"
DATA = ROOT / "data" / "hyperliquid"
STRATEGIES = sorted(p for p in EXAMPLES.glob("*.py") if p.name != "evaluate.py")
CONFIG = oxb.Config(starting_cash=100_000)
needs_data = pytest.mark.skipif(not (DATA / "BTC").is_dir(), reason="no dataset in data/hyperliquid")


def load(path):
    with warnings.catch_warnings():
        warnings.simplefilter("error", DeterminismWarning)
        module = oxb.load_strategy(path, name=f"example_{path.stem}")
    return oxb.strategy_of(module)


def test_there_are_examples():
    assert len(STRATEGIES) >= 10


def test_readme_lists_every_example_with_its_result():
    readme = (EXAMPLES / "README.md").read_text()
    for path in STRATEGIES:
        row = next((ln for ln in readme.splitlines() if ln.startswith(f"| [{path.name}]({path.name}) |")), None)
        assert row, f"examples/README.md has no row for {path.name}"
        sharpe, ret = [c.strip() for c in row.strip("|").split("|")][2:4]
        assert sharpe and ret, f"examples/README.md has no result for {path.name}"


@pytest.mark.parametrize("path", STRATEGIES, ids=lambda p: p.stem)
def test_example_loads_and_passes_the_check(path):
    s = load(path)
    assert s.warmup_ms > 0  # the check ran every step on an empty history
    assert packages.check_imports(path.read_text()) == []


@needs_data
@pytest.mark.parametrize("path", STRATEGIES, ids=lambda p: p.stem)
def test_example_trades(path):
    r = oxb.backtest(load(path), "2025-07-01", "2025-08-01", str(DATA), CONFIG)
    assert r.fills, path.name
    if path.stem == "exits":
        assert r.exits


@needs_data
def test_evaluate_runs(tmp_path):
    module = oxb.load_strategy(EXAMPLES / "evaluate.py", name="example_evaluate")
    lines = []
    module.main(str(DATA), seeds=2, start="2025-06-01", end="2025-08-01", split_at="2025-07-15",
                train="30d", test="15d", out=lines.append)  # fmt: skip
    text = "\n".join(lines)
    for part in ("vs BTC", "flipped signal", "deflated Sharpe", "split at", "walk-forward"):
        assert part in text
