# 0xarchive-backtest engine

The backtester the `0xarchive-backtest` skill runs: Rust crates for fills, portfolios, funding,
liquidation, bars, calendars and deterministic math, with a Python strategy API on top.

```bash
pip install "crates/py[curated]"     # builds the Rust extension; needs cargo
0xbt --help
```

| Crate | What it is |
|---|---|
| `crates/schema` | The 1-minute row and its columns |
| `crates/math` | Deterministic exp, log, pow, sin and the rest, the same bits on every platform |
| `crates/engine` | Fills (`bar-v2`), portfolios, exits, bars and history reads, clocks and calendars, point-in-time universes, stats, receipts |
| `crates/dataset` | Parquet read/write by byte range |
| `crates/build-api` | Builds the minute dataset from the 0xArchive API (`0xbt data pull`) |
| `crates/py` | The Python package `oxarchive_backtest` and the `0xbt` command; studies, the gauntlet and the CLI live in `python/oxarchive_backtest/` |

`examples/` has one runnable strategy per kind of piece. Tests: `cargo test` for the crates, and
`python -m pytest crates/py/tests` after installing (tests that need data look for `data/hyperliquid`
here and skip without it). The 7-day momentum example must keep result hash `f40f4c5fc66c0053`
(1,110 fills) over the full 62-market dataset, 2025-03-01 to 2026-10-01, cash 100,000.
