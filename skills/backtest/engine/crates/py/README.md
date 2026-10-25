# 0xarchive-backtest

<img alt="0xArchive backtest: a strategy's equity curve beating 12 random books" src="https://raw.githubusercontent.com/0xArchiveIO/0xarchive-quant/main/assets/readme/hero-light.png" width="800">

A deterministic backtester for Hyperliquid perps, on 0xArchive data. The engine is Rust; strategies
are Python. The same strategy, data and parameters give the same result hash on every machine, so a
result is a claim anyone can check.

It is the engine behind [0xarchive-quant](https://github.com/0xArchiveIO/0xarchive-quant), a quant
research skill for coding agents: hand Claude Code or Codex a wallet, an idea or a script, and it comes
back with a verdict you can rerun.

## Install

```bash
pip install "0xarchive-backtest[curated]"
```

Prebuilt for macOS, Linux and Windows; Python 3.12 or later (3.11 without `[curated]`, the pinned
numpy, pandas and scipy that strategies may import).

## Quickstart

Pull a minute dataset with your [0xArchive API key](https://www.0xarchive.io/pricing). Months are
inclusive. Free keys read only the last 30 days, so this September window needs a paid plan:

```bash
export OXARCHIVE_API_KEY="0xa_your_api_key"
0xbt data pull --markets BTC,ETH,SOL,HYPE --from 2026-09 --to 2026-09
```

Write a strategy:

```python
# momentum.py: each day, long the strongest of four perps over 3 days and short the weakest
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(lookback="3d"):
    return oxb.Strategy(
        universe=["BTC", "ETH", "SOL", "HYPE"],
        clock=x.daily(),
        signal=lambda h: h.close.ret(lookback),
        weights=x.long_short(0.25),
    )
```

Backtest it, then put it through the gauntlet:

```bash
0xbt backtest momentum.py --start 2026-09-01 --end 2026-10-01
0xbt gauntlet momentum.py --start 2026-09-01 --end 2026-10-01 \
  -g lookback=1d,3d --card card.md
```

The backtest prints the return, Sharpe, drawdown, fees and funding, and a result hash. The gauntlet
runs every check on the best setting and reads the verdict off a fixed ladder. This one stops at
`fails nulls`: one in ten random books did as well, over the 5% bar, so a month of momentum on four
markets is not an edge.

## The gauntlet

<img alt="A gauntlet report card: verdict holds out of sample, deflated Sharpe 0.93 over 4 tries, short of the 0.95 bar" src="https://raw.githubusercontent.com/0xArchiveIO/0xarchive-quant/main/assets/readme/card-light.png" width="800">

`0xbt gauntlet` runs, in order: the grid as trials; costs and a benchmark; nulls (the flipped signal,
random books and the strategy's own signal at random lags, redrawn at its holding period); a split and
walk-forward; the deflated Sharpe over every try; and robustness. The verdict is the first rung whose
rule fails:

| Rung | Verdict |
|---|---|
| 1 | loses after costs |
| 2 | fails nulls |
| 3 | in-sample only |
| 4 | holds out of sample |
| 5 | passes deflated Sharpe |
| 6 | passes sealed holdout |

`0xbt study init` starts a trial ledger: every backtest below it is recorded, the deflated Sharpe counts
all of them, and the last months of data stay sealed until one candidate is committed, once.

## What the engine models

Minute bars with taker flow, funding, open interest and liquidations; taker and maker fees; funding
payments; leverage limits and liquidation; point-in-time universes (`x.top_n`, `x.rule`) so delisted
markets are included; stops, take-profits, trailing and time exits checked every minute; and
deterministic math, so results don't depend on the platform.

## Links

- [Source, the agent skill and examples](https://github.com/0xArchiveIO/0xarchive-quant)
- [The strategy API](https://github.com/0xArchiveIO/0xarchive-quant/blob/main/skills/backtest/references/strategy-api.md)
- [0xArchive](https://www.0xarchive.io)

MIT licensed.
