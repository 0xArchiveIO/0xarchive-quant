"""Reads checked against our factor research's pandas originals  on
the real dataset: the 60-day beta to BTC and the 15-day residual return behind resid_mom,
computed in pandas from the same hourly closes the engine sees.

Needs data/hyperliquid; skipped without it.
"""

import pathlib

import pytest

import oxarchive_backtest as oxb
from oxarchive_backtest import x

DATA = pathlib.Path(__file__).resolve().parents[3] / "data" / "hyperliquid"
MARKETS = ["BTC", "ETH", "SOL", "DOGE", "AVAX", "LINK"]


@pytest.mark.skipif(not (DATA / "BTC").is_dir(), reason="needs the dataset in data/hyperliquid")
def test_beta_and_residuals_match_signal_lab():
    pd = pytest.importorskip("pandas")
    W, N = 24 * 60, 24 * 15
    got = {}

    def step(ctx, h):
        r = h.close.resid("15d", to="BTC", fit="60d")
        b = h.close.beta("60d", to="BTC")
        w = h.close.window(W + N + 2)
        if ctx.time:
            got.update(r=r, b=b, w=w)

    s = oxb.Strategy(universe=MARKETS, clock=x.every("7d", anchor="2026-06-01"), step=step)
    oxb.backtest(s, "2026-06-01", "2026-06-01 00:01", str(DATA))
    cl = pd.DataFrame({m: got["w"][m] for m in MARKETS}, dtype="float64")
    ret = cl.pct_change(fill_method=None).where(lambda v: v.abs() < 0.5).fillna(0.0)
    b = ret["BTC"]
    eb, eb2 = b.rolling(W).mean(), (b**2).rolling(W).mean()
    cov = ret.mul(b, axis=0).rolling(W).mean().sub(ret.rolling(W).mean().mul(eb, axis=0))
    beta = cov.div(eb2 - eb**2, axis=0)
    resid = ret.sub(beta.shift(1).mul(b, axis=0)).rolling(N).sum().iloc[-1]
    for m in MARKETS:
        assert got["b"][m] == pytest.approx(beta.iloc[-1][m], rel=1e-12, abs=1e-15), m
        assert got["r"][m] == pytest.approx(resid[m], rel=1e-9, abs=1e-14), m
