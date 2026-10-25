//! Known markets' limits, so a backtest of ETH or HYPE isn't run at BTC's 40x by default.
//! Hosts use them for a market the user gives no leverage for; the receipt records what
//! was used, so a later change here never changes an old run's meaning.

use crate::{Config, MarketSpec};

/// Hyperliquid's max leverage for the dataset's markets, from its `meta` on 2026-10-06.
/// Keys are dataset market names (`hyperliquid:BTC`).
const HYPERLIQUID: &[(&str, f64)] = &[
    ("hyperliquid:BTC", 40.0),
    ("hyperliquid:ETH", 25.0),
    ("hyperliquid:SOL", 20.0),
    ("hyperliquid:XRP", 20.0),
    ("hyperliquid:HYPE", 10.0),
    ("hyperliquid:ZEC", 10.0),
    ("hyperliquid:NEAR", 10.0),
    ("hyperliquid:PUMP", 10.0),
    ("hyperliquid:ENA", 10.0),
    ("hyperliquid:LIT", 5.0),
];

pub fn max_leverage(market: &str) -> Option<f64> {
    HYPERLIQUID
        .iter()
        .find(|(m, _)| *m == market)
        .map(|&(_, l)| l)
}

/// The spec for `dataset_market` traded under `name`: an explicit `max_leverage` first,
/// then the table, then the config's own limits.
pub fn spec(
    name: &str,
    dataset_market: &str,
    explicit_max_leverage: Option<f64>,
    cfg: &Config,
) -> MarketSpec {
    match explicit_max_leverage.or_else(|| max_leverage(dataset_market)) {
        Some(l) => MarketSpec::with_leverage(name, l),
        None => MarketSpec::from_config(name, cfg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btc_from_the_table_equals_the_default_config() {
        let cfg = Config::default();
        let s = spec("BTC", "hyperliquid:BTC", None, &cfg);
        assert_eq!(
            (s.max_leverage, s.maintenance_margin),
            (cfg.max_leverage, cfg.maintenance_margin)
        );
        assert_eq!(spec("x", "hyperliquid:ETH", None, &cfg).max_leverage, 25.0);
        assert_eq!(
            spec("x", "hyperliquid:ETH", Some(3.0), &cfg).max_leverage,
            3.0
        );
        assert_eq!(
            spec("x", "synthetic:reference", None, &cfg).max_leverage,
            40.0
        );
    }
}
