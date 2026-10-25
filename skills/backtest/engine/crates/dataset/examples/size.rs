//! Writes a synthetic year of BTC-like rows to Parquet and prints the file size.
//! Prices are rounded to a $1 tick and sizes to 5 decimals like the real data;
//! random-walk noise compresses worse than real markets, so treat this as an upper bound.
//! Run: cargo run --release -p oxs-dataset --example size

use oxs_dataset::{FileMeta, write};
use oxs_schema::Row;

fn main() {
    let mut s: u64 = 42;
    let mut u = || {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let r5 = |x: f64| (x * 1e5).round() / 1e5;
    let mut px = 86_000.0_f64;
    let mut oi = 37_000.0_f64;
    let rows: Vec<Row> = (0..525_600i64)
        .map(|i| {
            let open = px.round();
            px *= 1.0 + (u() - 0.5) * 0.003;
            let close = px.round();
            let (hi, lo) = (
                open.max(close) + (u() * 40.0).round(),
                open.min(close) - (u() * 40.0).round(),
            );
            let vol = r5(u() * 80.0);
            let buy = (vol * close * u() * 100.0).round() / 100.0;
            oi += (u() - 0.5) * 20.0;
            Row {
                ts: 1_760_000_000_000 + i * 60_000,
                open: Some(open),
                high: Some(hi),
                low: Some(lo),
                close: Some(close),
                volume: Some(vol),
                quote_volume: Some((vol * close * 100.0).round() / 100.0),
                trade_count: Some((u() * 600.0) as i32),
                vwap: Some(((open + close) / 2.0 * 10.0).round() / 10.0),
                taker_buy_quote: Some(buy),
                taker_sell_quote: Some((vol * close * 100.0).round() / 100.0 - buy),
                cvd_delta: Some(buy * 2.0 - vol * close),
                limit_orders_placed: Some(40_000 + (u() * 15_000.0) as i32),
                orders_canceled: Some(40_000 + (u() * 15_000.0) as i32),
                orders_filled: Some((u() * 600.0) as i32),
                trigger_orders_placed: Some((u() * 100.0) as i32),
                tpsl_orders_placed: Some((u() * 40.0) as i32),
                orders_triggered: Some((u() * 20.0) as i32),
                orders_force_canceled: Some((u() * 60.0) as i32),
                best_bid: Some(close - 1.0),
                best_ask: Some(close),
                spread_bps: Some(1.0 / close * 1e4),
                spread_bps_mean: Some(r5(0.12 + u() * 0.05)),
                depth_bid_10bps: Some((u() * 4e6).round()),
                depth_ask_10bps: Some((u() * 4e6).round()),
                depth_bid_50bps: Some((u() * 2e7).round()),
                depth_ask_50bps: Some((u() * 2e7).round()),
                imbalance_10bps: Some(r5(u() * 2.0 - 1.0)),
                book_snapshots: Some(48 + (u() * 4.0) as i16),
                open_interest: Some(r5(oi)),
                mark_price: Some(close),
                oracle_price: Some((close + u() * 30.0).round() / 1.0),
                mid_price: Some(close - 0.5),
                impact_bid: Some(close - 1.0),
                impact_ask: Some(close),
                funding_rate: Some((u() * 2e-5 * 1e10).round() / 1e10),
                premium: Some(((u() - 0.5) * 1e-3 * 1e10).round() / 1e10),
                funding_settled: (i % 60 == 0).then(|| (u() * 2e-5 * 1e10).round() / 1e10),
                liq_long_usd: Some(if u() < 0.1 { (u() * 1e5).round() } else { 0.0 }),
                liq_short_usd: Some(if u() < 0.1 { (u() * 1e5).round() } else { 0.0 }),
                liq_long_count: Some(0),
                liq_short_count: Some(0),
                liq_levels_long_1pct_usd: Some((u() * 5e7).round()),
                liq_levels_short_1pct_usd: Some((u() * 5e7).round()),
                liq_levels_age_ms: Some(((i % 5) * 60_000) as i32),
            }
        })
        .collect();
    let mut buf = Vec::new();
    let meta = FileMeta {
        market: "hyperliquid:BTC".into(),
        month: "synthetic-year".into(),
        dataset_version: "v0".into(),
    };
    write(&mut buf, &meta, &rows).unwrap();
    println!(
        "{} rows, {} columns, {:.1} MB",
        rows.len(),
        oxs_dataset::arrow_schema().fields().len(),
        buf.len() as f64 / 1e6
    );
}
