---
name: 0xarchive-backtest
version: 0.1.0
description: >
  Quant research and backtesting on 0xArchive data, done the way a careful desk does it: hand over a
  wallet, a trading idea, a tweet, a Pine or TS script, a paper, someone else's backtest, or a strategy
  file, and get a verdict with receipts. Reverse engineers a trader's style from their fills and scores how
  much of their book a strategy reproduces; tests whether a trader or a strategy is skilled or lucky.
  Every run goes into a trial ledger, the last months stay sealed until one candidate is committed, and
  nothing is called an edge until it passes costs, nulls, out-of-sample and walk-forward tests and a
  deflated Sharpe over every try. Uses the 0xarchive-strategy engine (0xs) on Hyperliquid perps.
  Use for backtest, "is this edge real", test this strategy, reverse engineer this wallet or trader, port
  this Pine script, audit this backtest, find me a strategy, or paper trade a strategy.
allowed-tools: Bash, Read, Write, Edit
argument-hint: "a wallet, an idea, a script or a strategy file, e.g. 'reverse engineer 0x...', 'does funding predict reversals', 'audit backtest.py'"
metadata: {"openclaw":{"requires":{"env":["OXARCHIVE_API_KEY"]},"primaryEnv":"OXARCHIVE_API_KEY"}}
---

# 0xArchive Backtest

Turn one sentence into a quant study: a testable spec, the cheapest evidence first, a backtest with every
cost in, a gauntlet of checks, and a verdict the user can rerun from its receipt.

The engine is `0xarchive-strategy` (the `0xs` command, imported as `oxarchive_strategy`). It is
deterministic: the same engine, dataset, strategy source and params give the same result hash on every
machine, so a result is a claim anyone can check. Market data questions outside a backtest belong to the
`0xarchive` skill (the API reference); this skill links to it.

## Before the first run

Check once per session, and only what the route needs:

1. **The engine:** `0xs --help`. Missing: `pip install '0xarchive-strategy[curated]'`. If that fails
   because the package is not published yet, tell the user and stop; do not write a substitute backtester.
2. **The data:** a directory of `<MARKET>/<YYYY-MM>.parquet` minute files, usually `data/hyperliquid`.
   Look for it in the working directory, then ask. Never invent or synthesize market data for a verdict.
   If a universe rule is used, `0xs hourly --data DIR` writes the hourly bars it reads.
3. **The API key** (`$OXARCHIVE_API_KEY`) only for the trader route and for API data checks. Never print it.

## Route the input first

Pick the route from what the user handed over, then read only that reference.

| Input | Route | Read |
|---|---|---|
| A wallet address or Lighter account index | Reverse engineer the trader | [references/traders.md](references/traders.md) |
| A one-line idea, a tweet, a thread, a claim | Write it as a spec, then test it | [references/ideas.md](references/ideas.md) |
| A paper or PDF | Extract the rules into a spec, then as an idea | [references/ideas.md](references/ideas.md) |
| A Pine Script or TS indicator | Port it to oxs, then test it | [references/ports.md](references/ports.md) |
| Code from another framework (backtrader, vectorbt, freqtrade, zipline, a notebook) | Audit the original, port it, test it | [references/audit.md](references/audit.md), [references/ports.md](references/ports.md) |
| A strategy file for oxs | `0xs check`, then the gauntlet | [references/gauntlet.md](references/gauntlet.md) |
| "Find me something", an open search | Exploration with a declared budget | [references/exploration.md](references/exploration.md) |

Every route ends in the gauntlet ([references/gauntlet.md](references/gauntlet.md)). Column names, coverage
dates and what the dataset cannot express are in [references/data.md](references/data.md); how to write a
strategy is in [references/strategy-api.md](references/strategy-api.md).

If the user says only "my strategy" or "this wallet" with nothing attached or named in the conversation,
ask which one. Never pick an example on their behalf.

## The rules

These are what make the result worth posting. They hold on every route.

