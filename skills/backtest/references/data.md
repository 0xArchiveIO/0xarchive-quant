# The backtest dataset

Built by `0xbt data pull` from the API (dataset version `api-v0`): one Parquet file per market per
month. Columns the API has no minute form of are null in these files: the book columns and the
liquidation levels below. Context columns (OI, mark, funding) are the API's 1-minute averages rather than
the last snapshot.

Files are `<MARKET>/<YYYY-MM>.parquet`, one row per minute, Hyperliquid core perps. Every value is known at the minute's close (no lookahead); a minute with no trades has null
prices, not a carried value. A strategy reads these as fields on `h` (`h.funding_rate`, `h.cvd_delta`),
resampled to its bars: prices take first/max/min/last, volumes and counts sum, state columns take the
last value.

| Family | Columns | From |
|---|---|---|
| Price and volume | `open`, `high`, `low`, `close`, `volume`, `quote_volume`, `trade_count`, `vwap` | 2025-03-22 |
| Taker flow | `taker_buy_quote`, `taker_sell_quote`, `cvd_delta` | 2025-03-22 |
| Book (null when pulled from the API) | `best_bid`, `best_ask`, `spread_bps`, `spread_bps_mean`, `depth_bid_10bps`, `depth_ask_10bps`, `depth_bid_50bps`, `depth_ask_50bps`, `imbalance_10bps`, `book_snapshots` | 2023-04-15 |
| Market context | `open_interest`, `mark_price`, `oracle_price`, `mid_price`, `impact_bid`, `impact_ask`, `funding_rate` (hourly estimate, a fraction), `premium`, `funding_settled` (the rate paid, on settlement minutes only) | 2023-05-20 |
| Liquidations | `liq_long_usd`, `liq_short_usd`, `liq_long_count`, `liq_short_count` | 2025-12-22 |
| Order activity | `limit_orders_placed`, `orders_canceled`, `orders_filled`, `trigger_orders_placed`, `tpsl_orders_placed`, `orders_triggered`, `orders_force_canceled` | 2026-03-10 |
| Liquidation levels (null when pulled from the API) | `liq_levels_long_1pct_usd`, `liq_levels_short_1pct_usd`, `liq_levels_age_ms` | 2026-07-27 |

Backtests that trade need prices, so the earliest trading window starts 2025-03-22 whatever else the
strategy reads. A strategy reading liquidations can't start before 2025-12-22; one reading order activity,
not before 2026-03-10. Liquidation levels start inside the default holdout, so they can't be tested in a
study yet: say so.

Not every market covers every month: newer listings start when they listed. `oxb.Source(DIR).markets()` lists them. Point-in-time universes need the hourly bars
(`0xbt hourly --data DIR`).

## Not in the dataset

Say "not testable with our data" for an idea that needs any of these, and say which:

- Other venues in a backtest (HIP-3, HIP-4, Spot, Lighter), and anything off Hyperliquid (CEX prices,
  options, on-chain flows, news, social data). The API has HIP-3, HIP-4, Spot and Lighter data for
  research reads (the `0xarchive` skill), but the engine trades Hyperliquid core perps only.
- Individual orders or the full book at a level (only the summaries above). The API's L4 and L2 routes
  have them for research.
- Per-wallet data inside a strategy. A trader's own book comes in through `trader.py` and the mirror
  (traders.md), not as a column.
- Anything faster than a minute. Strategies decide on closed minutes and trade from the next one.
