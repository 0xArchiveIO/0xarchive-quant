"""A two-market strategy, and the portfolio check check-pyodide.sh runs natively and in
Pyodide: a portfolio backtest and a paper session fed the same rows must give one hash,
the same on both."""

import oxarchive_backtest as oxb


def pairs(a="A", b="B", lookback=20, size=50.0, fills=None):
    """Long A and short B when A's 20-bar return beats B's, flat otherwise."""

    def step(ctx, h):
        r = h.close.ret(lookback)
        if r[a] is None or r[b] is None:
            return
        want = size if r[a] > r[b] else 0.0
        pos = ctx.positions
        for name, target in ((a, want), (b, -want)):
            delta = target - pos[name]
            if delta != 0.0:
                try:
                    ctx.market(name, delta)
                except ValueError:
                    pass

    on_fill = (lambda ctx, f: fills.append((f.market, f.size))) if fills is not None else None
    return oxb.Strategy(universe=[a, b], clock="1m", bars="1m", step=step, on_fill=on_fill)


def run(oxb, root):
    """(portfolio backtest hash, paper session hash) over two synthetic markets of
    different lengths, written to `root`."""
    from refdata import CONFIG, T0, end, rows, write

    write(root, "A", 4_000)
    write(root, "B", 3_000, seed=7)
    bt = oxb.backtest(pairs(), T0, end(4_000), root, CONFIG, max_leverage={"B": 20})
    s = oxb.Session(pairs(), {"A": "synthetic:reference", "B": "synthetic:reference"},
                    config=CONFIG, max_leverage={"B": 20})  # fmt: skip
    ra, rb = rows(root, "A"), rows(root, "B")
    for i, x in enumerate(ra):
        s.step({"A": x, "B": rb[i]} if i < len(rb) else {"A": x})
    assert len(bt.fills) > 20, "the check must trade"
    return bt.result_hash, s.report().result_hash
