# Examples

One runnable strategy per file, together using every kind of piece in `oxarchive_backtest`. Each file defines `strategy` (a function whose arguments are its parameters), so it runs anywhere a strategy runs:

```bash
0xbt backtest examples/momentum.py --start 2025-03-01 --end 2026-10-01 -c builder_fee_bps=1
```

The demo's dropdown loads them from here. `crates/py/tests/test_examples.py` loads each one the way the browser does and, with the dataset in `data/hyperliquid`, runs it for a month, so they can't go stale.

The results are over the full dataset (62 markets, 2025-03-01 to 2026-10-01), $100k, builder fee 1 bps. Several lose money. They are examples of the pieces, not strategies to run, and each file's header says why it did what it did.

| File | Shows | Sharpe | Return |
|---|---|---|---|
| [momentum.py](momentum.py) | `x.top_n`, `x.every`, `ret`, `vol`, `shift`, `x.long_short(by=)`. The reference strategy (hash `0ed429d550204eda`) | 2.09 | +78% |
| [momentum_fixed.py](momentum_fixed.py) | a fixed list of markets | 1.07 | +65% |
| [exits.py](exits.py) | `x.stop_loss`, `x.take_profit`, `x.trailing_stop`, `x.time_stop`, `x.drawdown_stop(cooldown=)`, `report.exits` | 0.47 | +9% |
| [residual_momentum.py](residual_momentum.py) | `resid`, `x.beta_neutral`, `x.drawdown_stop` | 0.76 | +19% |
| [trend_blend.py](trend_blend.py) | `h.bars("1d")`, `logret`, `ewma`, `beta`, `corr`, `x.blend`, `Series.neutralize`, `demean(by=)`, `where`, `x.weekly` | 0.77 | +24% |
| [funding_carry.py](funding_carry.py) | `x.top_n(by="open_interest", min_volume=, exclude=, sectors=, per_sector=)`, `on_universe`, `h.funding_rate`, `x.book`, `x.cap`, `x.rebalance(max_cost=)` | -0.71 | -21% |
| [own_universe.py](own_universe.py) | `x.rule` and its view, `x.long_only(top=)`, `x.cap`, `x.hedge`, `x.vol_target`, `x.turnover` | 0.80 | +21% |
| [us_open.py](us_open.py) | `x.at_open("us", after=)`, `h.now`, `x.only(condition)`, `x.time_stop` | -1.96 | -25% |
| [weekdays.py](weekdays.py) | `x.daily`, `delay=`, `x.only(..., outside="flat")`, `x.rebalance(band=)` | -0.36 | -12% |
| [monthly_index.py](monthly_index.py) | `x.monthly`, `hold=`, `repair=True`, `x.long_only(by=)` | 0.13 | -14% |
| [seasonality.py](seasonality.py) | `seasonal`, `x.only("us")` | -15.8 | -92% |
| [mean_cross.py](mean_cross.py) | one market, `mean`, `x.sign` | -0.34 | -18% |
| [leader_laggard.py](leader_laggard.py) | your own weights function, `Series.top`/`bottom` | -1.09 | -30% |
| [breakout.py](breakout.py) | `step=`: your own orders, `ctx.market`, `ctx.stop`, `ctx.state` | 0.44 | +13% |
| [evaluate.py](evaluate.py) | `benchmark=`, `report.vs`, `report.attribution`, `oxb.null_test`, `oxb.sweep`, `oxb.deflated_sharpe`, `oxb.split`, `oxb.walk_forward` on momentum.py | | |

`evaluate.py` is a script, not a strategy: `python examples/evaluate.py`. It runs about 50 backtests, which takes 7 minutes. Its docstring has the output: the signal beats every null, and walk-forward picks 7 days in all 7 folds (Sharpe 1.98 out of sample). Over 8 tries, though, the deflated Sharpe is 0.86, below the usual 0.95 bar.

A few pieces work exactly like a neighbour shown here and have no file of their own:

- `x.at_close("us", before="15m")` works like `x.at_open`.
- `x.gross(1.0)` works like `x.book(gross=)`.
- `Series.zscore()` works like `rank()`.
- `ctx.limit` works like `ctx.stop`.
- `ctx.rebalance(weights)` and `ctx.cost(weights)` are what `x.rebalance` calls.
- `s.replace(clock="1d")` returns a copy with some arguments changed.

[references/strategy-api.md in the skill](../references/strategy-api.md in the skill) documents every piece.
