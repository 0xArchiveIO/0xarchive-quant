//! The backtest receipt: everything needed to reproduce a run, and its
//! result. Saved through `POST /receipts`. Holds the strategy's source hash, never
//! its source, unless the user opts in elsewhere.
//!
//! Canonical form: JSON with fields in declaration order and params sorted by key,
//! so the same run always serializes to the same bytes and the same `receipt_id`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Config, ENGINE_VERSION, FILL_MODEL, MarketSpec, Report};

pub const RECEIPT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetRef {
    pub market: String,
    pub month: String,
    pub dataset_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigRecord {
    pub starting_cash: f64,
    pub taker_fee_bps: f64,
    pub maker_fee_bps: f64,
    pub fallback_slippage_bps: f64,
    pub max_leverage: f64,
    pub maintenance_margin: f64,
}

/// A market's limits as the run used them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarketRecord {
    pub name: String,
    pub max_leverage: f64,
    pub maintenance_margin: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultRecord {
    pub fills: usize,
    pub liquidations: u32,
    pub bad_debt: f64,
    pub funding_paid: f64,
    pub final_equity: Option<f64>,
    /// 16 hex digits.
    pub result_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub receipt_version: u32,
    pub engine_version: String,
    pub fill_model: String,
    pub schema_version: String,
    /// Where it ran: `native`, `wasm32`, `python` or `pyodide`. Informational: a strategy
    /// loaded with `load_strategy` that has no determinism warnings gives the same hash on
    /// all of them; one with warnings is compared trade by trade.
    pub platform: String,
    pub datasets: Vec<DatasetRef>,
    /// SHA-256 of the strategy source, hex.
    pub strategy_source_sha256: String,
    pub params: BTreeMap<String, serde_json::Value>,
    pub config: ConfigRecord,
    /// Every market's limits, in engine order. Empty, and left out of the JSON, for a
    /// one-market run on the config's own limits, so those receipts read as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markets: Vec<MarketRecord>,
    /// The run's window, [start, end) in ms: a strategy trades from `start` and reads data
    /// from before it only to warm up. Left out for runs over whole datasets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<(i64, i64)>,
    pub result: ResultRecord,
}

/// The `markets` a receipt records for these specs (see `Receipt::markets`).
pub fn market_records(markets: &[MarketSpec], cfg: &Config) -> Vec<MarketRecord> {
    let plain = markets.len() == 1
        && markets[0].max_leverage == cfg.max_leverage
        && markets[0].maintenance_margin == cfg.maintenance_margin;
    if plain {
        return Vec::new();
    }
    markets
        .iter()
        .map(|m| MarketRecord {
            name: m.name.clone(),
            max_leverage: m.max_leverage,
            maintenance_margin: m.maintenance_margin,
        })
        .collect()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl ResultRecord {
    pub fn of(report: &Report) -> Self {
        ResultRecord {
            fills: report.fills.len(),
            liquidations: report.liquidations,
            bad_debt: report.bad_debt,
            funding_paid: report.funding_paid,
            final_equity: report.final_equity,
            result_hash: format!("{:016x}", report.result_hash),
        }
    }
}

impl Receipt {
    pub fn new(
        platform: &str,
        datasets: Vec<DatasetRef>,
        strategy_source: &[u8],
        params: BTreeMap<String, serde_json::Value>,
        cfg: Config,
        report: &Report,
    ) -> Self {
        Self::with_result(
            platform,
            datasets,
            strategy_source,
            params,
            cfg,
            market_records(&report.markets, &cfg),
            ResultRecord::of(report),
        )
    }

    /// For hosts that keep only the summary of a run, not the engine's whole report.
    pub fn with_result(
        platform: &str,
        datasets: Vec<DatasetRef>,
        strategy_source: &[u8],
        params: BTreeMap<String, serde_json::Value>,
        cfg: Config,
        markets: Vec<MarketRecord>,
        result: ResultRecord,
    ) -> Self {
        Receipt {
            receipt_version: RECEIPT_VERSION,
            engine_version: ENGINE_VERSION.into(),
            fill_model: FILL_MODEL.into(),
            schema_version: oxs_schema::SCHEMA_VERSION.into(),
            platform: platform.into(),
            datasets,
            strategy_source_sha256: sha256_hex(strategy_source),
            params,
            config: ConfigRecord {
                starting_cash: cfg.starting_cash,
                taker_fee_bps: cfg.taker_fee_bps,
                maker_fee_bps: cfg.maker_fee_bps,
                fallback_slippage_bps: cfg.fallback_slippage_bps,
                max_leverage: cfg.max_leverage,
                maintenance_margin: cfg.maintenance_margin,
            },
            markets,
            window: None,
            result,
        }
    }

    pub fn to_canonical_json(&self) -> String {
        serde_json::to_string(self).expect("a receipt always serializes")
    }

    /// SHA-256 of the canonical JSON. Two receipts with the same id describe the same
    /// run with the same result.
    pub fn receipt_id(&self) -> String {
        sha256_hex(self.to_canonical_json().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::run_reference;

    fn receipt() -> Receipt {
        let report = run_reference();
        let cfg = Config {
            taker_fee_bps: 5.0,
            ..Config::default()
        };
        let mut params = BTreeMap::new();
        params.insert("slow".to_string(), serde_json::json!(30));
        params.insert("fast".to_string(), serde_json::json!(10));
        let ds = vec![DatasetRef {
            market: "synthetic:REF".into(),
            month: "2026-10".into(),
            dataset_version: "v0".into(),
        }];
        Receipt::new("native", ds, b"class MeanCross: ...", params, cfg, &report)
    }

    #[test]
    fn canonical_json_is_stable_and_round_trips() {
        let a = receipt();
        assert_eq!(a.to_canonical_json(), receipt().to_canonical_json());
        assert_eq!(a.receipt_id(), receipt().receipt_id());
        let back: Receipt = serde_json::from_str(&a.to_canonical_json()).unwrap();
        assert_eq!(back, a);
        // Params are sorted by key whatever order they were inserted in.
        assert!(
            a.to_canonical_json()
                .contains(r#""params":{"fast":10,"slow":30}"#)
        );
    }

    #[test]
    fn one_market_on_config_limits_leaves_markets_out() {
        assert!(!receipt().to_canonical_json().contains("markets"));
        let cfg = Config::default();
        let two = [
            MarketSpec::from_config("BTC", &cfg),
            MarketSpec::with_leverage("ETH", 25.0),
        ];
        let recs = market_records(&two, &cfg);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[1].maintenance_margin, 0.02);
        assert_eq!(
            market_records(&[MarketSpec::with_leverage("ETH", 25.0)], &cfg).len(),
            1
        );
    }

    #[test]
    fn stores_the_source_hash_not_the_source() {
        let json = receipt().to_canonical_json();
        assert!(!json.contains("MeanCross"));
        assert_eq!(receipt().strategy_source_sha256.len(), 64);
    }
}
