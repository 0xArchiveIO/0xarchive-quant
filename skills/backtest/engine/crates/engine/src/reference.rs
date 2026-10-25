//! A fixed backtest every build target runs, so their result hashes can be compared
//!. Data and strategy use only +, -, * and / on f64, which
//! IEEE 754 makes identical on every platform. Functions like `sin` or `exp` come
//! from each platform's own math library and could differ in the last bit.

use crate::{Config, Ctx, Report, Strategy, run};
use oxs_schema::Row;

const T0: i64 = 1_791_208_800_000;

/// `n` synthetic 1-minute rows from a fixed linear congruential generator.
pub fn rows(n: usize) -> Vec<Row> {
    rows_seeded(n, 0x5eed_0f0a)
}

/// Like `rows`, from another seed: a second market for portfolio checks.
pub fn rows_seeded(n: usize, seed: u64) -> Vec<Row> {
    let mut state: u64 = seed;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Top 53 bits as a uniform value in [0, 1).
        (state >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut px = 100.0;
    (0..n)
        .map(|i| {
            let open = px;
            px *= 1.0 + (next() - 0.5) * 0.004;
            let close = px;
            let hi = open.max(close) * (1.0 + next() * 0.001);
            let lo = open.min(close) * (1.0 - next() * 0.001);
            Row {
                ts: T0 + i as i64 * 60_000,
                open: Some(open),
                high: Some(hi),
                low: Some(lo),
                close: Some(close),
                mark_price: Some(close),
                funding_settled: (i % 60 == 0).then(|| (next() - 0.5) * 0.0002),
                ..Row::default()
            }
        })
        .collect()
}

/// Long when the 10-bar mean is above the 30-bar mean, flat otherwise.
#[derive(Default)]
pub struct MeanCross {
    closes: Vec<f64>,
}

impl Strategy for MeanCross {
    fn on_bar(&mut self, ctx: &mut Ctx, row: &Row) {
        let Some(c) = row.close else { return };
        self.closes.push(c);
        let n = self.closes.len();
        if n < 30 {
            return;
        }
        let mean = |k: usize| self.closes[n - k..].iter().sum::<f64>() / k as f64;
        let target = if mean(10) > mean(30) { 1.0 } else { 0.0 };
        let delta = target - ctx.position();
        if delta != 0.0 {
            let _ = ctx.market(delta);
        }
    }
}

pub fn run_reference() -> Report {
    let cfg = Config {
        builder_fee_bps: 1.0,
        ..Config::default()
    };
    run(&rows(10_000), &mut MeanCross::default(), cfg)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reference_trades_and_is_stable() {
        let a = super::run_reference();
        assert!(
            a.fills.len() > 50,
            "the reference strategy should trade: {}",
            a.fills.len()
        );
        assert_eq!(a.result_hash, super::run_reference().result_hash);
    }
}
