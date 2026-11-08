# Reverse engineer a trader

The question has three parts, answered in this order:

1. **What is their record, honestly?** Their own book through our engine, with our fees and funding.
2. **Is it skill or luck?** Their book through the gauntlet: does it beat its nulls?
3. **What rules produce it?** A strategy written from their style, scored by how much of their book it
   reproduces, then put through the gauntlet itself.

The helper is `scripts/trader.py` beside this skill; the mirror strategy is `templates/mirror.py`.
Copy both into the study directory so the ledger records the copies you ran.

## 1. Pull the book

```bash
python3 trader.py fetch 0xADDRESS --start 2025-06-01 --end 2026-07-01 --data data/hyperliquid
python3 trader.py profile trader.json
```

- `fetch` reads `GET /v1/hyperliquid/wallets/{address}/positions/changes`: one row per fill that changed
  a position, signed (shorts negative), from 2025-05-25 on Hyperliquid core. A Hyperliquid vault is an
  account like any other: pass the vault's address. `--venue hip3` and
  `--venue lighter` (account index) read the same route on those venues, but the backtest dataset is
  Hyperliquid core only, so the mirror covers core markets alone.
- It bills like trades, one credit per 1,000 rows. A busy wallet over a year can be hundreds of thousands
  of rows: try one month first and tell the user the row count before pulling a year.
- The Free plan reads only the last 30 days. A `historical_depth_exceeded` error means the window needs
  a paid plan; say so rather than shortening the window silently.
- Wallet data is public venue data, but it is still about a person. Analyse the address the user gave;
  don't go looking for who owns it, and don't build lists of other people's wallets.

`profile` gives the style from the fills alone: markets, round trips, median and 90th percentile holding
time, long share of held time, largest gross position, maker share, when it opens (UTC hour, weekday),
realized PnL and fees. `not_in_dataset` lists markets the backtest can't trade; report what share of the
book they were.

Its `scaling` block shows how the book gets in and out of each round trip: the share of trips with 1, 2, 3...
orders, then for each add its move from the first fill (signed by side, so negative is against the
position) and its size against what was held, and where the position closes against its average entry.
Tight add moves and a tight exit are a fixed ladder (DCA, grid, martingale): adds that always land near
-2%, -4% and -6% with sizes near 1.0x held, and an exit always near +0.5%, are rules, not habits. `losing_trips` against
`closed_trips` says whether the book ever takes a loss; a ladder with no stop shows almost none, and its
risk is in the trips still open.

A book that never goes flat in a market has no round trips there, so `scaling` can't see it. When a
market shows many fills and no round trips (a position held all window), check whether it is traded as a grid:

```bash
python3 trader.py grid trader.json
```

It joins fills into orders (same side and price, within `--join-minutes` of each other), then reports the
most common order size and how many orders match it, the most common price step from one order to the
next and how many steps match it, how many fills sit on that price grid, and the range the position moved
in. A fixed grid around a held position (sell a set size every step up, buy it back every step down)
shows almost every order, step and fill matching (98% or more); a book with no grid shows a few
percent at most. Its money then comes from two places: the grid's buy-backs and
re-sells (`realized_pnl`, `reducing_orders_in_profit`), and the held position's direction, which the
grid never closes. Report both.

## 2. Their record, and skill or luck

```bash
0xbt study init --holdout 2026-07-01
0xbt backtest mirror.py -p path=trader.json --start 2025-06-01 --end 2026-07-01
0xbt gauntlet mirror.py -p path=trader.json --start 2025-06-01 --end 2026-07-01 --card trader-card.md
```

The mirror holds their signed position value in each market, scaled by their largest gross position so
the book never passes 1x, and samples it each hour (`-p clock=1m` follows within a minute and runs
slower). Its nulls read as questions about the trader:

- **flipped:** fading them. If fading does as well, the book has no direction to speak of.
- **random:** random positions in the same markets on the same clock: what the markets alone paid.
- **stale:** their own positions at random earlier times. Beating every stale run means their timing,
  not just their exposure, made the money. This is the "skill or luck" answer.

The mirror's PnL is not their PnL: it trades up to a clock tick late, at our fill model's prices, at a
fixed scale. Compare it with `realized_pnl` from the profile and say how far apart they are. A big gap
usually means fast trading the hourly mirror can't follow; rerun with `clock=1m`.

## 3. Find the rules

Look at where their entries sit before guessing rules. Start with price levels:

```bash
python3 trader.py entries trader.json --data data/hyperliquid
```

It measures every opening fill against hourly EMAs (on closed hours, and live: closed hours plus the
forming bar), hourly SMAs and the session VWAP, and ranks the references by how tightly the offsets cluster
(`dispersion`: interquartile range over the median's size). A rule like "enter 1% under the 12-hour
EMA" shows as one reference at the top with a tight cluster, most entries at that level and few ordinary
minutes there (`lift`). Neighbouring lengths score close together; report the range, not just the winner.

Then, for what price levels don't explain, read the minutes around each opening fill with
`oxb.Source(DIR).rows(market, t - 6h, t + 1h, columns=[...])` and tabulate what was true at entry against
all other hours: recent return, funding rate, open interest change, taker flow (`cvd_delta`), spread and
depth, liquidations, the hour of day. A feature that separates their entries from ordinary hours is a
candidate rule; one that doesn't is not, however plausible it sounds. A filter on which markets it trades
shows the same way: rank the markets each hour (by volatility, volume) and compare how often the book
opens in each rank.

Write the rules as a strategy file, then score it. For a ladder, start from `templates/ladder.py` and
fill in the numbers from `scaling` and `entries`; it places its own resting orders on a 1-minute clock.
Strategies that place their own orders should cancel only orders still in `ctx.open_orders`: an order
can fill in the minute it would be replaced, and cancelling a filled order raises.

```bash
python3 trader.py replicate trader.json candidate.py --start 2025-06-01 --end 2026-07-01 -p ...
```

It reports, over the market hours they held a position, the share where the candidate was on the same
side, the other side, or flat; the market hours the candidate held and they didn't; the correlation of
the two books' daily returns; and the share of their profitable markets where the candidate also made
money. The books run at different sizes, so compare returns, not PnL. Both runs are trials in the ledger.

As a guide: the strategy that made a book scores about 1.0 same side and a high return correlation
against it; an unrelated strategy scores near 0 same side and mostly flat. A book that trades fast (holds
under a few hours) needs `--clock 1m` on `replicate`, and even then same side stays well under 1.0 when
entries land minutes apart; the daily return correlation is the better guide there.

Each revision of the rules is a try. Keep them few and say how many there were. A candidate that copies
their book closely is a description of what they did, not yet a strategy: put it through the gauntlet
like any other, because rules fitted to one trader's year are fitted to that year.

## What to report

1. Their record in the engine (Sharpe, return, drawdown, against BTC), and how it compares with their
   realized PnL.
2. Skill or luck: the mirror's rung, and the stale-null result in a sentence.
3. Their style in three or four facts from the profile.
4. The rules found, with the replication numbers (same side, return correlation) and the candidate's own rung.
5. What the data couldn't show: markets outside the dataset, a hold time shorter than the clock, rules
   that read data we don't have (their other venues, their spot book, off-exchange hedges).
