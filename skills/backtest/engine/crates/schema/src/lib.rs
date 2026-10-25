//! The 1-minute dataset row, shared by the builder, the live stream and the engine.
//!
//! The source of truth for column meaning is `docs/dataset-schema.md`. Every column
//! is known by the bucket's close; `None` means the source had no data, never zero.

pub const SCHEMA_VERSION: &str = "1m-v0";
pub const BUCKET_MS: i64 = 60_000;

/// Floor a Unix-ms time to the open of its 1-minute bucket.
pub fn bucket_of(time_ms: i64) -> i64 {
    time_ms.div_euclid(BUCKET_MS) * BUCKET_MS
}

/// Serializes with the dataset's column names, nulls included: the live stream's row
/// format, which `oxarchive_backtest.Session.step` takes as is.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    /// Bucket open, Unix ms, UTC.
    pub ts: i64,

    // Price and volume (fills). OHLC is None on a covered minute with no fills;
    // volume, quote_volume and trade_count are then Some(0).
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub volume: Option<f64>,
    pub quote_volume: Option<f64>,
    pub trade_count: Option<i32>,
    pub vwap: Option<f64>,

    // Taker flow (fills).
    pub taker_buy_quote: Option<f64>,
    pub taker_sell_quote: Option<f64>,
    pub cvd_delta: Option<f64>,

    // Order activity (L4 orders).
    pub limit_orders_placed: Option<i32>,
    pub orders_canceled: Option<i32>,
    pub orders_filled: Option<i32>,
    pub trigger_orders_placed: Option<i32>,
    pub tpsl_orders_placed: Option<i32>,
    pub orders_triggered: Option<i32>,
    pub orders_force_canceled: Option<i32>,

    // Book (L2 snapshots inside the bucket).
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread_bps: Option<f64>,
    pub spread_bps_mean: Option<f64>,
    pub depth_bid_10bps: Option<f64>,
    pub depth_ask_10bps: Option<f64>,
    pub depth_bid_50bps: Option<f64>,
    pub depth_ask_50bps: Option<f64>,
    pub imbalance_10bps: Option<f64>,
    pub book_snapshots: Option<i16>,

    // Market context (asset context snapshots).
    pub open_interest: Option<f64>,
    pub mark_price: Option<f64>,
    pub oracle_price: Option<f64>,
    pub mid_price: Option<f64>,
    pub impact_bid: Option<f64>,
    pub impact_ask: Option<f64>,
    pub funding_rate: Option<f64>,
    pub premium: Option<f64>,
    /// The rate actually paid, only on the bucket that contains a settlement.
    pub funding_settled: Option<f64>,

    // Liquidations.
    pub liq_long_usd: Option<f64>,
    pub liq_short_usd: Option<f64>,
    pub liq_long_count: Option<i32>,
    pub liq_short_count: Option<i32>,

    // Liquidation levels (snapshots about every 5 minutes).
    pub liq_levels_long_1pct_usd: Option<f64>,
    pub liq_levels_short_1pct_usd: Option<f64>,
    pub liq_levels_age_ms: Option<i32>,
}

/// The data sources a row is built from. Coverage is tracked per source because
/// each one starts on a different date and can have its own collection gaps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Fills,
    Orders,
    Book,
    Context,
    Liquidations,
    LiqLevels,
}

impl Source {
    pub const ALL: [Source; 6] = [
        Source::Fills,
        Source::Orders,
        Source::Book,
        Source::Context,
        Source::Liquidations,
        Source::LiqLevels,
    ];

    /// First UTC date each source exists for Hyperliquid perps, per `get_data_coverage`
    /// on 2026-10-06. Rows before it have every column of that source null.
    pub fn hyperliquid_perp_start(self) -> &'static str {
        match self {
            Source::Book => "2023-04-15",
            Source::Context => "2023-05-20",
            Source::Fills => "2025-03-22",
            Source::Liquidations => "2025-12-22",
            Source::Orders => "2026-03-10",
            Source::LiqLevels => "2026-07-27",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_floor_to_the_minute() {
        assert_eq!(bucket_of(1_791_208_800_000), 1_791_208_800_000);
        assert_eq!(bucket_of(1_791_208_859_999), 1_791_208_800_000);
        assert_eq!(bucket_of(1_791_208_860_000), 1_791_208_860_000);
        assert_eq!(bucket_of(-1), -60_000);
    }
}
