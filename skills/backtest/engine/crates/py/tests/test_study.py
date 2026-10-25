"""Studies: the trial ledger, the sealed holdout, commit, and the gauntlet's verdict."""

import json

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import x
from oxarchive_backtest.cli import main
from refdata import CONFIG, HOUR, T0, end, write

DAY = 86_400_000
N = 6 * 1440
HOLDOUT = T0 + 4 * DAY


def momentum(lookback=3):
    return oxb.Strategy(universe=["A", "B"], clock="4h",
                        signal=lambda h: h.close.ret(lookback),
                        weights=x.long_short(0.5))  # fmt: skip


@pytest.fixture
def data(tmp_path):
    root = tmp_path / "data"
    write(root, "A", N, "hyperliquid:A", seed=1)
    write(root, "B", N, "hyperliquid:B", seed=2)
    return str(root)


@pytest.fixture
def study(tmp_path, data, monkeypatch):
    monkeypatch.delenv("OXS_STUDY", raising=False)
    monkeypatch.chdir(tmp_path)
    return oxb.Study.init(tmp_path, holdout=HOLDOUT, source=data)


def test_no_study_records_nothing(tmp_path, data, monkeypatch):
    monkeypatch.delenv("OXS_STUDY", raising=False)
    monkeypatch.chdir(tmp_path)
    oxb.backtest(momentum(), T0, end(N), data, CONFIG)
    assert not (tmp_path / ".0xarchive").exists()


def test_every_run_is_a_trial_once(study, data):
    oxb.backtest(momentum, T0, HOLDOUT, data, CONFIG, params={"lookback": 3})
    oxb.backtest(momentum, T0, HOLDOUT, data, CONFIG, params={"lookback": 3})
    oxb.backtest(momentum, T0, HOLDOUT, data, CONFIG, params={"lookback": 6})
    # The same strategy over another window is another try.
    oxb.backtest(momentum, T0 + DAY, HOLDOUT, data, CONFIG, params={"lookback": 3})
    entries = study.entries()
    assert len(entries) == 4 and {e["kind"] for e in entries} == {"trial"}
    trials = study.trials()
    assert len(trials) == 3
    assert trials[0]["params"] == {"lookback": 3} and trials[0]["days"] > 0
    assert all(not e["after_reveal"] for e in entries)


def test_checks_are_not_trials(study, data):
    oxb.backtest(momentum(), T0, HOLDOUT, data, CONFIG, benchmark="A")
    oxb.null_test(momentum(), T0, HOLDOUT, data, CONFIG, seeds=2)
    oxb.split(momentum, {"lookback": [3, 6]}, T0, HOLDOUT, T0 + 3 * DAY, data, CONFIG)
    kinds = [e["kind"] for e in study.entries()]
    assert kinds.count("trial") == 1 and kinds.count("check") == len(kinds) - 1


def test_deflated_sharpe_counts_the_ledger(study, data):
    for lb in (2, 3, 4, 6):
        r = oxb.backtest(momentum, T0, HOLDOUT, data, CONFIG, params={"lookback": lb})
    d = study.deflated_sharpe(r)
    assert d["trials"] == 4
    assert d == oxb.deflated_sharpe(r, [t["sharpe"] for t in study.trials()])


def test_the_holdout_is_sealed(study, data):
    with pytest.raises(oxb.Sealed, match="sealed"):
        oxb.backtest(momentum(), T0, end(N), data, CONFIG)
    with pytest.raises(oxb.Sealed):
        oxb.sweep(momentum, T0, HOLDOUT + HOUR, {"lookback": [3]}, data, CONFIG)
    session = oxb.Session(momentum(), ["A", "B"], start=T0 + DAY)
    with pytest.raises(oxb.Sealed):
        session.warm_up(data)
    # Up to the holdout is fine, and a failed run records nothing.
    oxb.backtest(momentum(), T0, HOLDOUT, data, CONFIG)
    assert len(study.entries()) == 1


def test_commit_opens_the_holdout_once(study, data):
    oxb.backtest(momentum, T0, HOLDOUT, data, CONFIG, params={"lookback": 3})
    report = study.commit(momentum, params={"lookback": 3}, config=CONFIG)
    assert not study.sealed
    r = study.holdout_result
    assert r["result_hash"] == report.result_hash and r["passed"] == (r["sharpe"] is not None and r["sharpe"] > 0)
    assert study.commit_record["params"] == {"lookback": 3}
    assert study.entries()[-1]["kind"] == "holdout"
    with pytest.raises(ValueError, match="already open"):
        study.commit(momentum, params={"lookback": 3}, config=CONFIG)
    # Open now, and every run after the look is marked.
    oxb.backtest(momentum(), T0, end(N), data, CONFIG)
    assert study.entries()[-1]["after_reveal"]
    # The state survives a reload.
    assert not oxb.Study.find().sealed


def test_a_broken_study_never_unseals(tmp_path, monkeypatch, data):
    monkeypatch.setenv("OXS_STUDY", str(tmp_path / "nowhere"))
    with pytest.raises(FileNotFoundError):
        oxb.backtest(momentum(), T0, end(N), data, CONFIG)


