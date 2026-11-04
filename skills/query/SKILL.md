---
name: 0xarchive
version: 1.17.0
description: >
  Query historical and real-time crypto market data from 0xArchive across two venues: Hyperliquid and Lighter.
  Lighter has two deployments: mainnet at /v1/lighter and Robinhood Chain at /v1/rh-lighter (USDG-quoted, e.g. BTC, AAPL-USDG).
  Hyperliquid includes HIP-3 builder perps (/v1/hyperliquid/hip3), HIP-4 outcome markets (/v1/hyperliquid/hip4), and Spot (/v1/hyperliquid/spot, e.g. HYPE-USDC).
  Covers orderbooks, trades, candles, funding, open interest, liquidations, account positions, outcome markets, TWAP, CVD, breadth, trade intent, USDC flows, positioning, webhooks, and data quality; data types are route-specific.
  WebSocket support is channel-specific and GET /v1/capabilities lists it per venue and data type: Lighter orderbook, trades, open interest, and funding stream live on both deployments; Lighter candles and L3 are replay-only; L4 channels replay on Hyperliquid core, HIP-3, HIP-4, and Spot.
  Use in Claude Code, Codex, and SKILL.md-compatible agents for market data, orderbooks, trades, candles, funding, prices, positions, real-time streams, prediction markets, or spot pairs on Hyperliquid, HIP-3, HIP-4, Hyperliquid Spot, Lighter, or Lighter on Robinhood Chain.
allowed-tools: Bash
argument-hint: "query, e.g. 'BTC funding rate', 'AAPL-USDG orderbook on Robinhood Chain', or 'positions for wallet 0x...'"
metadata: {"openclaw":{"requires":{"env":["OXARCHIVE_API_KEY"]},"primaryEnv":"OXARCHIVE_API_KEY"}}
---

# 0xArchive API Skill

Query historical and real-time crypto market data from **0xArchive** using `curl`. 0xArchive covers two venues: **Hyperliquid** and **Lighter**. **HIP-3** builder perps live under the Hyperliquid namespace at `/v1/hyperliquid/hip3`. **HIP-4** outcome markets (binary prediction markets) live at `/v1/hyperliquid/hip4`. **Hyperliquid Spot** lives at `/v1/hyperliquid/spot`. Lighter has two deployments: mainnet at `/v1/lighter` and **Robinhood Chain** at `/v1/rh-lighter`. Robinhood Chain is a second deployment of Lighter, not a third venue. Data types are route-specific: orderbooks, trades, candles, funding rates, open interest, liquidations, account positions, outcome markets, spot, TWAP, and data quality metrics.

Orderbook depth limits apply to L2 snapshot endpoints only.

## Authentication and API Version

Market-data endpoints require the `x-api-key` header. The key is read from `$OXARCHIVE_API_KEY`. Also send `0xArchive-Version: 2026-10-01` on every request; every example in this skill does.

```bash
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" "https://api.0xarchive.io/v1/..."
```

The version header selects the current response contract, and responses echo it in their own `0xArchive-Version` header. Requests without it keep the earlier shapes, which remain supported. With it:

- Error bodies use the current `error_code` names (see [Error Handling](#error-handling)).
- Time fields that are otherwise integer milliseconds become RFC 3339 strings with an integer companion ending `_ms`: resting-order `timestamp` in L4 snapshots (adds `timestamp_ms`), CVD buckets, HIP-3 oracle prices, Lighter and Robinhood Chain liquidations and liquidation volume, and levels and trigger-levels `snapshot_ts` (adds `snapshot_ts_ms`).
- Data quality routes and `/v1/symbols` return the standard `{success, data, meta}` envelope.

On WebSocket, add `version=2026-10-01` to the connection URL: `wss://api.0xarchive.io/ws?apiKey=KEY&version=2026-10-01`. The `subscribed` and `replay_started` acks echo the version, and Lighter and Robinhood Chain replay rows use the same shapes as their live messages.

## Capabilities

`GET /v1/capabilities` lists what each venue offers. It is public (no API key) and costs no credits. It returns one row per venue and data type:

| Field | Meaning |
|-------|---------|
| `venue` | `hyperliquid`, `hip3`, `hip4`, `spot`, `lighter`, or `rh-lighter` |
| `datatype` | For example `trades`, `l2_orderbook`, `l2_full_depth`, `l4_diffs`, `l4_orders`, `candles`, `funding`, `oi` |
| `rest_routes` | REST routes that serve it |
| `ws_channels` | WebSocket channels that carry it |
| `live`, `replay` | Whether its channels stream live, and whether they replay history |
| `available_from` | RFC 3339 start of served history, or `null` when not tracked |
| `cadence` | `event`, `snapshot`, `sample`, `interval`, or `reference` |
| `page_limit` | Maximum `limit` for the route family |
| `intervals` | Accepted `interval` values |
| `notes` | Caveats, such as policy floors and bulk single-channel replay |

Check it before calling a route or channel you have not used:

```bash
curl -s "https://api.0xarchive.io/v1/capabilities" \
  | jq '.data[] | select(.venue == "hip3") | {datatype, live, replay, available_from, page_limit}'
```

A venue that does not offer a data type answers 404 `unsupported_for_venue`, naming the venues and routes that do. Per-symbol coverage comes from `GET /v1/symbols` and `GET /v1/data-quality/coverage/{exchange}/{symbol}`.

## Venue Scopes & Coin Naming

| Scope | Path prefix | Coin format | Examples |
|----------|-------------|-------------|---------|
| Hyperliquid | `/v1/hyperliquid` | UPPERCASE | `BTC`, `ETH`, `SOL` |
| Hyperliquid HIP-3 | `/v1/hyperliquid/hip3` | Case-sensitive, `builder:NAME` | `xyz:SP500`, `xyz:GOLD`, `hyna:BTC`, `vntl:SPACEX`, `flx:TSLA`, `cash:NVDA` |
| Hyperliquid HIP-4 | `/v1/hyperliquid/hip4` | Bare numeric `<10*outcome_id + side>` (legacy `#0` / `%230` also accepted) | `0`, `1`, `10`, `11` |
| Hyperliquid Spot | `/v1/hyperliquid/spot` | Dashed canonical `BASE-QUOTE` | `HYPE-USDC`, `PURR-USDC`, `AAPL-USDC` |
| Lighter (mainnet) | `/v1/lighter` | UPPERCASE | `BTC`, `ETH` |
| Lighter on Robinhood Chain | `/v1/rh-lighter` | UPPERCASE perps; dashed `BASE-USDG` spot | `BTC`, `ETH`, `AAPL-USDG` |

Hyperliquid and both Lighter deployments auto-uppercase the symbol server-side on market-data routes. Lighter and Robinhood Chain account positions routes (`/positions/{symbol}`, `/positions/{symbol}/summary`, and the `symbol` filter on account routes) match the symbol exactly, so pass it uppercase as `GET /instruments` lists it (`BTC`, not `btc`). HIP-3 coin names are passed through as-is. HIP-4 coins encode outcome and side: `0` is outcome 0 / side 0 (YES), `1` is outcome 0 / side 1 (NO), `10` is outcome 1 / side 0, etc. The bare numeric form is canonical; the legacy `#0` and `%230` forms still work for backward compatibility. Spot symbols are dashed (`HYPE-USDC`, `PURR-USDC`, `AAPL-USDC`); the server resolves the dashed form to the wire format (`PURR/USDC`, `@107`) internally, so always use the dashed form.

Lighter on Robinhood Chain markets and account indices are separate from Lighter mainnet even when the symbol text matches: `BTC` under `/v1/rh-lighter` is a different market from `BTC` under `/v1/lighter`. Keep the namespace with every result.

The unprefixed legacy routes (`/v1/trades/{symbol}`, `/v1/orderbook/{symbol}`, `/v1/funding/{symbol}`, `/v1/openinterest/{symbol}`, `/v1/instruments`) are deprecated: they answer with a `Deprecation: true` header and a `Link` to the successor. Use the `/v1/hyperliquid` routes instead.

## Timestamps

Request time parameters (`start`, `end`, `timestamp`, and `at`) accept **Unix milliseconds** or **RFC 3339** strings such as `2026-09-01T00:00:00Z` (use `Z`, or URL-encode a `+hh:mm` offset). An unparsable time returns 400 `invalid_time_range`. Response records use RFC 3339 UTC strings with millisecond precision; with the version header, integer milliseconds appear only in fields ending `_ms`. Use these shell helpers for request windows:

```bash
NOW=$(( $(date +%s) * 1000 ))
HOUR_AGO=$(( NOW - 3600000 ))
DAY_AGO=$(( NOW - 86400000 ))
WEEK_AGO=$(( NOW - 604800000 ))
```

## Response Format

Every response follows this shape:

```json
{
  "success": true,
  "data": [ ... ],
  "meta": {
    "count": 100,
    "request_id": "uuid",
    "has_more": true,                 // paged routes: keep paging while true
    "next_cursor": "1706000000000",   // present exactly when has_more is true; pass back unchanged
    "symbol": "BTC",                  // per-symbol routes: canonical public symbol
    "venue": "hyperliquid"            // hyperliquid, hip3, hip4, spot, lighter, or rh-lighter
  }
}
```

Coverage boundaries are explicit. A range that starts before a dataset's first row and ends after it is served from that row. A per-symbol range that ends before the symbol's coverage returns 200 with empty `data` plus `meta.coverage_from` and `meta.notice`. A range that ends before a venue-wide policy floor (for example Hyperliquid candles before 2025-03-22 12:00 UTC, or Lighter liquidations before 2026-06-10) returns 400 `range_before_coverage`. Treat an empty page with `coverage_from` as "outside coverage", not as "nothing happened".

Some routes add optional `meta` fields. Lighter trades (both deployments) carry `finalized_through`, plus `requested_end` and `clamped_to` when `end` was clamped, and `/recent` carries `preliminary_row_count`. Account positions routes add `as_of`, `snapshot_ts`, `source`, `quality`, `stale`, `built_through`, `finalized_through`, and `totals`; see [Account Positions](#account-positions). Positions reads and ranges before coverage return 200 with an empty list, not an error; the as-of position read, the change log, and Lighter hourly history add `notice` and `coverage_from`.

## Endpoint Reference

### Hyperliquid (`/v1/hyperliquid`)

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /instruments` | -- | List all instruments |
| `GET /instruments/{symbol}` | -- | Single instrument details |
| `GET /orderbook/{symbol}` | `timestamp`, `depth` | Latest or at timestamp |
| `GET /orderbook/{symbol}/history` | `start`, `end`, `limit`, `cursor`, `depth` | Historical snapshots |
| `GET /trades/{symbol}` | `start`, `end`, `limit`, `cursor`, `side` | Trade history; `side=buy` or `side=sell` filters trades |
| `GET /candles/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OHLCV candles |
| `GET /funding/{symbol}/current` | -- | Current funding rate |
| `GET /funding/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Funding rate history |
| `GET /openinterest/{symbol}/current` | -- | Current open interest |
| `GET /openinterest/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OI history |
| `GET /liquidations/{symbol}` | `start`, `end`, `limit`, `cursor` | Liquidation events |
| `GET /liquidations/{symbol}/volume` | `start`, `end`, `limit`, `cursor`, `interval` | Aggregated liquidation volume (USD) |
| `GET /liquidations/{symbol}/levels` | `range_pct`, `buckets`, `side`, `at` | Projected forced-liquidation price levels (refreshed about every five minutes) |
| `GET /liquidations/{symbol}/levels/history` | `start`, `end`, `limit`, `cursor`, `summary`, `range_pct`, `buckets`, `side` | Liquidation levels history |
| `GET /orders/{symbol}/trigger-levels` | `range_pct`, `buckets`, `side` | Resting TP/SL trigger price levels |
| `GET /orders/{symbol}/trigger-levels/history` | `start`, `end`, `limit`, `cursor`, `summary`, `range_pct`, `buckets`, `side` | Trigger levels history |
| `GET /cvd/{symbol}` | `start`, `end`, `interval`, `limit`, `cursor` | Cumulative volume delta; pass `meta.next_cursor` back to page |
| `GET /breadth/above-vwap/current` | -- | Percent of eligible instruments above their UTC-session VWAP |
| `GET /breadth/above-vwap` | `start`, `end`, `interval`, `limit`, `cursor` | Breadth history |
| `GET /breadth/above-ma/current` | `n` (required: `5`, `20`, `50`, `200`) | Newest reading of the share of perps above their n-day moving average. Build plan and above |
| `GET /breadth/above-ma` | `n` (required), `start`, `end`, `interval`, `limit`, `cursor` | Moving-average breadth history; each `n` starts on its own date. Build plan and above |
| `GET /trade-intent/{symbol}` | `start`, `end`, `interval`, `limit`, `cursor` | Why the market traded, per bucket: volume of new longs and shorts, profit taken, stops and flips, market-maker volume, and the winners' and losers' aggregate position and flow. Build plan and above |
| `GET /positioning` | `window` (required: `1h`, `4h`, `24h`) | Every listed perp's open interest change, taker flow, liquidations and typical moves over a trailing window; current window only. Build plan and above |
| `GET /usdc-flows` | `start`, `end`, `min_usd`, `kind`, `limit`, `cursor` | Every USDC deposit, withdrawal and HyperEVM transfer, with the HyperCore account; join pages on `id`. Build plan and above |
| `GET /usdc-flows/volume` | `start`, `end`, `interval`, `limit`, `cursor` | USDC flow totals per bucket, zeros included. Build plan and above |
| `GET /wallets/classify` | `min_orders`, `min_volume_usd`, `uses_twap`, `uses_priority_gas`, `min_cancel_rate`, `max_cancel_rate`, `date`, `sort`, `order`, `limit`, `offset` | Classify wallets by trading behaviour |
| `GET /liquidations/user/{address}` | `start`, `end`, `limit`, `cursor`, `coin` | Liquidations for a user |
| `GET /freshness/{symbol}` | -- | Data freshness per data type |
| `GET /summary/{symbol}` | -- | Combined market summary (price, funding, OI, volume, liquidations) |
| `GET /prices/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Mark/oracle/mid price history |
| `GET /orders/{symbol}/history` | `start`, `end`, `user`, `status`, `order_type`, `triggered`, `limit`, `cursor` | Order history with user attribution; `triggered=true` keeps only `triggered` status events, `false` every other status |
| `GET /orders/{symbol}/flow` | `start`, `end`, `interval`, `limit`, `cursor` | Order flow aggregation; `cursor` resumes after a bucket |
| `GET /orders/{symbol}/tpsl` | `start`, `end`, `user`, `triggered`, `limit`, `cursor` | TP/SL order history |
| `GET /orderbook/{symbol}/l4` | `timestamp`, `depth` | L4 orderbook reconstruction |
| `GET /orderbook/{symbol}/l4/diffs` | `start`, `end`, `limit`, `cursor` | L4 orderbook diffs |
| `GET /orderbook/{symbol}/l4/history` | `start`, `end`, `limit`, `cursor` | L4 orderbook checkpoints |
| `GET /orderbook/{symbol}/l2` | `timestamp`, `depth` | L2 full-depth orderbook derived from L4 |
| `GET /orderbook/{symbol}/l2/history` | `start`, `end`, `limit`, `cursor`, `depth` | L2 full-depth checkpoints |
| `GET /orderbook/{symbol}/l2/diffs` | `start`, `end`, `limit`, `cursor` | L2 tick-level diffs |

### HIP-3 (`/v1/hyperliquid/hip3`)

Coin names are **case-sensitive** and carry the builder prefix (e.g., `xyz:SP500`, `xyz:XYZ100`). Builders list and delist markets over time; call `GET /instruments` for the current set. Trades and oracle prices begin 2025-10-13; candles and liquidations 2025-12-22; native L2, funding, and OI 2026-02-16; L4 diffs and order-lifecycle rows 2026-03-10; reconstructable checkpoints and point-in-time state can begin later by symbol. A range that starts before a dataset's first row is served from that row. All HIP-3 symbols are available on every tier.

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /instruments` | -- | List HIP-3 instruments |
| `GET /instruments/{coin}` | -- | Single instrument |
| `GET /orderbook/{coin}` | `timestamp`, `depth` | All HIP-3 symbols, every tier. |
| `GET /orderbook/{coin}/history` | `start`, `end`, `limit`, `cursor`, `depth` | All HIP-3 symbols, every tier. |
| `GET /trades/{coin}` | `start`, `end`, `limit`, `cursor`, `side` | Trade history; `side=buy` or `side=sell` filters trades |
| `GET /trades/{coin}/recent` | `limit`, `side` | Recent trades (no time range needed) |
| `GET /candles/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | OHLCV candles |
| `GET /funding/{coin}/current` | -- | Current funding rate |
| `GET /funding/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | Funding history |
| `GET /openinterest/{coin}/current` | -- | Current OI |
| `GET /openinterest/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | OI history |
| `GET /liquidations/{coin}` | `start`, `end`, `limit`, `cursor` | Liquidation events |
| `GET /liquidations/{coin}/volume` | `start`, `end`, `limit`, `cursor`, `interval` | Aggregated liquidation volume (USD) |
| `GET /liquidations/{coin}/levels` | `range_pct`, `buckets`, `side`, `at` | Projected forced-liquidation price levels |
| `GET /liquidations/{coin}/levels/history` | `start`, `end`, `limit`, `cursor`, `summary`, `range_pct`, `buckets`, `side` | Liquidation levels history |
| `GET /orders/{coin}/trigger-levels` | `range_pct`, `buckets`, `side` | Resting TP/SL trigger price levels |
| `GET /orders/{coin}/trigger-levels/history` | `start`, `end`, `limit`, `cursor`, `summary`, `range_pct`, `buckets`, `side` | Trigger levels history |
| `GET /cvd/{coin}` | `start`, `end`, `interval`, `limit`, `cursor` | Cumulative volume delta |
| `GET /oracle/external-price/{coin}` | -- | Latest external (oracle source) price |
| `GET /oracle/discovery-bounds/{coin}` | -- | Current oracle price discovery bounds |
| `GET /breadth/above-vwap/current` | -- | Percent of eligible HIP-3 instruments above session VWAP |
| `GET /breadth/above-vwap` | `start`, `end`, `interval`, `limit`, `cursor` | HIP-3 breadth history |
| `GET /breadth/above-ma/current` | `n` (required: `5`, `20`, `50`, `200`) | Newest share of HIP-3 perps above their n-day moving average. Build plan and above |
| `GET /breadth/above-ma` | `n` (required), `start`, `end`, `interval`, `limit`, `cursor` | HIP-3 moving-average breadth history. Build plan and above |
| `GET /trade-intent/{coin}` | `start`, `end`, `interval`, `limit`, `cursor` | Trade intent for one HIP-3 market. Build plan and above |
| `GET /positioning` | `window` (required: `1h`, `4h`, `24h`) | Every active HIP-3 perp over a trailing window; current window only. Build plan and above |
| `GET /wallets/classify` | same as Hyperliquid | Classify HIP-3 wallets by trading behaviour |
| `GET /freshness/{coin}` | -- | Data freshness per data type |
| `GET /summary/{coin}` | -- | Combined market summary (price, funding, OI) |
| `GET /prices/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | Mark/oracle/mid price history |
| `GET /orders/{coin}/history` | `start`, `end`, `user`, `status`, `order_type`, `triggered`, `limit`, `cursor` | Order history with user attribution; `triggered=true` keeps only `triggered` status events, `false` every other status |
| `GET /orders/{coin}/flow` | `start`, `end`, `interval`, `limit`, `cursor` | Order flow aggregation; `cursor` resumes after a bucket |
| `GET /orders/{coin}/tpsl` | `start`, `end`, `user`, `triggered`, `limit`, `cursor` | TP/SL order history |
| `GET /orderbook/{coin}/l4` | `timestamp`, `depth` | L4 orderbook reconstruction |
| `GET /orderbook/{coin}/l4/diffs` | `start`, `end`, `limit`, `cursor` | L4 orderbook diffs |
| `GET /orderbook/{coin}/l4/history` | `start`, `end`, `limit`, `cursor` | L4 orderbook checkpoints |
| `GET /orderbook/{coin}/l2` | `timestamp`, `depth` | L2 full-depth orderbook derived from L4 |
| `GET /orderbook/{coin}/l2/history` | `start`, `end`, `limit`, `cursor`, `depth` | L2 full-depth checkpoints |
| `GET /orderbook/{coin}/l2/diffs` | `start`, `end`, `limit`, `cursor` | L2 tick-level diffs |

### Build-and-above datasets

Trade intent, USDC flows, breadth above the moving average, and positioning come with the Build, Pro, Scale and Enterprise plans. A Free key gets 403 `forbidden` with `datatype`, `required_plans` and `upgrade_url`, and is not charged. USDC flows bill 1 credit per 1,000 rows; the other three 1 credit per request. None has a WebSocket channel.

- **Trade intent** (`/trade-intent/{symbol}` on Hyperliquid and HIP-3): `interval` is `1s`, `5s`, `15s`, `1m`, `5m`, `15m`, `1h` (default), `4h` or `1d`; `limit` up to 2,000 buckets. Hourly intervals start 2025-05-25 15:00 UTC (HIP-3 2025-10-13 12:00 UTC), minute intervals 2026-07-14, second intervals cover the last 3 days. Both sides of every trade count; market makers are in `mm_usd`. Winners and losers are aggregates over the trailing 72 hours, never wallets. A market idle for more than 7 days has no buckets: a gap, not zero. Set `start` and `end` when paging and stop when `next_cursor` is absent, even on an empty page. Buckets with `is_final: false` can still change.
- **USDC flows** (`/usdc-flows`, `/usdc-flows/volume`, Hyperliquid only): from 2025-09-27 09:33 UTC. `kind` is `deposit`, `withdraw`, `evm_in` or `evm_out`. A page reads at most 31 days, so with a high `min_usd` it can be empty and still have `has_more: true`: stop on the cursor, not on a short page. Events in the last 3 hours can still be added; re-read that span and deduplicate on `id`. `/volume` refuses `min_usd` and `kind`.
- **Breadth above the moving average** (`/breadth/above-ma` and `/current` on Hyperliquid and HIP-3): `n` is required. Each bucket carries its last reading; never average `value_pct`. `coverage_ratio` depends on `n` and is not comparable with above-VWAP breadth. Starts: Hyperliquid n=5 2025-03-27, n=20 2025-04-11, n=50 2025-05-11, n=200 2025-10-08; HIP-3 2025-12-27, 2026-01-11, 2026-02-10, 2026-07-10.
- **Positioning** (`/positioning` on Hyperliquid and HIP-3): `window` is required, and any other parameter is refused; there is no history. One record per market, sorted by symbol; `meta.as_of` is the 5-minute close the records describe. `oi_*` and `mark_*` are null when a bucket has no reading, never 0; `typical_*` are null under 288 counted windows.

### HIP-4 (`/v1/hyperliquid/hip4`)

Outcome markets are binary prediction markets (e.g. "Will BTC be >= $X by date Y?"). Coin names are bare numerics `<10*outcome_id + side>` (e.g. `0`, `1`, `10`); the legacy `#0` / `%230` forms are still accepted. HIP-4 has **candles and outcome-side open interest from May 2, 2026**, with raw OI updates at roughly 10 seconds. HIP-4 has **no funding rates and no liquidations**. The `mark_price` field on HIP-4 instruments and prices is an **implied probability in the range 0..1**, not a USD price.

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /outcomes` | -- | List all outcome markets (HIP-4 only; not present on other venues) |
| `GET /outcomes/{outcome_id}` | -- | Single outcome market detail (HIP-4 only) |
| `GET /questions` | `limit`, `cursor` | Multi-outcome questions, each grouping several binary outcomes |
| `GET /questions/{question_id}` | -- | Single question |
| `GET /instruments` | -- | List HIP-4 instruments (one per side per outcome) |
| `GET /instruments/{coin}` | -- | Single instrument. Coin is the bare numeric (e.g. `0`); legacy `%230` also accepted. |
| `GET /orderbook/{coin}` | `timestamp`, `depth` | Latest or at timestamp |
| `GET /orderbook/{coin}/history` | `start`, `end`, `limit`, `cursor`, `depth` | Historical snapshots |
| `GET /trades/{coin}` | `start`, `end`, `limit`, `cursor`, `side` | Trade history; `side=buy` or `side=sell` filters trades |
| `GET /trades/{coin}/recent` | `limit`, `side` | Recent trades |
| `GET /candles/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | Implied-probability OHLCV candles |
| `GET /openinterest/{coin}/current` | -- | Current open interest |
| `GET /openinterest/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | Outcome-side OI history; raw rows roughly every 10 seconds |
| `GET /freshness/{coin}` | -- | Data freshness per data type |
| `GET /summary/{coin}` | -- | Combined market summary (implied probability + OI; no funding) |
| `GET /prices/{coin}` | `start`, `end`, `limit`, `cursor`, `interval` | Implied-probability history (mark/oracle/mid in 0..1) |
| `GET /orders/{coin}/history` | `start`, `end`, `user`, `status`, `order_type`, `triggered`, `limit`, `cursor` | Order history; `triggered=true` keeps only `triggered` status events, `false` every other status |
| `GET /orders/{coin}/flow` | `start`, `end`, `interval`, `limit`, `cursor` | Order flow aggregation; `cursor` resumes after a bucket |
| `GET /orders/{coin}/tpsl` | `start`, `end`, `user`, `triggered`, `limit`, `cursor` | TP/SL order history |
| `GET /orderbook/{coin}/l4` | `timestamp`, `depth` | L4 orderbook reconstruction |
| `GET /orderbook/{coin}/l4/diffs` | `start`, `end`, `limit`, `cursor` | L4 orderbook diffs |
| `GET /orderbook/{coin}/l4/history` | `start`, `end`, `limit`, `cursor` | L4 orderbook checkpoints |

HIP-4 has no full-depth L2 routes (`/orderbook/{coin}/l2*`); they answer 404 `unsupported_for_venue`. Use the native L2 or L4 routes above.

### Hyperliquid Spot (`/v1/hyperliquid/spot`)

Spot pairs include HYPE-USDC, PURR-USDC and AAPL-USDC; call `GET /pairs` for the current set. Symbols are **dashed canonical** (`BASE-QUOTE`); the server resolves to the wire format (`PURR/USDC`, `@107`) internally. Spot has **no funding rates, open interest, or liquidations**. Spot candles are served from 2025-03-22T10:50:00Z through a dedicated OHLCV route. Use spot for pair discovery, candles, current and historical L2 orderbooks, fills, L4 reconstruction, order lifecycle, and TWAP execution status.

Coverage:
- **Candles**: served from 2025-03-22T10:50:00Z at `1m`, `5m`, `15m`, `30m`, `1h`, `4h`, `1d`, and `1w`; maximum `limit` is 10,000 and `next_cursor` is opaque.
- **Trades**: backfilled from 2025-03-22 10:50:22 UTC. Spot fills before that are not available (no public archive existed).
- **Native L2 and TWAP**: live-forward from 2026-05-05 (L2 from 19:56 UTC, TWAP from 13:05 UTC). No native L2 history is claimed before that date.
- **L4**: raw diffs and order history are served from 2026-03-10. Checkpoints and point-in-time reconstruction start per pair: PURR-USDC from 2026-03-11 01:03 UTC, other pairs from 2026-05-05 22:57 UTC or later (HYPE-USDC from 2026-05-13 15:33 UTC).

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /pairs` | -- | List current Spot pairs |
| `GET /pairs/{symbol}` | -- | Single pair detail (e.g. `HYPE-USDC`) |
| `GET /candles/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OHLCV candles from 2025-03-22T10:50:00Z; intervals `1m` through `1w`; max `limit` 10,000; cursor is opaque |
| `GET /orderbook/{symbol}` | `timestamp`, `depth` | Current L2 orderbook (live from 2026-05-05) |
| `GET /orderbook/{symbol}/history` | `start`, `end`, `limit`, `cursor`, `depth` | L2 history window (from 2026-05-05) |
| `GET /orderbook/{symbol}/l4` | `timestamp`, `depth` | Point-in-time L4 reconstruction; starts per pair (PURR-USDC from 2026-03-11 01:03 UTC, other pairs from 2026-05-05 22:57 UTC or later) |
| `GET /orderbook/{symbol}/l4/diffs` | `start`, `end`, `limit`, `cursor` | Raw L4 diffs |
| `GET /orderbook/{symbol}/l4/history` | `start`, `end`, `limit`, `cursor` | L4 checkpoints |
| `GET /trades/{symbol}` | `start`, `end`, `limit`, `cursor`, `user`, `side` | Spot trades. Pass `user` to filter to a wallet's fills. Backfilled to 2025-03-22. |
| `GET /trades/{symbol}/recent` | `limit`, `side` | Recent spot trades (no time range needed) |
| `GET /orders/{symbol}/history` | `start`, `end`, `user`, `status`, `order_type`, `limit`, `cursor` | Spot order lifecycle |
| `GET /twap/{symbol}` | `start`, `end`, `limit`, `cursor` | TWAP execution statuses for a symbol |
| `GET /twap/user/{user}` | `start`, `end`, `limit`, `cursor` | TWAP execution statuses for a wallet |
| `GET /freshness/{symbol}` | -- | Data freshness per data type |

### Lighter mainnet (`/v1/lighter`)

Lighter has native L2, L3, trades, candles, funding, open interest, liquidation events and volume, freshness, summary, and price history. Candles begin August 1, 2025. Funding and OI begin August 25, 2025 and update roughly every 10 seconds. Per-fill Lighter trade rows begin January 17, 2025 and carry maker/taker context; exact starts vary by market. Native L2 begins January 29, 2026. L3 begins March 5, 2026 and is capped at 250 resting orders per side. Liquidation events and volume are served from 2026-06-10, the start of live capture; there is no earlier public backfill, and a range that ends before that date returns 400 `range_before_coverage`.

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /instruments` | -- | List Lighter instruments |
| `GET /instruments/{symbol}` | -- | Single instrument |
| `GET /orderbook/{symbol}` | `timestamp`, `depth` | Latest or at timestamp |
| `GET /orderbook/{symbol}/history` | `start`, `end`, `limit`, `cursor`, `depth`, `granularity` | Default granularity: `checkpoint` |
| `GET /trades/{symbol}` | `start`, `end`, `limit`, `cursor`, `side` | Per-fill history with maker/taker context. Per-fill Lighter trade rows begin January 17, 2025; exact starts vary by market. Returns reconciled trades only; `end` is clamped to `meta.finalized_through` |
| `GET /trades/{symbol}/recent` | `limit`, `side` | Recent trades (no time range needed), including preliminary trades not yet reconciled |
| `GET /candles/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OHLCV candles from August 1, 2025 |
| `GET /funding/{symbol}/current` | -- | Current funding rate |
| `GET /funding/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Funding history from August 25, 2025; raw updates roughly every 10 seconds |
| `GET /openinterest/{symbol}/current` | -- | Current OI |
| `GET /openinterest/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OI history from August 25, 2025; raw updates roughly every 10 seconds |
| `GET /liquidations/{symbol}` | `start`, `end`, `limit`, `cursor` | Liquidation events from 2026-06-10 (live capture; no earlier public backfill) |
| `GET /liquidations/{symbol}/volume` | `start`, `end`, `limit`, `cursor`, `interval` | Time-bucketed liquidation volume |
| `GET /freshness/{symbol}` | -- | Data freshness per data type |
| `GET /summary/{symbol}` | -- | Combined market summary (price, funding, OI) |
| `GET /prices/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Mark/oracle price history |
| `GET /l3orderbook/{symbol}` | `timestamp`, `depth`, `account` | L3 order-level snapshot; up to 250 orders per side |
| `GET /l3orderbook/{symbol}/history` | `start`, `end`, `limit`, `cursor`, `granularity`, `account` | Tick-level L3 snapshots from March 5, 2026; up to 250 orders per side |

Account positions for Lighter mainnet live under the same prefix (`/accounts/{account_index}/positions`, `/positions/{symbol}`, and the L1 lookup `/accounts?l1_address=`); see [Account Positions](#account-positions).

### Lighter on Robinhood Chain (`/v1/rh-lighter`)

Robinhood Chain is Lighter's second deployment, not a third venue. `/v1/rh-lighter` mirrors `/v1/lighter` with the same parameters and response shapes, except the two `l3orderbook` routes: this deployment has no L3 capture, so they return 404 with a message pointing to `/v1/rh-lighter/orderbook/{symbol}`. Markets are quoted in USDG and include perps and spot. Perp symbols are uppercase (`BTC`, `ETH`); spot symbols are the base and quote joined by a dash (`AAPL-USDG`). Funding, open interest, and liquidations exist for perps only. Start with `GET /instruments` to discover symbols.

Coverage (UTC):
- **Trades and liquidations**: from 2026-06-26 20:10:26, the deployment's first trade. A range that starts earlier and ends later is served from that point; a range that ends before it returns an empty page with `meta.coverage_from` and `meta.notice`. The first liquidation is at 2026-06-27 23:14, so a liquidations range before it returns an empty page, not an error.
- **Order book, open interest, and funding**: from 2026-08-22 18:43. Earlier history of these streams is not recoverable.
- **Candles**: from 2026-06-26 20:10 UTC, at `1m` through `1w`.
- **L3**: not captured. Use L2.

Trades are two-tier, exactly like Lighter mainnet. `GET /trades/{symbol}` returns finalized rows (`source: "bucket"`) and clamps `end` to `meta.finalized_through`, which trails the present by about a day; when it clamps, the response adds `meta.requested_end` and `meta.clamped_to`. `GET /trades/{symbol}/recent` includes newer preliminary rows (`source: "ws"`), counted by `meta.preliminary_row_count`. Prices and amounts are in USDG, and account fields hold Robinhood Chain account indices, unrelated to mainnet indices with the same number. Liquidation rows from before live capture (2026-06-27 to 2026-08-22) were backfilled from the venue's finalized export: they carry `source: "bucket"` and an empty `raw_json`. Live-captured rows carry `source: "ws"` and the venue's raw JSON in `raw_json`.

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /instruments` | -- | List markets (perps and USDG-quoted spot) |
| `GET /instruments/{symbol}` | -- | Single market |
| `GET /orderbook/{symbol}` | `timestamp`, `depth` | Latest or at timestamp, from 2026-08-22 18:43 UTC |
| `GET /orderbook/{symbol}/history` | `start`, `end`, `limit`, `cursor`, `depth`, `granularity` | Default granularity: `checkpoint` |
| `GET /trades/{symbol}` | `start`, `end`, `limit`, `cursor`, `side` | Finalized per-fill history from 2026-06-26 20:10:26 UTC; `end` is clamped to `meta.finalized_through` |
| `GET /trades/{symbol}/recent` | `limit`, `side` | Recent trades, including preliminary rows (`meta.preliminary_row_count`) |
| `GET /candles/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OHLCV candles from 2026-06-26 20:10 UTC |
| `GET /funding/{symbol}/current` | -- | Current funding rate (perps) |
| `GET /funding/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Funding history (perps) from 2026-08-22 18:43 UTC |
| `GET /openinterest/{symbol}/current` | -- | Current OI (perps) |
| `GET /openinterest/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | OI history (perps) from 2026-08-22 18:43 UTC |
| `GET /liquidations/{symbol}` | `start`, `end`, `limit`, `cursor` | Liquidation events (perps) from 2026-06-26 20:10:26 UTC; rows before 2026-08-22 carry `source: "bucket"` and an empty `raw_json` |
| `GET /liquidations/{symbol}/volume` | `start`, `end`, `limit`, `cursor`, `interval` | Time-bucketed liquidation volume from 2026-06-26 20:10:26 UTC |
| `GET /freshness/{symbol}` | -- | Data freshness per data type |
| `GET /summary/{symbol}` | -- | Combined market summary |
| `GET /prices/{symbol}` | `start`, `end`, `limit`, `cursor`, `interval` | Mark/oracle price history |

Account positions for this deployment use `/v1/rh-lighter/accounts/{account_index}/positions` and the market routes under `/v1/rh-lighter/positions`; there is no L1 address lookup on Robinhood Chain. See [Account Positions](#account-positions). Check symbol-level windows with `GET /v1/data-quality/coverage/rh-lighter/{symbol}` before a long pull.

### Account Positions

Open perpetual positions of public venue accounts: what an address or Lighter account index holds now, what it held at any past instant, every fill that changed it, and how a whole market is positioned. Positions exist on Hyperliquid core, HIP-3, Lighter mainnet, and Lighter on Robinhood Chain. Spot and HIP-4 have no positions routes.

| Venue | Prefix | Account key | Change log from (UTC) | Hourly snapshots from (UTC) | Live snapshot |
|-------|--------|-------------|-----------------------|-----------------------------|---------------|
| Hyperliquid core | `/v1/hyperliquid` | `0x` address | 2025-05-25 | 2026-06-07 | Every 5 minutes |
| HIP-3 | `/v1/hyperliquid/hip3` | `0x` address, optional `dex` | 2025-10-13 | 2026-06-07 | Every 5 minutes |
| Lighter mainnet | `/v1/lighter` | Integer `account_index` | 2025-01-17 | 2025-01-17 | Every 2 minutes |
| Lighter on Robinhood Chain | `/v1/rh-lighter` | Integer `account_index` | 2026-06-26 | 2026-06-26 | Every 2 minutes |

Append these paths to the venue prefix:

| Hyperliquid and HIP-3 | Lighter and Robinhood Chain | Params | Notes |
|-----------------------|-----------------------------|--------|-------|
| `GET /wallets/{address}/positions` | `GET /accounts/{account_index}/positions` | `timestamp`, `symbol`, `dex` (HIP-3), `limit`, `cursor` | Open positions at the latest live snapshot, or as of `timestamp`. `data.positions`; `data.account` on the first page of a snapshot read (HIP-3 needs `dex` or a dex-prefixed `symbol`; null when reconstructed or when a Lighter `symbol` filter is set); `data.account_seen` when empty |
| `GET /wallets/{address}/positions/history` | `GET /accounts/{account_index}/positions/history` | `start`, `end`, `symbol`, `dex` (HIP-3), `limit`, `cursor` | One row per open position per committed hour in `[start, end)` |
| `GET /wallets/{address}/positions/changes` | `GET /accounts/{account_index}/positions/changes` | `start`, `end`, `symbol`, `dex` (HIP-3), `limit`, `cursor` | One row per fill leg that changed a position, oldest first, with `start_position`, `end_position`, `event_type`, and `cause` |
| `GET /wallets/{address}/account` | `GET /accounts/{account_index}/account` | `dex` (HIP-3) | Latest account summary. Lighter returns position aggregates only (no margin or collateral fields) |
| `GET /wallets/{address}/account/history` | `GET /accounts/{account_index}/account/history` | `start`, `end`, `dex` (HIP-3), `limit`, `cursor` | Hourly account summaries |
| `GET /positions/{symbol}` | `GET /positions/{symbol}` | `hour`, `side`, `min_value`, `include_system` (Lighter), `limit`, `cursor` | Every open position in one market, largest value first; `meta.totals` on the first page |
| `GET /positions/{symbol}/summary` | `GET /positions/{symbol}/summary` | `start`, `end`, `include_system` (Lighter), `limit`, `cursor` | Long/short counts, sizes, values, average entries, and top-10 share: now, or an hourly series over `[start, end)` |
| `GET /positions` | `GET /positions` | `hour`, `include_system` (Lighter), `limit`, `cursor`, `format` | Every open position across markets at one hour (bulk) |

Two more routes support these: `GET /v1/lighter/accounts?l1_address=0x...` lists the Lighter mainnet account indices an L1 address owns (`total_accounts`, `accounts[].account_index`), and `GET /v1/data-quality/positions` reports per venue the latest live snapshot and its age, the latest hourly snapshot, `built_through`, and `finalized_through`.

How to read positions:
- `timestamp`, `hour`, `start`, and `end` are Unix milliseconds; `hour` must be an exact UTC hour. `address` is a 42-character `0x` hex string; Lighter paths take integer account indices only.
- Without `timestamp`, wallet and account routes read the latest live snapshot; `meta.stale` is `true`, with a `meta.notice`, when it is more than 12 minutes old. With `timestamp`, the response is the state after every event before that instant: an exact hour with a committed snapshot serves that snapshot (`meta.source: "snapshot"`); any other instant is reconstructed (`meta.source: "reconstructed"`), with exact size, entry, and `opened_at` and mark-based values at that instant. Snapshot-only fields are empty there: on Hyperliquid and HIP-3, `leverage.value`, `max_leverage`, `margin_used`, `liquidation_price`, `return_on_equity`, and the `cum_funding` members are `null`, `leverage.type` is `"unknown"`, and `liquidation_price_status` is `"unavailable"`. Lighter rows leave those fields empty on every read, with `leverage.type` holding the margin mode. Use `meta.source`, not a null field, to tell a reconstructed read apart.
- Reads are clamped to `meta.built_through`, adding `meta.requested_end` and `meta.clamped_to` when they are. `meta.finalized_through` marks how far the data is final; on Lighter it follows the trade finalization watermark, about a day behind, and newer Lighter rows carry `finalized: false`.
- Before coverage, `timestamp` reads and `[start, end)` ranges return 200 with an empty list, not an error. A wallet or account `/positions` read with `timestamp`, `/positions/changes`, and Lighter `/positions/history` add `meta.notice` and `meta.coverage_from`; Hyperliquid and HIP-3 `/positions/history`, `/account/history`, and summary series return the empty list alone. An `hour` with no committed snapshot, including any hour before coverage, returns 404 on `/positions/{symbol}` and `/positions`.
- An empty wallet page sets `data.account_seen`: `flat` (activity recorded, nothing open), `never_seen` (no recorded activity in covered history; `meta.notice` and `meta.coverage_from` scope the claim), or `outside_coverage` (the instant is before coverage).
- Every row carries `quality` (`complete`, `partial`, `degraded`; Lighter rows can also be `preliminary`, `unreconciled`, or `incomplete`), and `meta.quality` reports the snapshot the page was read from. A `partial` row is missing some fields, such as the mark, the entry, or the leverage, which read null or `unknown`.
- Hyperliquid and HIP-3 snapshots normally read `complete`; hourly snapshots before 2026-09-26 19:00 UTC can read `degraded` on every row. On both Lighter deployments, a snapshot of the most recent, not yet reconciled day can read `degraded` in `meta.quality` while its rows read `preliminary`; those rows read `complete` once the venue's daily reconcile has covered them, which `meta.finalized_through` tracks. Report the quality with the numbers rather than treating `degraded` as missing data.
- Numbers are decimal strings and `data` timestamps are RFC 3339 UTC. Lighter `account_index` values are strings in responses, with `account_kind` (`user`, `settlement`, `insurance`, `system`). Lighter market and bulk routes exclude system accounts unless `include_system=true`.
- Market routes (`/positions/{symbol}`, `/positions`) read committed snapshots only: the latest live tick by default for `/positions/{symbol}`, the latest hourly snapshot by default for `/positions`, or `hour=H`, echoed as `meta.snapshot_ts`.
- Cursors are signed and bound to the route, account or market, and filters: pass `meta.next_cursor` back unchanged with the same parameters. Market and bulk cursors pin the first page's snapshot; if it has been replaced or expired, the next page returns 409 `snapshot_advanced`, so restart without a cursor.
- Limits: wallet and account routes default 500, max 5,000 rows; market routes default 100, max 2,000; summary series cover at most 168 hours per page; bulk `/positions` defaults to 1,000 rows, max 2,000 as JSON. Where Arrow is enabled, `format=arrow` (or `Accept: application/vnd.apache.arrow.stream`) returns an Arrow IPC stream of up to 50,000 rows with pagination in the `x-next-cursor` header; check `Content-Type`, because JSON is the default.

**Billing:** positions routes bill like trades: one credit per 1,000 rows returned, with a minimum of one credit per request. Account summary routes (`/account`, `/account/history`) and the Lighter L1 lookup cost one credit per request. Plan history windows (Free: rolling 30 days) apply to `timestamp`, `hour`, and `[start, end)`.

### Data Quality (`/v1/data-quality`)

| Endpoint | Params | Notes |
|----------|--------|-------|
| `GET /status` | -- | System health status |
| `GET /coverage` | -- | Coverage summary across venue APIs |
| `GET /coverage/{exchange}` | -- | Coverage for one venue scope: `hyperliquid`, `hip3`, `hip4`, `spot`, `lighter`, or `rh-lighter` |
| `GET /coverage/{exchange}/{symbol}` | `from`, `to` | Symbol-level coverage + gaps |
| `GET /incidents` | `status`, `exchange`, `since`, `limit`, `offset` | List incidents |
| `GET /incidents/{id}` | -- | Single incident |
| `GET /latency` | -- | Ingestion latency metrics |
| `GET /sla` | `year`, `month` | SLA compliance report |
| `GET /positions` | -- | Account positions freshness per venue: live snapshot age, latest hourly snapshot, `built_through`, `finalized_through` |

### WebSocket Channels

Connect to `wss://api.0xarchive.io/ws?apiKey=KEY&version=2026-10-01`. WebSocket access (including all L4 channels) is available on every tier, starting with Free (10 subscriptions / 2 connections / 10x replay). Live and replay support below matches `GET /v1/capabilities`. Lighter mainnet and Lighter on Robinhood Chain channels are listed separately under [Lighter channels](#lighter-channels).

| Scope | Channels | Live | Replay |
|-------|----------|------|--------|
| Hyperliquid | `orderbook`, `trades`, `liquidations`, `open_interest`, `funding` | Yes | Yes |
| Hyperliquid | `candles` | No | Yes |
| Hyperliquid | `ticker`, `all_tickers` | Yes | No |
| Hyperliquid | `orderbook_full`, `l4_diffs`, `l4_orders` | Yes | Yes (bulk), from 2026-03-11 01:03 UTC |
| HIP-3 | `hip3_orderbook`, `hip3_trades`, `hip3_liquidations`, `hip3_open_interest`, `hip3_funding` | Yes | Yes |
| HIP-3 | `hip3_candles` | No | Yes |
| HIP-3 | `hip3_orderbook_full`, `hip3_l4_diffs`, `hip3_l4_orders` | Yes | Yes (bulk), from 2026-03-11 01:03 UTC |
| HIP-4 | `hip4_trades` | Yes | Yes |
| HIP-4 | `hip4_orderbook`, `hip4_open_interest` | No (replay only) | Yes |
| HIP-4 | `hip4_l4_diffs`, `hip4_l4_orders` | Yes | Yes (bulk), from 2026-05-02 07:47 UTC |
| Spot | `spot_orderbook`, `spot_trades` | Yes | No |
| Spot | `spot_twap` | No (use REST `/twap`) | No |
| Spot | `spot_l4_diffs`, `spot_l4_orders` | Yes | Yes (bulk), from 2026-05-05 22:57 UTC (PURR-USDC from 2026-03-11 01:03 UTC) |

- Trades channels send one row per side per fill. `liquidations` and `hip3_liquidations` events are fill rows with `is_liquidation: true`, the same shape as the matching trades channel.
- `orderbook_full` and `hip3_orderbook_full` carry full-depth L2 (every level) derived from L4. `*_l4_diffs` carry L4 orderbook diffs with user attribution; `*_l4_orders` carry order lifecycle events. Spot symbols are dashed (`HYPE-USDC`) on every Spot channel. HIP-4 channels take the coin with its `#` prefix (`#` followed by the bare numeric), as `GET /v1/hyperliquid/hip4/instruments` lists it; the bare numeric alone is refused with `invalid_symbol`.
- `hip4_orderbook`, `hip4_open_interest`, and `spot_twap` accept a live subscription but do not stream yet, so a live subscribe to them is acknowledged and then sends no data. Replay `hip4_orderbook` and `hip4_open_interest` for their history, and read Spot TWAP statuses from REST (`GET /v1/hyperliquid/spot/twap/{symbol}`).
- L4 and full-depth replay is bulk and single-channel. It starts at the nearest L4 checkpoint at or before `start`, sends it as an `l4_snapshot`, then streams ordered `l4_batch` pages through `end`. `speed` is ignored, `replay.seek` is refused, and these channels cannot join a multi-channel replay. Full-depth replay batches are L2 level deltas (`side`, `px`, `sz`, `n`, `bn`), one per 100 ms of event time, as in live. A `start` before the first checkpoint returns `range_before_coverage`.
- A live subscribe to a channel without a live feed (such as `candles`) and a replay on a channel without replay answer `{"type":"error","error_code":"unsupported_for_venue",...}`. The exceptions are the three channels above that acknowledge a live subscribe without streaming.
- `wss://stream.0xarchive.io/ws` serves only the live trades, liquidations, and L4 channels; any other channel or a replay there returns an error pointing to `wss://api.0xarchive.io/ws`.

**HIP-4 outcome events:**

| Event type | Notes |
|------------|-------|
| `outcome_settled` | Fired once per HIP-4 outcome when it resolves (`is_settled` flips to true). Payload includes `outcome_id`, `winning_side`, and the settled timestamp. |

#### Lighter channels

Lighter channels, for both deployments, are served on `wss://api.0xarchive.io/ws` only; a Lighter subscribe on `stream.0xarchive.io` returns an error pointing back to `wss://api.0xarchive.io/ws`. Mainnet symbols are the same as `GET /v1/lighter/instruments`, case-insensitive on subscribe and echoed uppercase. Lighter on Robinhood Chain uses its own `rh_lighter_*` channels, described [below](#lighter-on-robinhood-chain-channels). Live Lighter data is available on every tier and is metered per message, the same as Hyperliquid live data; per-tier subscription and connection limits and the limit of 10 subscribe operations per second per connection apply.

| Channel | Live subscribe | Replay |
|---------|----------------|--------|
| `lighter_orderbook` | Yes. Full top-20 book, one per second by default (`interval_ms` 100 to 5000) | Yes |
| `lighter_trades` | Yes. Two fill legs per trade | Yes |
| `lighter_open_interest` | Yes. Same message as `lighter_funding` | Yes |
| `lighter_funding` | Yes. Same message as `lighter_open_interest` | Yes |
| `lighter_candles` | No. Subscribe returns an error | Yes |
| `lighter_l3_orderbook` | No. Subscribe returns an error | Yes |

```json
{"op":"subscribe","channel":"lighter_orderbook","symbol":"BTC"}
{"op":"subscribe","channel":"lighter_orderbook","symbol":"BTC","interval_ms":250}
{"op":"subscribe","channel":"lighter_trades","symbol":"BTC"}
{"op":"unsubscribe","channel":"lighter_trades","symbol":"BTC"}
```

Acks and data use the same envelope as Hyperliquid live data: `{"type":"subscribed","channel":"lighter_orderbook","coin":"BTC","symbol":"BTC"}`, then `{"type":"data","channel":"lighter_orderbook","coin":"BTC","symbol":"BTC","data":{...}}`. Unsubscribe is acknowledged with `{"type":"unsubscribed",...}`.

**`lighter_orderbook`** `data` (truncated to 2 levels per side):

```json
{"coin":"BTC","time":1790294171459,"levels":[[{"px":"84368.7","sz":"0.00020","n":1},{"px":"84368.6","sz":"0.00020","n":1}],[{"px":"84368.8","sz":"0.05720","n":1},{"px":"84368.9","sz":"0.14223","n":1}]]}
```

- Every message is a full book of up to 20 levels per side, not a diff. `levels[0]` is bids, highest first; `levels[1]` is asks, lowest first.
- `px` and `sz` are decimal strings exactly as Lighter publishes them. `n` is always `1` (Lighter does not publish per-level order counts). `time` is Lighter's book update time in ms.
- The newest book is sent at most once per interval: 1000 ms by default, or `interval_ms` between 100 and 5000 inclusive. Each book sent is one metered message. The current book is sent on subscribe when one is available; illiquid markets can go minutes without a change.
- `interval_ms` is accepted only on `lighter_orderbook`. On another channel the subscribe returns `interval_ms is only supported on lighter_orderbook.`; an out-of-range value such as `50` returns `interval_ms must be between 100 and 5000 for lighter_orderbook (got 50). Leave it out for one book a second.`

**`lighter_trades`** `data` is an array of fills, two legs per trade sharing one `tid`:

```json
[{"coin":"BTC","side":"A","px":"84367.9","sz":"0.00003","time":1790294182211,"hash":"0000001dc8774b28000001a0d5d94943000000000000000000000000000000000000000000000000","tid":31944180930,"oid":562953419896990,"crossed":false,"dir":null,"fee":null,"fee_token":null,"closed_pnl":null,"start_position":"109.79011","users":["281474976623827"]},
 {"coin":"BTC","side":"B","px":"84367.9","sz":"0.00003","time":1790294182211,"hash":"0000001dc8774b28000001a0d5d94943000000000000000000000000000000000000000000000000","tid":31944180930,"oid":844421425107071,"crossed":true,"dir":null,"fee":null,"fee_token":null,"closed_pnl":null,"start_position":"0.03940","users":["713845"]}]
```

- `side` `A` is the ask side and `B` the bid side; `crossed: true` is the taker leg. `users` holds the Lighter account index as a string, `oid` is that side's order id, `start_position` is that account's signed position before the trade, and `hash` is the Lighter transaction hash. `time` is in ms.
- `fee`, `fee_token`, `closed_pnl`, and `dir` are always `null` in live messages; Lighter's live stream does not carry them.
- Count trades by distinct `tid`, not by array length. For volume, sum `sz` over one leg per `tid` (for example the `crossed: true` leg).
- Live trades are delivered in small batches as they happen and are preliminary. `GET /v1/lighter/trades/{symbol}` serves the finalized record, including fields the live stream does not carry such as fees, and returns reconciled trades only (see `meta.finalized_through`); `GET /v1/lighter/trades/{symbol}/recent` serves the preliminary tier.

**`lighter_open_interest`** and **`lighter_funding`** carry the same `data`:

```json
{"coin":"BTC","ctx":{"openInterest":"172706178.266310","funding":"0.000012","premium":"-0.000327","markPx":"84363.5","oraclePx":"84397.0","midPx":"84368.8","dayNtlVlm":"908611371.550746","dayBaseVlm":"10808.97087","prevDayPx":"84285.9","impactPxs":null}}
```

- `openInterest` is Lighter's reported open interest, the same value as `open_interest` from `GET /v1/lighter/openinterest/{symbol}/current`.
- `funding` is Lighter's current funding rate as a fraction (Lighter publishes percent; the value is divided by 100), the same as REST `funding_rate`. `premium` is also a fraction.
- `markPx` is the mark price, `oraclePx` Lighter's index price, `midPx` the mid price, `dayNtlVlm` 24h quote volume, and `dayBaseVlm` 24h base volume. `prevDayPx` is derived from the last trade price and Lighter's 24h percent change. `impactPxs` is always `null` (Lighter has no impact prices).
- Updates arrive as Lighter publishes them, about once per second per market. The latest values are sent on subscribe when available.

**Slow connections:** a client that falls behind `lighter_trades`, `lighter_open_interest`, or `lighter_funding` receives an `error` notice such as `Dropped ~N live lighter_trades messages for BTC: your connection fell behind the Lighter stream, and those trades were not delivered.` Persistent lag stops that subscription with `Stopped the lighter_trades stream for BTC: your connection is too slow to keep up. Re-subscribe to resume.` Both notices carry `error_code: "slow_consumer"`. `lighter_orderbook` always sends the newest book, so it never delivers an older book in place of a newer one.

**Replay** (`{"op":"replay",...}`) works on all six Lighter channels. With `version=2026-10-01` on the connection, orderbook, trades, open interest, and funding replay rows use the live shapes above (`levels` books, fill legs, and `ctx`); replayed finalized trades fill `fee` and `closed_pnl`, which live messages leave `null`. Without the version, replay sends `historical_data` rows in the stored replay format (for example orderbook rows with `bids`/`asks` and fill rows with `price`/`size`/`tradeId`); parse each with its own schema.

#### Lighter on Robinhood Chain channels

The Robinhood Chain deployment has its own channels, prefixed `rh_lighter_`. Live messages are identical in shape to the mainnet Lighter live messages above (`levels` books, two fill legs per trade, and `ctx` for open interest and funding), with prices in USDG. Symbols come from `GET /v1/rh-lighter/instruments`: uppercase perps (`BTC`) and dashed spot (`AAPL-USDG`). In `rh_lighter_trades`, `users` holds Robinhood Chain account indices, unrelated to mainnet indices with the same number. Live trades are preliminary; `GET /v1/rh-lighter/trades/{symbol}` serves the finalized record.

| Channel | Live subscribe | Replay |
|---------|----------------|--------|
| `rh_lighter_orderbook` | Yes. Full top-20 book, one per second by default (`interval_ms` 100 to 5000) | Yes, from 2026-08-22 18:43 UTC |
| `rh_lighter_trades` | Yes. Two fill legs per trade | Yes, from 2026-06-26 20:10:26 UTC |
| `rh_lighter_open_interest` | Yes. Perps; same message as `rh_lighter_funding` | Yes, from 2026-08-22 18:43 UTC |
| `rh_lighter_funding` | Yes. Perps; same message as `rh_lighter_open_interest` | Yes, from 2026-08-22 18:43 UTC |
| `rh_lighter_candles` | No. Subscribe returns an error | Yes, from 2026-06-26 20:10 UTC |

```json
{"op":"subscribe","channel":"rh_lighter_orderbook","symbol":"AAPL-USDG","interval_ms":500}
{"op":"subscribe","channel":"rh_lighter_trades","symbol":"BTC"}
{"op":"replay","channel":"rh_lighter_trades","symbol":"BTC","start":1782518400000,"end":1782522000000,"speed":10}
```

- `interval_ms` is accepted only on `rh_lighter_orderbook` (and `lighter_orderbook` on mainnet); errors name the deployment's book channel, for example `interval_ms is only supported on rh_lighter_orderbook.`
- Slow-connection notices and stops work as on mainnet, naming the `rh_lighter_*` channel.
- A multi-channel replay cannot mix `lighter_*` and `rh_lighter_*` channels: they are separate exchange families.

### Webhooks (`/v1/webhooks`)

Push delivery of market and account events to your HTTPS endpoint. Delivery starts on the Build plan; the estimate and dry-run previews answer on every plan. Full guide: [Webhooks](https://docs.0xarchive.io/webhooks).

| Endpoint | Body / Params | Notes |
|----------|---------------|-------|
| `GET /event-types` | -- | Event types and their condition fields |
| `GET /limits` | -- | Plan limits and current usage |
| `GET /endpoints` / `POST /endpoints` | `url`, `description` | List or create receiving endpoints (the secret is returned once) |
| `DELETE /endpoints/{id}` | -- | Delete an endpoint |
| `POST /endpoints/{id}/enable` | -- | Re-enable an endpoint disabled after failures |
| `POST /endpoints/{id}/rotate` | -- | Rotate the signing secret (old and new both sign for 24 hours) |
| `POST /endpoints/{id}/test` | -- | Send a test delivery |
| `GET /endpoints/{id}/deliveries` | `limit` | Delivery log |
| `POST /deliveries/{id}/redeliver` | -- | Send a past delivery again |
| `GET /subscriptions` / `POST /subscriptions` | `event_type`, `endpoint_id`, `filters` | List or create subscriptions. `filters` holds `venue`, `symbols`, `addresses`, `conditions`, `params`, and thresholds such as `min_notional_usd`; `GET /event-types` lists each type's fields and a `filters_example` |
| `PATCH /subscriptions/{id}` / `DELETE /subscriptions/{id}` | `filters`, `enabled` | Update in place (a new `filters` replaces the old one) or delete |
| `POST /subscriptions/{id}/resume`, `POST /subscriptions/resume` | -- | Resume one or every paused subscription |
| `POST /subscriptions/estimate` | `event_type`, `filters`, `lookback_days` | Expected delivery volume before creating |
| `POST /subscriptions/dry-run` | `event_type`, `filters`, `lookback_s`, `limit` | Events the rule would have matched recently |
| `GET /addresses` / `POST /addresses` / `DELETE /addresses/{id}` | `address`, `label` | Watched wallets for account-scoped events. `label` is an optional name of up to 64 characters; venues are chosen per subscription, in `filters.venue` |

Preview a rule with the estimate or the dry-run, then create it with the same `event_type` and `filters`. Put every constraint inside `filters`: a subscription created without `filters` matches every occurrence of its event type.

```bash
# Expected daily deliveries for BTC liquidations of at least $100,000 on Hyperliquid
curl -s -X POST -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" -H "Content-Type: application/json" \
  "https://api.0xarchive.io/v1/webhooks/subscriptions/estimate" \
  -d '{"event_type":"market.liquidation","filters":{"venue":"hyperliquid","symbols":["BTC"],"min_notional_usd":100000},"lookback_days":1}' \
  | jq '.data | {per_day_p50, per_day_max}'

# Create the same rule (Build plan and above)
curl -s -X POST -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" -H "Content-Type: application/json" \
  "https://api.0xarchive.io/v1/webhooks/subscriptions" \
  -d '{"endpoint_id":"YOUR_ENDPOINT_ID","event_type":"market.liquidation","filters":{"venue":"hyperliquid","symbols":["BTC"],"min_notional_usd":100000}}' | jq '.data'
```

Every delivery carries an `0xa-signature` header (`t=<unix seconds>,v1=<hex HMAC-SHA256 of "<t>.<raw body>">`; two `v1` values during a rotation). Verify against the raw request body, reject stale timestamps, and compare in constant time.

### Web3 Authentication (`/v1`)

Use SIWE for existing wallet accounts and x402 for paid wallet access. Free accounts are created through the standard signup at `https://www.0xarchive.io/signup`. No API key is required for these Web3 endpoints.

| Endpoint | Params | Notes |
|----------|--------|-------|
| `POST /auth/web3/challenge` | `address` (wallet address) | Returns a SIWE message to sign; does not create an account |
| `POST /auth/web3/verify` | `message`, `signature` | Signs in an existing active wallet account; an unknown wallet returns 403 and no account is created |
| `POST /web3/signup` | -- | Retired: always returns HTTP 410 `wallet_free_signup_retired` and never creates an account or key |
| `POST /web3/keys` | `message`, `signature` | List all keys for wallet |
| `POST /web3/keys/revoke` | `message`, `signature`, `key_id` | Revoke a key |
| `POST /web3/subscribe` | `tier` (`build` or `pro`), `payment-signature` header | x402 USDC subscription (see flow below) |

**Existing-wallet flow:** Call `/auth/web3/challenge` with the wallet address → sign the returned message with `personal_sign` (EIP-191) → submit it to `/auth/web3/verify`, or to `/web3/keys` to list that wallet's keys. Verification only signs in an existing wallet account.

**Free tier:** sign up at `https://www.0xarchive.io/signup`. The legacy `/web3/signup` route always returns HTTP 410.

**Paid-tier flow (x402):**

1. `POST /web3/subscribe` with `{ "tier": "build" }` → server returns 402 with `payment.amount` (micro-USDC), `payment.pay_to` (treasury address), `payment.network`.
2. Sign an EIP-712 `TransferWithAuthorization` (EIP-3009) on USDC Base:
   - Domain: `{ name: "USD Coin", version: "2", chainId: 8453, verifyingContract: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" }`
   - Type: `TransferWithAuthorization(address from, address to, uint256 value, uint256 validAfter, uint256 validBefore, bytes32 nonce)`
   - Message: `{ from: <wallet>, to: <pay_to>, value: <amount>, validAfter: 0, validBefore: <now+3600>, nonce: <32 random bytes hex> }`
3. Build x402 v2 payment payload:
   ```json
   {
     "x402Version": 2,
     "payload": {
       "signature": "0x<EIP-712 signature hex>",
       "authorization": {
         "from": "0x<wallet>",
         "to": "0x<pay_to from step 1>",
         "value": "<amount as string>",
         "validAfter": "0",
         "validBefore": "<unix timestamp as string>",
         "nonce": "0x<64 hex chars>"
       }
     }
   }
   ```
4. Base64-encode the JSON and retry: `POST /web3/subscribe` with `{ "tier": "build" }` and header `payment-signature: <base64 payload>` → receive API key + subscription.

**Important:** All `authorization` values (`value`, `validAfter`, `validBefore`) must be strings, not numbers.

## Common Parameters

| Param | Type | Description |
|-------|------|-------------|
| `start` | int | Start timestamp (Unix ms). Defaults to 24h ago. |
| `end` | int | End timestamp (Unix ms). Defaults to now. |
| `limit` | int | Max records. Default 100 on most routes. The maximum is per route family and published as `page_limit` in `GET /v1/capabilities` (for example 50,000 for trades; 10,000 for candles, L4 diffs, and order history, except 1,000 for Spot order history; 1,000 for L2 snapshots, funding, and open interest). Account positions routes have their own limits (see [Account Positions](#account-positions)). |
| `cursor` | string | Pagination cursor from `meta.next_cursor`. Pass it back unchanged; do not parse or construct it. |
| `side` | string | Trades routes on every venue, including `/recent`: `buy` or `sell` (any case). Rows still report `side` as `B` or `A`. Any other value returns 400 `invalid_parameter` with `valid_values`. Liquidation-levels and trigger-levels routes use their own `side` values. |
| `triggered` | bool | Order history on Hyperliquid, HIP-3, and HIP-4: `true` keeps only `triggered` status events, `false` every other status. On TP/SL history it filters triggered orders. |
| `interval` | string | Candle interval: `1m`, `5m`, `15m`, `30m`, `1h`, `4h`, `1d`, `1w`. Default: `1h`. For OI, funding, and prices: `1m`, `5m`, `15m`, `30m`, `1h`, `4h`, `1d`; order flow: `1m`, `5m`, `15m`, `1h`. `GET /v1/capabilities` lists `intervals` per data type. Omit for raw data: core funding is roughly 1 minute; HIP-3 funding/OI, HIP-4 OI, and Lighter funding/OI are roughly 10 seconds. |
| `depth` | int | Route-specific orderbook depth, also accepted on L2 history routes (native and full-depth). Hyperliquid-family native L2 caps at 20 levels per side; Lighter L3 caps at 250 orders per side. |
| `granularity` | string | Lighter orderbook resolution (both deployments): `checkpoint` (default), `30s`, `10s`, `1s`, `tick`. |
| `account` | int | Lighter mainnet L3 orderbook: filter by account index (e.g., `281474976710654` for LLP vault). |

## Smart Defaults

When the user does not specify a time range, default to the **last 24 hours**:

```bash
NOW=$(( $(date +%s) * 1000 ))
DAY_AGO=$(( NOW - 86400000 ))
```

For candles with no explicit range, default to a range that makes sense for the interval (e.g., last 7 days for 4h candles, last 30 days for 1d candles).

## Trade Response Fields

Each trade/fill record includes:

| Field | Type | Description |
|-------|------|-------------|
| `coin` / `symbol` | string | Trading pair symbol |
| `side` | string | `B` (buy) or `A`/`S` (sell). Filter with the `side=buy` or `side=sell` request parameter. |
| `price` | string | Execution price |
| `size` | string | Trade size |
| `timestamp` | string | ISO 8601 timestamp |
| `trade_id` | integer | Unique trade ID |
| `order_id` | integer | Associated order ID |
| `crossed` | boolean | `true` = taker, `false` = maker |
| `fee` | string | Base trading fee |
| `fee_token` | string | Fee denomination (e.g., USDC) |
| `closed_pnl` | string | Realized PnL if closing position |
| `direction` | string | `Open Long`, `Close Short`, `Long > Short`, etc. |
| `start_position` | string | Position size before trade |
| `user_address` | string | User's wallet address |
| `builder_address` | string | Builder address that routed this order. Only present when the order was placed through a builder. |
| `builder_fee` | string | Builder fee charged on this fill, paid to the builder (quote currency, typically USDC). Only present when `builder_address` is set. |
| `deployer_fee` | string | HIP-3 deployer fee share (quote currency). Negative for the maker side (rebate), positive for the taker side. HIP-3 only. |
| `priority_gas` | number | Priority fee **burned in HYPE** (not USDC) for write priority on the Hyperliquid validator queue. Independent of `builder_fee` and `deployer_fee` (paid to the network, not to a builder or deployer). Only present when the order paid for priority. |
| `cloid` | string | Client order ID |
| `twap_id` | integer | TWAP execution ID |

`builder_address`, `builder_fee`, `deployer_fee`, `priority_gas`, `cloid`, and `twap_id` are optional. They are only present when non-zero/non-empty. `deployer_fee` is specific to HIP-3. `priority_gas` appears on any order that paid for write priority (most common on HIP-3 IOC orders).

## Pagination

Paged routes set `meta.has_more`. While it is `true`, `meta.next_cursor` is present: pass it back unchanged as `cursor` with the same `start`, `end`, and filters, and do not parse or construct it. Stop when `has_more` is `false`, not when a page is shorter than `limit`: a short page can still have more, and a final page can be empty. Never send an edited cursor; some routes reject it with 400 `invalid_cursor` and others restart from the first page.

```bash
CURSOR=""
while :; do
  PAGE=$(curl -sG -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
    "https://api.0xarchive.io/v1/hyperliquid/trades/BTC" \
    --data-urlencode "start=$START" \
    --data-urlencode "end=$END" \
    --data-urlencode "limit=1000" \
    ${CURSOR:+--data-urlencode "cursor=$CURSOR"})
  echo "$PAGE" | jq -c '.data[]'
  [ "$(echo "$PAGE" | jq -r '.meta.has_more')" = "true" ] || break
  CURSOR=$(echo "$PAGE" | jq -r '.meta.next_cursor')
done
```

## Tier Limits

Every tier, Free included, has every symbol, order book depth, Lighter granularity (including `tick`), WebSocket channel and data type, with two exceptions: the live `mempool` channel on `wss://stream.0xarchive.io/ws` comes with Pro, Scale and Enterprise, and trade intent, USDC flows, breadth above the moving average and positioning come with Build, Pro, Scale and Enterprise. Tiers otherwise differ in credits, history window, request rate, and connection limits.

| Tier | Price | Credits | Historical Depth | Rate Limit |
|------|-------|---------|------------------|------------|
| Free | $0 | 50,000/mo | Rolling 30 days (30-day span per request or replay) | 15 RPS |
| Build | $49/mo | 80M/mo | Full history | 50 RPS |
| Pro | $199/mo | 400M/mo | Full history | 150 RPS |
| Scale | $799/mo | 2B/mo | Full history | 500 RPS |
| Enterprise | Custom | Custom | Full history | Custom |

Scale ($799/mo, $639 annual) also includes 20,000 WebSocket subscriptions across 16 connections, 300x replay speed, and 200 API keys.

## Error Handling

Errors return JSON with a stable `error_code`. Branch on `error_code` and the HTTP status, not on the message text:

```json
{"success": false, "code": 400, "error_code": "invalid_parameter", "error": "Invalid side 'long'. Use buy or sell.", "param": "side", "valid_values": ["buy", "sell"], "request_id": "..."}
```

`param` names the offending parameter and `valid_values` lists the accepted values when the set is small. Keep `request_id` for support.

| `error_code` | HTTP | Meaning | Action |
|--------------|------|---------|--------|
| `invalid_parameter` | 400 | A parameter is malformed or has an unsupported value | Fix `param`; do not retry unchanged |
| `invalid_symbol` | 400 | Unknown symbol for this venue | Check spelling, case, and namespace against `GET /instruments` (Spot: `GET /pairs`) |
| `invalid_interval` | 400 | Unsupported `interval` | Use one of `valid_values` |
| `invalid_cursor` | 400 | The cursor was altered or does not match the request | Pass `next_cursor` back unchanged with the same filters, or restart without `cursor` |
| `invalid_time_range` | 400 | `start` after `end`, a time in the future, or an unparsable time | Fix the window |
| `range_before_coverage` | 400 | The whole range ends before the dataset's served history | Move the window to the date named in the message or later |
| `historical_depth_exceeded` | 403 | Older than the plan's history window (Free: rolling 30 days) | Move the window forward or upgrade |
| `historical_range_exceeded` | 403 | The span exceeds the plan's per-request limit | Split into smaller windows |
| `unauthorized` | 401 | Missing or invalid API key | Set `$OXARCHIVE_API_KEY` |
| `forbidden` | 403 | The key or account may not make this request, or the plan does not include the dataset (`datatype`, `required_plans`, `upgrade_url` in the body) | Check the key status and account, or change plan; do not retry unchanged |
| `unsupported_for_venue` | 404 | This venue does not offer the data type | Use a route from `available_on` in the body, or check `GET /v1/capabilities` |
| `route_not_found` | 404 | No such route | Check the path; `GET /v1/capabilities` lists each venue's routes |
| `not_found` | 404 | The resource id does not exist (for example an outcome or incident id) | Check the id |
| `conflict` | 409 | The request conflicts with current state | Re-read state, then retry |
| `rate_limited` | 429 | Request rate or concurrency limit | Honor `Retry-After` when present; otherwise back off with jitter and lower concurrency |
| `insufficient_credits` | 429 | Monthly credits are spent | Wait for the reset or upgrade; retrying does not help |
| `internal_error` | 500 | Server error | Retry with backoff; report `request_id` if it persists |
| `upstream_unavailable` | 503 | A data store is temporarily unavailable | Retry with backoff |

Route-specific codes keep their names, for example `snapshot_advanced` (409: the snapshot a positions cursor was paging was replaced or expired; restart pagination without a cursor), `positions_unavailable`, and `api_key_limit_reached`.

Without the `0xArchive-Version` header, four codes keep their earlier names: `invalid_query_params` and `invalid_path_params` (now `invalid_parameter`), `history_window_exceeded` (now `historical_depth_exceeded`), and `request_range_exceeded` (now `historical_range_exceeded`). If an error has no `error_code`, fall back to the HTTP status.

WebSocket `{"type":"error"}` messages carry the same `error_code` next to `message`: for example `invalid_parameter` for an unknown channel, `unsupported_for_venue` for a channel without a live feed or replay, `range_before_coverage`, `rate_limited` for subscribe bursts, and `conflict` when a replay is already running. Two codes are WebSocket-only:

| `error_code` | Meaning | Action |
|--------------|---------|--------|
| `slow_consumer` | The connection fell behind a stream: messages were dropped or the stream was stopped | Re-subscribe or restart the replay to resync; consume faster or subscribe to less |
| `endpoint_unsupported` | This endpoint does not serve the channel or operation; the message names the endpoint that does | Connect to the named endpoint, for example `wss://api.0xarchive.io/ws` instead of `stream.0xarchive.io` |

## Example Queries

```bash
# What each venue offers: data types, routes, channels, live/replay, history start (public, no key)
curl -s "https://api.0xarchive.io/v1/capabilities" \
  | jq '.data[] | select(.venue == "spot") | {datatype, live, replay, available_from}'

# BTC buy-side trades from the last hour
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/trades/BTC?start=$HOUR_AGO&end=$NOW&side=buy&limit=100" | jq '{has_more: .meta.has_more, trades: .data}'

# BTC orders that triggered in the last hour
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/orders/BTC/history?start=$HOUR_AGO&end=$NOW&triggered=true&limit=100" | jq '.data'

# HIP-3 xyz:SP500 full-depth L2 history, 50 levels per side
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip3/orderbook/xyz:SP500/l2/history?start=$HOUR_AGO&end=$NOW&depth=50&limit=10" | jq '.data'

# List Hyperliquid instruments
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/instruments" | jq '.data | length'

# Current BTC orderbook (top 10 levels)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/orderbook/BTC?depth=10" | jq '.data'

# ETH trades from the last hour
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/trades/ETH?start=$HOUR_AGO&end=$NOW&limit=100" | jq '.data'

# SOL 4h candles for the last week
NOW=$(( $(date +%s) * 1000 )); WEEK_AGO=$(( NOW - 604800000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/candles/SOL?start=$WEEK_AGO&end=$NOW&interval=4h" | jq '.data'

# Current BTC funding rate
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/funding/BTC/current" | jq '.data'

# BTC open interest aggregated to 1h intervals (last week)
NOW=$(( $(date +%s) * 1000 )); WEEK_AGO=$(( NOW - 604800000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/openinterest/BTC?start=$WEEK_AGO&end=$NOW&interval=1h" | jq '.data'

# ETH funding rates aggregated to 4h intervals (last 30 days)
NOW=$(( $(date +%s) * 1000 )); MONTH_AGO=$(( NOW - 2592000000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/funding/ETH?start=$MONTH_AGO&end=$NOW&interval=4h" | jq '.data'

# HIP-3 xyz:SP500 current orderbook
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip3/orderbook/xyz:SP500" | jq '.data'

# HIP-3 xyz:SP500 orderbook history
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip3/orderbook/xyz:SP500/history?start=$HOUR_AGO&end=$NOW&limit=10" | jq '.data'

# HIP-3 xyz:SP500 candles (last 24h, 1h interval)
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip3/candles/xyz:SP500?start=$DAY_AGO&end=$NOW&interval=1h" | jq '.data'

# HIP-4 list all outcome markets
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/outcomes" | jq '.data'

# HIP-4 single outcome market detail (outcome_id = 0)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/outcomes/0" | jq '.data'

# HIP-4 orderbook for outcome 0 / side 0 (coin = "0", canonical bare numeric form)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/orderbook/0?depth=10" | jq '.data'

# HIP-4 implied-probability price history for outcome 0 / side 1 (last 24h)
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/prices/1?start=$DAY_AGO&end=$NOW&interval=1h" | jq '.data'

# HIP-4 recent trades for outcome 0 / side 0
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/trades/0/recent?limit=20" | jq '.data'

# HIP-4 list active outcomes (not yet settled)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip4/outcomes?is_settled=false" | jq '.data'

# Hyperliquid Spot list current pairs
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/pairs" | jq '.data | length'

# Hyperliquid Spot single pair detail (HYPE-USDC, dashed canonical)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/pairs/HYPE-USDC" | jq '.data'

# Hyperliquid Spot current orderbook (top 10 levels)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/orderbook/HYPE-USDC?depth=10" | jq '.data'

# Hyperliquid Spot trades for the last hour (PURR-USDC)
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/trades/PURR-USDC?start=$HOUR_AGO&end=$NOW&limit=100" | jq '.data'

# Hyperliquid Spot trades filtered to a specific wallet
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/trades/HYPE-USDC?start=$DAY_AGO&end=$NOW&user=0xYourWalletHere" | jq '.data'

# Hyperliquid Spot TWAP statuses for a symbol (last hour)
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/twap/HYPE-USDC?start=$HOUR_AGO&end=$NOW" | jq '.data'

# Hyperliquid Spot TWAP statuses for a wallet
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/twap/user/0xYourWalletHere?start=$DAY_AGO&end=$NOW" | jq '.data'

# Hyperliquid Spot data freshness (per data type)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/spot/freshness/HYPE-USDC" | jq '.data'

# Lighter BTC orderbook history (30s granularity, last hour)
NOW=$(( $(date +%s) * 1000 )); HOUR_AGO=$(( NOW - 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/lighter/orderbook/BTC/history?start=$HOUR_AGO&end=$NOW&granularity=30s&limit=100" | jq '.data'

# Lighter on Robinhood Chain markets (USDG-quoted perps like BTC and spot like AAPL-USDG)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/instruments" | jq '.data | length'

# Lighter on Robinhood Chain AAPL-USDG orderbook (top 10 levels)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/orderbook/AAPL-USDG?depth=10" | jq '.data'

# Lighter on Robinhood Chain finalized BTC trades from 2 days ago to 1 day ago (end is clamped to meta.finalized_through)
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 )); TWO_DAYS_AGO=$(( NOW - 172800000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/trades/BTC?start=$TWO_DAYS_AGO&end=$DAY_AGO&limit=100" | jq '{finalized_through: .meta.finalized_through, trades: .data}'

# Lighter on Robinhood Chain recent trades, including preliminary rows
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/trades/AAPL-USDG/recent?limit=20" | jq '{preliminary: .meta.preliminary_row_count, trades: .data}'

# Lighter on Robinhood Chain current BTC funding rate (perps only)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/funding/BTC/current" | jq '.data'

# Current positions and account summary for a Hyperliquid wallet
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/wallets/0xYourWalletHere/positions" | jq '{as_of: .meta.as_of, positions: .data.positions, account: .data.account}'

# Hyperliquid wallet positions as of 3 days ago (reconstructed unless the instant is an exact committed hour)
NOW=$(( $(date +%s) * 1000 )); T=$(( NOW - 259200000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/wallets/0xYourWalletHere/positions?timestamp=$T" | jq '{source: .meta.source, positions: .data.positions}'

# BTC position changes for a wallet over the last 24h (one row per fill leg)
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/wallets/0xYourWalletHere/positions/changes?start=$DAY_AGO&end=$NOW&symbol=BTC" | jq '.data'

# HIP-3 positions for a wallet on one dex
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/hip3/wallets/0xYourWalletHere/positions?dex=xyz" | jq '.data.positions'

# Largest BTC longs worth at least $1M, with market totals
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/positions/BTC?side=long&min_value=1000000&limit=20" | jq '{totals: .meta.totals, positions: .data}'

# BTC long/short positioning summary, hourly for the last 24h
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/positions/BTC/summary?start=$DAY_AGO&end=$NOW" | jq '.data'

# Lighter mainnet: resolve an L1 address to account indices, then read one account's positions
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/lighter/accounts?l1_address=0xYourWalletHere" | jq '.data.accounts'
ACCOUNT_INDEX=123456
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/lighter/accounts/$ACCOUNT_INDEX/positions" | jq '.data'

# Lighter on Robinhood Chain positions for one of its own account indices (no L1 lookup on this deployment).
# Take the index from a Robinhood Chain trade, liquidation, or /v1/rh-lighter/positions/{symbol} row,
# never from the mainnet lookup above: the same number is a different account here.
RH_ACCOUNT_INDEX=654321
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/rh-lighter/accounts/$RH_ACCOUNT_INDEX/positions" | jq '.data'

# Every open Hyperliquid position at one committed hour (bulk; follow meta.next_cursor)
NOW=$(( $(date +%s) * 1000 )); HOUR=$(( (NOW / 3600000 - 2) * 3600000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/positions?hour=$HOUR&limit=1000" | jq '{snapshot: .meta.snapshot_ts, next: .meta.next_cursor, rows: (.data | length)}'

# Account positions freshness per venue
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/data-quality/positions" | jq '.data'

# System health status
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/data-quality/status" | jq '.'

# SLA report for current month
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/data-quality/sla" | jq '.'

# BTC market summary (price, funding, OI, volume, liquidations in one call)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/summary/BTC" | jq '.data'

# BTC data freshness (lag per data type)
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/freshness/BTC" | jq '.data'

# BTC price history (mark/oracle/mid) aggregated to 1h
NOW=$(( $(date +%s) * 1000 )); DAY_AGO=$(( NOW - 86400000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/prices/BTC?start=$DAY_AGO&end=$NOW&interval=1h" | jq '.data'

# BTC liquidation volume aggregated to 4h buckets
NOW=$(( $(date +%s) * 1000 )); WEEK_AGO=$(( NOW - 604800000 ))
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/hyperliquid/liquidations/BTC/volume?start=$WEEK_AGO&end=$NOW&interval=4h" | jq '.data'

# Data coverage for Hyperliquid BTC
curl -s -H "x-api-key: $OXARCHIVE_API_KEY" -H "0xArchive-Version: 2026-10-01" \
  "https://api.0xarchive.io/v1/data-quality/coverage/hyperliquid/BTC" | jq '.'
```