1. **Start a study before the first backtest of a new question:** `0xs study init` in the working
   directory. From then on every backtest, sweep and gauntlet below that directory is written to
   `.0xarchive/trials.jsonl`, and the last three months of data (or `--holdout DATE`) are sealed.
   Never edit, move or delete `.0xarchive/`, and never run backtests from outside the study directory to
   avoid it. One question per study; a new question gets a new directory.
2. **Never read the Parquet files directly** (pandas, pyarrow, duckdb). For event studies and research
   reads, use `oxs.Source(DIR).rows(market, start, end, columns=[...])`, which goes through the seal.
3. **Write the spec before the first run**, in `STUDY.md` beside `.0xarchive/`: the hypothesis in one
   sentence, the universe, clock, signal, sizing, costs and window, and the result that would kill it.
   Do not change the kill condition after seeing results.
4. **Cheap evidence first.** An event study or a single backtest before a grid. Most ideas die here.
5. **Label every number** as measured (it has a result hash, receipt id or API `request_id`), inferred
   (your reasoning from measured numbers), or untested. Never present an inference as a measurement.
6. **The gauntlet decides, not you.** Report its rung in its own words (`loses after costs`,
   `fails nulls`, `in-sample only`, `holds out of sample`, `passes deflated Sharpe`,
   `passes sealed holdout`) and never soften, round up or reword it. Do not call anything an edge, a
   signal that works, or profitable below `holds out of sample`, and say "deflated Sharpe not passed"
   alongside it at that rung.
7. **Every try counts.** Tweaking until something passes is what the ledger is for: each variant is a
   trial, and the deflated Sharpe gets harder with each. Tell the user the trial count with every verdict
   (`0xs study status`). Do not restart a study to reset the count.
8. **The holdout is the user's to open.** `0xs study commit` runs one candidate over the sealed months,
   once, and can't be undone. Propose the candidate, say what it is and why, and commit only on the
   user's go-ahead.
9. **Not testable is an answer.** When the idea needs data the dataset doesn't have (see data.md), say
   so plainly and say what would make it testable. Don't approximate it with a different idea silently.
10. **Point in time.** Universes come from `x.top_n` or `x.rule`, which see only data from before each
    pick, so delisted markets stay in. A hand-picked list of today's winners is survivorship bias; flag it.

## The workflow

1. **Route** the input (above) and read its reference.
2. **Frame:** write `STUDY.md` (rule 3). Check coverage for the columns the idea reads (data.md) and set
   the window inside it, ending at or before the holdout.
3. **Cheap evidence:** an event study or one backtest. Stop here if it is dead, and say so.
4. **Strategy:** write it as an oxs file (`def strategy(param=default, ...)` returning `oxs.Strategy`),
   then `0xs check FILE`. Keep it to the pieces in `x` ([references/strategy-api.md](references/strategy-api.md)).
5. **Gauntlet:** `0xs gauntlet FILE --start ... --end HOLDOUT -g name=v1,v2 --card card.md`. A small grid
   of values the spec already justified, not a search.
6. **Verdict:** report the rung, its reasons, the notes, and the trial count. Show `card.md`.
7. **Holdout**, only at `passes deflated Sharpe` and only with the user's go-ahead (rule 8):
   `0xs study commit FILE -p ...`, then rerun the gauntlet so the card shows the holdout.
8. **Build:** hand over the deliverables below.

For a request with several questions (a trader with three markets, an idea with two variants), keep a
checklist in `STUDY.md`, mark each question answered, partly answered or open, and don't call the study
finished while one is open.

## Finish

Deliver, in this order:

1. The verdict line: rung, trial count, and the one reason that decided it.
2. `card.md` (the gauntlet's one-page report) and the strategy file.
3. The rerun command, exactly: `0xs backtest FILE --start ... --end ... -p ...` with the result hash it
   must reproduce.
4. What is inferred or untested, in one list.
5. For a strategy that passed: how to paper trade it on live rows (`oxs.Session`, strategy-api.md
   "Paper trading"). Never place real orders or handle keys; live trading runs on the user's own runner.