def test_gauntlet_verdict(study, data):
    g = oxb.gauntlet(momentum, {"lookback": [2, 3, 6]}, T0, HOLDOUT, data, CONFIG, seeds=2,
                     benchmark="A")  # fmt: skip
    v = g["verdict"]
    assert v["rung"] == oxb.LADDER[v["level"] - 1]
    assert g["deflated"]["over"] == "study" and g["deflated"]["trials"] == 3
    assert g["robustness"]["grid_points"] == 3
    assert g["holdout"]["sealed"] and g["walk_forward"] is None
    assert any("walk-forward not run" in n for n in v["notes"])
    # Only the grid counts as trials; its checks don't.
    assert len(study.trials()) == 3
    # The candidate is the grid's best Sharpe.
    best = max(g["grid"], key=lambda r: r["sharpe"] if r["sharpe"] is not None else -1e9)
    assert g["candidate"]["params"] == best["params"]
    if v["level"] > 1:
        assert g["nulls"] is not None


def test_gauntlet_rejects_a_losing_strategy(tmp_path, data, monkeypatch):
    monkeypatch.delenv("OXS_STUDY", raising=False)
    monkeypatch.chdir(tmp_path)
    g = oxb.gauntlet(lambda: momentum().flipped(), None, T0, end(N), data,
                     oxb.Config(builder_fee_bps=500.0), seeds=1, benchmark="A")  # fmt: skip
    assert g["verdict"]["level"] == 1 and g["deflated"]["over"] == "grid"


STRATEGY = """
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(lookback=3):
    return oxb.Strategy(universe=["A", "B"], clock="4h",
                        signal=lambda h: h.close.ret(lookback),
                        weights=x.long_short(0.5))
"""


def test_cli_study_and_gauntlet(tmp_path, data, monkeypatch, capsys):
    monkeypatch.delenv("OXS_STUDY", raising=False)
    monkeypatch.chdir(tmp_path)
    (tmp_path / "s.py").write_text(STRATEGY)
    assert main(["study", "init", "--holdout", str(HOLDOUT), "--data", data]) == 0
    with pytest.raises(SystemExit, match="sealed"):
        main(["backtest", "s.py", "--start", str(T0), "--end", str(end(N)), "--data", data])
    window = ["--start", str(T0), "--end", str(HOLDOUT), "--data", data]
    assert main(["gauntlet", "s.py", *window, "-g", "lookback=2,3", "--seeds", "1",
                 "--benchmark", "A", "--card", "card.md", "--json", "g.json"]) == 0  # fmt: skip
    card = (tmp_path / "card.md").read_text()
    assert card.startswith("# s.py: ") and "Deflated Sharpe" in card and "the study ledger" in card
    assert json.loads((tmp_path / "g.json").read_text())["receipt_id"] in card
    st = oxb.Study.find()
    assert {t["file"] for t in st.trials()} == {"s.py"} and len(st.trials()) == 2
    capsys.readouterr()
    assert main(["study", "status"]) == 0
    assert "2 trial(s)" in capsys.readouterr().out
    assert main(["study", "commit", "s.py", "-p", "lookback=3", "--data", data]) == 0
    out = capsys.readouterr()
    assert "one look" in out.out and "not in the ledger" not in out.err
    assert st.__class__.find().commit_record["was_tried"] is True
    with pytest.raises(SystemExit, match="already open"):
        main(["study", "commit", "s.py", "-p", "lookback=3", "--data", data])


def test_research_rows_respect_the_seal(study, data):
    src = oxb.Source(data)
    rows = src.rows("A", T0, T0 + HOUR, columns=["close"])
    assert len(rows) == 60 and rows[0]["ts"] == T0 and rows[0]["close"] is not None
    with pytest.raises(oxb.Sealed):
        src.rows("A", T0, end(N))


def test_nulls_redraw_once_per_period(tmp_path, data, monkeypatch):
    monkeypatch.delenv("OXS_STUDY", raising=False)
    monkeypatch.chdir(tmp_path)
    # The scores are the weights, traded only when they move, so fills count redraws.
    s = momentum().replace(clock="1h", weights=lambda s: s * 0.4,
                           execute=x.rebalance(band=0.05))  # fmt: skip
    fast = oxb.backtest(s.random_signal(0), T0, end(N), data, CONFIG)
    slow = oxb.backtest(s.random_signal(0, every="1d"), T0, end(N), data, CONFIG)
    # Drawn once a day, the null trades far less than one drawn every hour.
    assert len(slow.fills) < len(fast.fills) / 3
    # Stale lags kept for a day flip the book less often than lags drawn every hour.
    signs = momentum().replace(clock="1h", weights=x.sign(0.4), execute=x.rebalance(band=0.05))
    held = oxb.backtest(signs.stale_signal(0, within="12h", every="1d"), T0, end(N), data, CONFIG)
    drawn = oxb.backtest(signs.stale_signal(0, within="12h"), T0, end(N), data, CONFIG)
    assert 0 < len(held.fills) < len(drawn.fills)
    # Without `every`, nothing changes.
    assert fast.result_hash == oxb.backtest(s.random_signal(0, every=None), T0, end(N), data,
                                            CONFIG).result_hash  # fmt: skip


def test_holding_period():
    from oxarchive_backtest.evaluate import holding_period

    class F:
        def __init__(self, ts, market="A"):
            self.ts, self.market = ts, market

    class R:
        def __init__(self, fills):
            self.fills = fills

    day = DAY
    # Two fills a minute apart are one trade; trades every 2 days.
    fills = [F(i * 2 * day + k * 60_000) for i in range(5) for k in range(2)]
    assert holding_period(R(fills)) == 2 * day
    assert holding_period(R(fills[:4])) is None
