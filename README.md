# 0xArchive Skill

Agent-ready 0xArchive market data prompts with no runtime dependency beyond `curl` and `jq`.

0xArchive is granular market data infrastructure for two venues: Hyperliquid and Lighter. Hyperliquid includes core perps, HIP-3 builder perps, HIP-4 outcome markets, and Hyperliquid Spot; HIP-3, HIP-4, and Spot stay under the Hyperliquid namespace. Lighter has two deployments: mainnet (`/v1/lighter`) and Robinhood Chain (`/v1/rh-lighter`, USDG-quoted perps and spot such as `AAPL-USDG`). Account positions are available on Hyperliquid core, HIP-3, and both Lighter deployments. This repository ships the local skill package for Claude Code, ChatGPT Codex, and other skill-capable coding-agent environments.

Use this repo when Claude Code, ChatGPT Codex, or another local skill-capable coding agent needs the fastest route from `X-API-Key` to one live market-data answer. Use [AI Clients](https://docs.0xarchive.io/ai-clients) to choose between skills, MCP, CLI, markdown docs, `llms.txt`, OpenAPI, and other Claude Code or ChatGPT Codex routes.

## Install

```bash
git clone https://github.com/0xArchiveIO/0xarchive-skill.git

# Claude Code: project skill
mkdir -p .claude/skills/0xarchive
cp 0xarchive-skill/skills/query/SKILL.md .claude/skills/0xarchive/SKILL.md

# ChatGPT Codex: repo-scoped skill
mkdir -p .agents/skills/0xarchive
cp 0xarchive-skill/skills/query/SKILL.md .agents/skills/0xarchive/SKILL.md

export OXARCHIVE_API_KEY="0xa_your_api_key"
```

For a skill available in every project, copy it to `~/.claude/skills/0xarchive/` (Claude Code) or `~/.agents/skills/0xarchive/` (ChatGPT Codex) instead.

If your workflow already manages skills through OpenClaw, install the same skill with:

```bash
openclaw install 0xarchive
```

## First Answer

The skill is named `0xarchive`. How you call it depends on the client:

| Client | Call it with | Example |
| --- | --- | --- |
| Claude Code | `/0xarchive` | `/0xarchive BTC funding rate` |
| ChatGPT Codex CLI or IDE extension | `$0xarchive`, or pick it from `/skills` | `$0xarchive BTC funding rate` |

Both clients can also load the skill on their own when a request matches its description, so a plain question such as "What is the BTC funding rate on Hyperliquid?" works too.

Expected result: current or recent 0xArchive market data, or a direct auth/configuration error.

## Backtest Skill: `0xarchive-backtest`

Reverse engineer any trader, or test any trading idea, the way a careful quant desk would. Hand your agent a wallet, a one-line idea, a tweet, a Pine script, a paper, someone else's backtest or a strategy file, and it comes back with a verdict and a receipt anyone can rerun.

```bash
# Claude Code: project skill (the whole directory: it has references, a script and a template)
mkdir -p .claude/skills
cp -R 0xarchive-skill/skills/backtest .claude/skills/0xarchive-backtest

# ChatGPT Codex
mkdir -p .agents/skills
cp -R 0xarchive-skill/skills/backtest .agents/skills/0xarchive-backtest
```

It needs the `0xarchive-strategy` engine (the `0xs` command) and a directory of 0xArchive minute Parquet files to backtest on. The trader route also needs `OXARCHIVE_API_KEY`.

| Ask | What happens |
|---|---|
| `/0xarchive-backtest reverse engineer 0x...` | Pulls the wallet's position changes, profiles its style, runs its book through the engine, tests skill against luck, and scores candidate rules by how much of the book they reproduce |
| `/0xarchive-backtest does a funding flip predict a reversal?` | Writes the claim as a spec with a kill condition, runs an event study, then a strategy through the gauntlet |
| `/0xarchive-backtest port this Pine script` | Ports it to the strategy API, checks the port bar by bar, then tests it |
| `/0xarchive-backtest audit backtest.py` | Lists lookahead, survivorship, cost and fill problems line by line, then reruns it honestly |
| `/0xarchive-backtest find me something in cross-sectional momentum` | A declared search with a trial budget, then one candidate committed to the sealed holdout |

What keeps the answers honest:

- **A trial ledger.** Every backtest the agent runs is recorded, and the deflated Sharpe counts all of them, so the tenth tweak isn't reported as the first try.
- **A sealed holdout.** The last months of data can't be read until one candidate is committed, once.
- **A verdict read off fixed rules**, never the agent's judgment: `loses after costs`, `fails nulls`, `in-sample only`, `holds out of sample`, `passes deflated Sharpe`, `passes sealed holdout`.
- **Receipts.** The engine is deterministic: the same engine, data, strategy and parameters give the same result hash on every machine.

The engine's 7-day momentum example over 2025-03 to 2026-07, through the gauntlet with a 4-point grid, lands on `holds out of sample`: it beats every null and holds up out of sample, but its deflated Sharpe over 4 tries is 0.92, short of the 0.95 bar.

## Agent Integration Requirements

| Agent surface | Requirement | Setup mechanic |
| --- | --- | --- |
| Claude Code | Local skills enabled | Copy the skill into `.claude/skills/0xarchive` |
| ChatGPT Codex | ChatGPT Codex with skills enabled | Copy the skill into `.agents/skills/0xarchive` in the target repo, or into `~/.agents/skills/0xarchive` for every repo |
| Both coding agents | `OXARCHIVE_API_KEY`, `curl`, and `jq` | Ask one question first, then expand into CLI, MCP, SDKs, or Data Catalog exports |
| OpenClaw | Optional install helper | `openclaw install 0xarchive` |
| No local skill support | Shell, MCP, or docs access | Use the CLI, MCP where supported, SDKs, `llms.txt`, OpenAPI, or markdown docs |

## Claude Code And ChatGPT Codex

Claude Code and ChatGPT Codex should both start from the same 0xArchive product truth: one authenticated request first, then expand into deeper tooling only after the result is concrete. Use this SKILL.md package directly in skill-capable clients. When a client does not support local skills, use the same API key through the CLI, SDKs, MCP where supported, [llms.txt](https://www.0xarchive.io/llms.txt), [OpenAPI](https://www.0xarchive.io/openapi.json), and markdown docs.

## Usage Examples

Shown in the Claude Code form. In ChatGPT Codex, write `$0xarchive` in place of `/0xarchive`.

| Command | What you get |
|---------|-------------|
| `/0xarchive BTC funding rate` | Current funding rate for BTC |
| `/0xarchive ETH 4h candles last week` | Historical OHLCV candles |
| `/0xarchive SOL liquidations last 24h` | Recent liquidation events |
| `/0xarchive xyz:SP500 trades last hour` | Hyperliquid HIP-3 trades |
| `/0xarchive system health` | Data quality status across venue APIs |
| `/0xarchive BTC open interest on Lighter` | Lighter open-interest history |
| `/0xarchive AAPL-USDG orderbook on Lighter Robinhood Chain` | Lighter on Robinhood Chain L2 order book |
| `/0xarchive positions for wallet 0x...` | Current Hyperliquid positions and account summary |
| `/0xarchive largest BTC longs` | Market-wide positions for one symbol, largest first |
| `/0xarchive HIP-4 outcome candles last day` | HIP-4 implied-probability OHLCV |
| `/0xarchive verify wallet 0x...` | SIWE sign-in for an existing wallet account |

## Data Available

- **Orderbooks** -- Hyperliquid-family native L2 is capped at 20 levels per side. Lighter native L2 includes all served levels on both deployments; Lighter on Robinhood Chain order books begin August 22, 2026.
- **L4 Orderbooks** -- All-level user-attributed reconstruction, diffs, and checkpoints across Hyperliquid core, Spot, HIP-3, and HIP-4.
- **L3 Orderbooks** -- Lighter mainnet order-level snapshots with owner filtering, capped at 250 orders per side from March 5, 2026. Lighter on Robinhood Chain has no L3.
- **Orders** -- Order history with user attribution, order flow aggregation, and TP/SL history on Hyperliquid families, separate from the resting L4 book.
- **Trades** -- Fill-level rows. Per-fill Lighter trade rows begin January 17, 2025, exact starts vary by market, and maker/taker context is included where served. Lighter on Robinhood Chain trades begin June 26, 2026. Both Lighter deployments serve finalized trades up to `meta.finalized_through` and preliminary rows on `/recent`. HIP-3 trades begin October 13, 2025.
- **Candles** -- OHLCV aggregations from 1m to 1w where served. Lighter candles begin August 1, 2025; Lighter on Robinhood Chain candles begin June 26, 2026; HIP-4 implied-probability candles begin May 2, 2026; Hyperliquid Spot candles begin exactly 2025-03-22T10:50:00Z and accept a maximum limit of 10,000 with opaque cursors.
- **Funding Rates** -- Hyperliquid core at roughly one minute. HIP-3 begins February 16, 2026; Lighter begins August 25, 2025. Both update at roughly 10 seconds. Lighter on Robinhood Chain perps begin August 22, 2026. HIP-4 and Spot have no funding.
- **Open Interest** -- Hyperliquid core, HIP-3, Lighter, and HIP-4 outcome-side OI. HIP-3 begins February 16, 2026; Lighter begins August 25, 2025; Lighter on Robinhood Chain perps begin August 22, 2026; HIP-4 begins May 2, 2026 and updates at roughly 10 seconds.
- **Liquidations** -- Completed-event history on Hyperliquid and HIP-3; Lighter live-captured liquidation-event and aggregated-volume routes from June 10, 2026; Lighter on Robinhood Chain liquidations from June 26, 2026, the venue's first trade, with rows before August 22, 2026 backfilled from the venue's finalized export.
- **Account Positions** -- Open perp positions by Hyperliquid wallet or Lighter account index: current, as of any instant, hourly history, a fill-by-fill change log, account summaries, and market-wide positioning. Change logs begin May 25, 2025 (Hyperliquid core), October 13, 2025 (HIP-3), January 17, 2025 (Lighter mainnet), and June 26, 2026 (Lighter on Robinhood Chain). Billed like trades: one credit per 1,000 rows.
- **WebSocket** -- Live and replay support is channel-specific. Lighter orderbook, trades, open interest, and funding stream live on `wss://api.0xarchive.io/ws` for both deployments (`lighter_*` and `rh_lighter_*`); Lighter candles and L3 orderbooks are replay-only. L4 channels stream live and replay on Hyperliquid core, HIP-3, HIP-4, and Spot, and the full-depth order book channels (`orderbook_full`, `hip3_orderbook_full`) do the same on Hyperliquid core and HIP-3. HIP-4 order book and open interest channels are replay-only, and Spot TWAP is served over REST only. `GET /v1/capabilities` lists live and replay support per venue and data type.
- **Price History** -- Mark, oracle, and mid price over time
- **Freshness** -- Per-data-type lag and last-updated timestamps
- **Market Summary** -- Price, funding, OI, volume, and liquidations in one call
- **Data Quality** -- Coverage, latency, SLA, incidents

## Data Catalog

For file-based historical pulls, use the [Data Catalog](https://www.0xarchive.io/data). It lets you search markets, choose datasets and dates, see a live quote, and export zstd-compressed Parquet. Lighter on Robinhood Chain exports use the exchange key `rh-lighter` with L2 order book, trades, funding, and open-interest data.

## Links

- [API Docs](https://docs.0xarchive.io)
- [Python SDK](https://github.com/0xArchiveIO/sdk-python)
- [TypeScript SDK](https://github.com/0xArchiveIO/sdk-typescript)
- [Rust SDK](https://github.com/0xArchiveIO/sdk-rust)
- [CLI](https://github.com/0xArchiveIO/0xarchive-cli)
- [MCP Server](https://docs.0xarchive.io/mcp-server)
- [Examples](https://github.com/0xArchiveIO/examples)

## Requirements

- An 0xArchive API key from [signup](https://www.0xarchive.io/signup)
- `curl` and `jq` available in your shell

## Next Paths

- First authenticated route: [Quick Start](https://docs.0xarchive.io/quickstart)
- Skill/MCP/agent choice: [AI Clients](https://docs.0xarchive.io/ai-clients)
- CLI for Claude Code, ChatGPT Codex, and other shell-first agent work: [CLI docs](https://docs.0xarchive.io/cli)
- Plans and limits: [Pricing](https://www.0xarchive.io/pricing)
- Status and changes: [Status](https://www.0xarchive.io/status), [Changelog](https://www.0xarchive.io/changelog)
