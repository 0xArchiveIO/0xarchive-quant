"""Point-in-time universes: the built-in rule, your own, and what a strategy sees.

Run: python -m pytest crates/py/tests  (after installing the wheel)
"""

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import x
from refdata import CONFIG, HOUR, MIN, T0, end, rows, write

DAY = 86_400_000
N = 4000  # minutes, about 2.8 days


@pytest.fixture
def three(tmp_path):
    """Three markets with dollar volumes B > C > A."""
    for name, scale, qv in (("A", 1.0, 1.0), ("B", 1.1, 3.0), ("C", 0.9, 2.0)):
        write(tmp_path, name, N, f"hyperliquid:{name}", scale, qv)
    return str(tmp_path)


def watcher(universe, seen):
    def step(ctx, h):
        seen[ctx.time] = (list(h.markets), ctx.universe)

    return oxb.Strategy(universe=universe, clock="1h", step=step)


def test_top_n_picks_from_earlier_hours(three):
    rule = x.top_n(2, window="1d", min_history="1d", every="1d", anchor=T0)
    s = watcher(rule, {})
    u, files = oxb.picks(s, T0, end(N), three)
    assert u.picks[0] == (T0 + DAY, ["B", "C"])  # the first day is history
    assert u.members(T0 + DAY - 1) == [] and u.members(T0 + DAY) == ["B", "C"]
    assert u.markets_between(T0, end(N)) == ["B", "C"]
    back = oxb.Universe.from_json(u.json)
    assert back.picks == u.picks
    assert len(files) == 3  # one month of each market
    with pytest.raises(ValueError, match="time order"):
        oxb.Universe.from_json('{"rule": null, "picks": [{"at": 2, "members": []}, {"at": 1, "members": []}]}')


def test_a_run_trades_only_the_members_it_had(three):
    rule = x.top_n(1, window="1d", min_history="1d", every="1d", anchor=T0)
    seen = {}
    r = oxb.backtest(watcher(rule, seen), T0, end(N), three, CONFIG)
    # Markets are the members over the window, not every market of the source.
    assert set(r.final_positions) == {"B"}
    assert seen[T0 + DAY - 2 * HOUR] == ([], [])
    assert seen[T0 + DAY + HOUR] == (["B"], ["B"])
    # A top-1 rule picks at once (it needs half of n eligible: none), then B.
    assert r.universe.picks[:2] == [(T0, []), (T0 + DAY, ["B"])]


def test_hourly_files_give_the_same_picks(three, tmp_path):
    from oxarchive_backtest.cli import main

    rule = x.top_n(2, window="1d", min_history="1d", every="1d", anchor=T0)
    s = watcher(rule, {})
    before, _ = oxb.picks(s, T0, end(N), three)
    assert main(["hourly", "--data", three]) == 0
    src = oxb.Source(three)
    assert src.has_hourly()
    after, files = oxb.picks(s, T0, end(N), src)
    assert after.picks == before.picks
    assert all(f[2] == f"lcg-{N}+1h" for f in files)


def test_own_rule_sees_only_hours_before_the_pick(three):
    seen = []

    def cheapest_two(view):
        for m in view.markets:
            closes = view.closes(m, 24)
            assert len(closes) == 24
            seen.append((view.at, view.history_hours(m)))
        return sorted(view.markets, key=lambda m: view.volume(m, 24))[:2]

    s = watcher(x.rule(cheapest_two, every="1d", anchor=T0), {})
    u, _ = oxb.picks(s, T0, end(N), three)
    assert u.picks[0] == (T0, [])  # at the first pick there is no history yet
    assert all(h <= (at - T0) // HOUR for at, h in seen)
    assert u.members(T0 + DAY) == ["A", "C"]
    bad = watcher(x.rule(lambda v: ["Z"], anchor=T0), {})
    with pytest.raises(ValueError, match="not a market of the run"):
        oxb.picks(bad, T0, end(N), three)
    bad = watcher(x.rule(lambda v: 5, anchor=T0), {})
    with pytest.raises(ValueError, match="list of market names"):
        oxb.picks(bad, T0, end(N), three)


def test_a_session_picks_as_it_goes_and_matches_the_backtest(three):
    """Live, the rule picks from the rows the session has seen when each pick is due:
    the same picks and the same run as the backtest."""
    rule = x.top_n(1, window="1d", min_history="1d", every="1d", anchor=T0)
    strategy = oxb.Strategy(universe=rule, clock="1h", signal=lambda h: h.close.ret(3),
                            weights=x.sign())  # fmt: skip
    bt = oxb.backtest(strategy, T0, end(N), three, CONFIG)
    assert len(bt.fills) > 0
    s = oxb.Session(strategy, {m: f"hyperliquid:{m}" for m in "ABC"}, start=T0, config=CONFIG)
    rs = {m: rows(three, m) for m in "ABC"}
    for i in range(N):
        s.step({m: rs[m][i] for m in "ABC"})
    assert s.report().universe.picks == bt.universe.picks
    # The session trades all three markets' engine but only B is ever held.
    assert s.report().result_hash != "" and s.ctx.universe == ["B"]
    assert {f.market for f in s.report().fills} == {f.market for f in bt.fills} == {"B"}


def test_a_window_after_the_first_picks_loads_only_its_members(three):
    """Picks before the window may hold markets the run doesn't load."""
    rule = x.top_n(1, window="1d", min_history="1d", every="1d", anchor=T0)
    seen = {}
    start = T0 + DAY + 2 * HOUR
    r = oxb.backtest(watcher(rule, seen), start, end(N), three, CONFIG)
    assert set(r.final_positions) == {"B"}
    assert r.universe.picks[0] == (T0 + DAY, ["B"])  # the pick in force at start


def test_top_n_filters_and_the_pick_views_fields(three):
    rule = x.top_n(2, window="1d", min_history="1d", every="1d", anchor=T0, exclude=["B"])
    u, _ = oxb.picks(watcher(rule, {}), T0, end(N), three)
    assert u.picks[0] == (T0 + DAY, ["C", "A"])
    rule = x.top_n(3, window="1d", min_history="1d", every="1d", anchor=T0,
                   sectors={"B": "l1", "C": "l1"}, per_sector=1)  # fmt: skip
    u, _ = oxb.picks(watcher(rule, {}), T0, end(N), three)
    assert u.picks[0][1] == ["B", "A"]
    with pytest.raises(ValueError, match="'volume' or"):
        x.top_n(by="funding")
    seen = {}

    def look(view):
        m = view.markets[0]
        seen.update(vol=view.volatility(m, 24), listed=view.listed(m), oi=view.open_interest(m),
                    funding=view.funding(m, 24), at=view.at)  # fmt: skip
        return None

    oxb.picks(watcher(x.rule(look, every="1d", anchor=T0 + DAY), {}), T0, end(N), three)
    assert seen["vol"] > 0 and seen["listed"] == T0
    assert seen["oi"] is None and seen["funding"] is None  # the reference rows have neither
