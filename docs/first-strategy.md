# Your first strategy

This walks one idea from a sentence to a verdict with the engine directly, no coding agent. You will write one Python file, run four kinds of command, and learn to read what comes back. Every number below is from a real run of these exact steps.

The idea: **when traders crowd one side of a perp, bet against them.** On Hyperliquid, a crowd shows up in the funding rate: when longs pile in, funding runs far above normal. So short when funding is unusually high, and buy when it is unusually low.

## 1. Install and get the data

You need Python 3.12 or newer and an 0xArchive API key ([sign up](https://www.0xarchive.io/signup)). This tutorial reads 15 months of history, so it needs a [paid plan](https://www.0xarchive.io/pricing): free keys read the last 30 days.

```bash
mkdir funding-fade && cd funding-fade
pip install "0xarchive-backtest[curated]"
export OXARCHIVE_API_KEY="0xa_your_api_key"
0xbt data pull --markets BTC,ETH,SOL --from 2025-03 --to 2026-09
```

`data pull` writes one file per market per month into `data/hyperliquid` (about 250 MB for these three) and takes a few minutes. You pull once; every backtest after that reads the files and runs offline. `0xbt data check` confirms what you have.

Each file has a row per minute: prices and volume, taker buys and sells, open interest, funding and more. [data.md](../skills/backtest/references/data.md) lists every column and the date it starts.

## 2. Write the idea as code

Save this as `funding_fade.py`:

```python
# When traders crowd one side of a perp, its funding rate runs far from normal.
# Bet against the crowd: short when funding is unusually high, long when unusually low.
import oxarchive_backtest as oxb
from oxarchive_backtest import x


def strategy(lookback="14d", z=2.0, clock="1h"):
    def signal(h):
        funding = h.funding_rate
        # How unusual is funding right now, against its own last two weeks?
        score = (funding.last() - funding.mean(lookback)) / funding.std(lookback)
        # Only trade when it is extreme, and take the other side.
        return (-score).where(abs(score) >= z)

    return oxb.Strategy(
        universe=["BTC", "ETH", "SOL"],
        clock=clock,
        signal=signal,
        weights=x.sign(1 / 3),
    )
```

What each part does:

- **`strategy(...)`** is a function whose arguments are the settings you might change. The command line sets them with `-p name=value`.
- **`universe`** is the markets the strategy can trade.
- **`clock`** is how often it decides. `"1h"` means at the close of every hour.
- **`signal`** gets `h`, the history up to that hour, and returns a score per market. `h.funding_rate` is the funding column; `.last()`, `.mean("14d")` and `.std("14d")` read it over a window. A score of 2.5 means funding is 2.5 standard deviations above its two-week average. `.where(...)` blanks the markets that aren't extreme, and a blank means no position.
- **`weights`** turns scores into positions. `x.sign(1 / 3)` puts a third of the account long where the score is positive and a third short where it is negative.

The engine handles the rest: it never shows the strategy a bar that hasn't closed, fills orders on the next minute with Hyperliquid's fees and slippage, and charges or pays funding while a position is open.

## 3. Start a study

```bash
0xbt study init --holdout 2026-07-01
```

A study keeps two things honest. **It records every backtest you run** in `.0xarchive/trials.jsonl`, so later checks know how many tries it took. **It seals the data from 2026-07-01 on**, so you can't peek at the last three months while you tinker. Any run that reaches past that date stops with an error. You get to test on those months once, at the end.

## 4. Run it

```bash
0xbt backtest funding_fade.py --start 2025-04-01 --end 2026-07-01
```

```text
3 markets; return -6.60%, sharpe -0.37, max drawdown +19.79%; 2892 fills, fees 2384.85, funding paid -233.95, 0 liquidation(s); final equity 9339.93
result_hash=15798bc56ac94d46 receipt_id=79d81a...
```

It lost 6.6% of a $10,000 account. Read the line left to right:

- **sharpe -0.37**: return per unit of risk, per year. Above 1 is good, below 0 lost money.
- **max drawdown 19.79%**: the worst fall from a high.
- **2892 fills, fees 2384.85**: it paid $2,385 in trading fees, about a quarter of the account.
- **funding paid -233.95**: negative means it *received* $234 of funding. Being on the other side of the crowd gets paid.
- **result_hash**: the same file, data and settings give the same hash on any machine. Quote it and anyone can rerun your number.

The fees look like the problem. Check by running it again with costs switched off:

```bash
0xbt backtest funding_fade.py --start 2025-04-01 --end 2026-07-01 -c taker_fee_bps=0 -c fallback_slippage_bps=0
```

```text
3 markets; return +31.46%, sharpe 1.77, max drawdown +10.35%; 2891 fills, ...
```

**Before costs the idea makes 31%. After costs it loses 6.6%.** The signal has something; trading it every hour throws it away in fees, because funding crosses the threshold back and forth and each crossing is a trade. That zero-cost run is a diagnosis, not a result: nobody trades for free.

## 5. Change one thing

Decide every 8 hours instead of every hour:

```bash
0xbt backtest funding_fade.py --start 2025-04-01 --end 2026-07-01 -p clock=8h
```

```text
3 markets; return +3.25%, sharpe 0.27, max drawdown +15.03%; 495 fills, fees 648.88, funding paid -174.62, ...
```

Fills drop from 2,892 to 495 and fees from $2,385 to $649. It now makes 3.3%. Small, but positive after costs.

```bash
0xbt study status
```

```text
3 trial(s), 0 check run(s)
  sharpe   1.77  2025-04-01 to 2026-07-01  funding_fade.py {}  af64d21307a69602
  sharpe   0.27  2025-04-01 to 2026-07-01  funding_fade.py {"clock": "8h"}  41afb40907e205cb
  sharpe  -0.37  2025-04-01 to 2026-07-01  funding_fade.py {}  15798bc56ac94d46
```

Three tries, all on the record, the zero-cost one included. That count matters in the next step.

## 6. Run the gauntlet

A positive backtest isn't evidence yet. The gauntlet asks whether it beats luck:

```bash
0xbt gauntlet funding_fade.py --start 2025-04-01 --end 2026-07-01 -p clock=8h --card card.md
```

It takes about 20 seconds here and writes `card.md`:

```text
Verdict: in-sample only (rung 3 of 6)

| After costs     | Sharpe 0.27, return +3.25%, max drawdown 15.03%, fees 648.88, funding paid -174.62 |
| Against BTC     | alpha +4.93% a year, beta 0.06, information ratio 0.70 |
| Nulls           | flipped Sharpe -1.40; random mean -2.46, +0.00% at or above; stale mean -1.06, +0.00% at or above |
| Split           | Sharpe 0.02 before 2026-01-30, 1.14 after |
| Walk-forward    | 5 folds, Sharpe -0.13, return -1.31% |
| Deflated Sharpe | 0.23 over 3 trial(s); luck's best: Sharpe 0.94 |

Notes:
- one market: without SOL the strategy made nothing
```

The verdict is the first rung on a fixed ladder that the strategy fails. Read the checks top to bottom:

- **After costs**: positive, so it clears rung 1.
- **Nulls**: the same strategy with the signal flipped loses (Sharpe -1.40), and none of the random or stale versions did as well. So the funding signal carries real information, and it clears rung 2.
- **Out of sample**: the split looks fine, but **walk-forward loses**. Walk-forward picks settings on six months, trades the next two, and steps through the whole window five times. Run that way, the strategy lost 1.3%. That stops it at **rung 3, in-sample only**.
- **Deflated Sharpe 0.23**: after three tries, luck alone would be expected to produce a best Sharpe of 0.94, and 0.27 is well short of that. Passing needs 0.95.
- **The SOL note**: more than half the gains came from SOL, and without it the strategy made nothing. It is closer to a bet on one market than a pattern across three.

So the honest answer is: **crowded funding does point the wrong way, but this version doesn't make money reliably.** That isn't a failed tutorial. It is the job: you found out in an afternoon, without risking anything, and every number has a hash someone else can rerun. [gauntlet.md](../skills/backtest/references/gauntlet.md) covers every check and note on the card.

Don't open the holdout for this one. A strategy that stops at rung 3 has nothing worth spending it on.

## 7. Where to go from here

- **Try your own idea.** Copy `funding_fade.py`, change the signal, keep the study. [strategy-api.md](../skills/backtest/references/strategy-api.md) lists everything `h` can read and every piece in `x`: exits such as `x.stop_loss`, session clocks such as `x.at_open("us")`, risk limits, and your own orders with `step=`.
- **Test more settings at once.** `-g` on the gauntlet runs a grid, for example `-g z=1.5,2,2.5 -g clock=4h,8h`. Every point counts as a try, so a big grid needs a bigger result to pass.
- **Open the holdout once you have a candidate that passes.** `0xbt study commit funding_fade.py -p clock=8h` runs it on the sealed months, once. Do it when you would bet on the result, not to see how it does.
- **Let your agent do the legwork.** The [backtest skill](../README.md#install) runs this same loop from a sentence, including the spec, the study and the verdict, and reads the card back to you.
- **See the pieces one at a time.** [The examples](../skills/backtest/engine/examples/README.md) show every part of the API, one file each. They are demos, not strategies, and most lose money.
