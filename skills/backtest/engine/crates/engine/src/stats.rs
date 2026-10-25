//! A run's headline numbers, computed once here so backtests and paper trading report the same Sharpe and drawdown. Daily figures are annualised over 365 days.

use crate::bars::{stdev, sum};
use crate::{Fill, Liquidity, MarketId};

pub const DAY_MS: i64 = 86_400_000;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Stats {
    pub start: i64,
    pub end: i64,
    pub start_equity: f64,
    pub end_equity: f64,
    /// end / start - 1.
    #[serde(rename = "return")]
    pub ret: f64,
    /// The return compounded to a year; None for a window under a day.
    pub annual_return: Option<f64>,
    /// Annualised standard deviation of daily returns.
    pub volatility: Option<f64>,
    /// Annualised Sharpe of daily returns, counted from the first day the equity moved;
    /// None with fewer than three such days or no variation.
    pub sharpe: Option<f64>,
    /// As Sharpe, over the downside deviation: the root mean square of the days' losses.
    pub sortino: Option<f64>,
    /// The largest fall from a peak, as a fraction of the peak.
    pub max_drawdown: f64,
    /// The annual return over the maximum drawdown.
    pub calmar: Option<f64>,
    /// The share of days (from the first move) with a gain.
    pub hit_rate: Option<f64>,
}

/// The equity at the end of each day from the curve's start, then at its last point: one
/// per day boundary crossed, so a day with no points repeats the last equity.
fn daily_equity(curve: &[(i64, f64)]) -> Vec<f64> {
    let Some(&(t0, e0)) = curve.first() else {
        return vec![];
    };
    let mut daily = vec![e0];
    let mut next = t0 + DAY_MS;
    let mut last = e0;
    for &(t, x) in curve {
        while t > next {
            daily.push(last);
            next += DAY_MS;
        }
        last = x;
    }
    daily.push(curve[curve.len() - 1].1);
    daily
}

/// Daily returns from the first day the equity moved: what Sharpe and the rest are taken
/// over.
pub fn daily_returns(curve: &[(i64, f64)]) -> Vec<f64> {
    let mut rets: Vec<f64> = daily_equity(curve)
        .windows(2)
        .map(|w| w[1] / w[0] - 1.0)
        .collect();
    let first = rets.iter().position(|&r| r != 0.0).unwrap_or(rets.len());
    rets.drain(..first);
    rets
}

/// Stats of an equity curve: (time, equity) points in time order, the first being the
/// window's start. None for an empty curve.
pub fn stats(curve: &[(i64, f64)]) -> Option<Stats> {
    let &(t0, e0) = curve.first()?;
    let &(t1, e1) = curve.last()?;
    let mut peak = f64::NEG_INFINITY;
    let mut dd = 0.0_f64;
    for &(_, x) in curve {
        if x > peak {
            peak = x;
        }
        if peak > 0.0 {
            dd = dd.max((peak - x) / peak);
        }
    }
    let rets = daily_returns(curve);
    let year = 365f64.sqrt();
    let enough = rets.len() > 2;
    let m = if rets.is_empty() {
        0.0
    } else {
        sum(&rets) / rets.len() as f64
    };
    let sd = stdev(&rets).filter(|&sd| enough && sd > 0.0);
    let sharpe = sd.map(|sd| m / sd * year);
    let downside = {
        let mut sq = 0.0;
        for &r in &rets {
            if r < 0.0 {
                sq += r * r;
            }
        }
        (sq / rets.len().max(1) as f64).sqrt()
    };
    let sortino = (enough && downside > 0.0).then(|| m / downside * year);
    let ret = e1 / e0 - 1.0;
    let days = (t1 - t0) as f64 / DAY_MS as f64;
    let annual_return =
        (days >= 1.0 && e1 > 0.0).then(|| oxs_math::pow(e1 / e0, 365.0 / days) - 1.0);
    let mut up = 0usize;
    for &r in &rets {
        if r > 0.0 {
            up += 1;
        }
    }
    Some(Stats {
        start: t0,
        end: t1,
        start_equity: e0,
        end_equity: e1,
        ret,
        annual_return,
        volatility: sd.map(|sd| sd * year),
        sharpe,
        sortino,
        max_drawdown: dd,
        calmar: annual_return.filter(|_| dd > 0.0).map(|a| a / dd),
        hit_rate: (!rets.is_empty()).then(|| up as f64 / rets.len() as f64),
    })
}

/// One market's share of a run's result.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Attribution {
    /// What the market made, after its fees and funding, marked at its last price.
    pub pnl: f64,
    pub fees: f64,
    /// Paid (positive) or received.
    pub funding: f64,
    /// Notional traded.
    pub volume: f64,
    /// Round trips: from flat (or a flip) back to flat (or the next flip).
    pub trades: usize,
    /// Round trips that made money, after fees.
    pub wins: usize,
}

/// Each market's PnL, fees, funding, volume and round trips, from its fills (oldest first),
/// final position, last price and funding.
pub fn attribution(
    fills: &[Fill],
    markets: usize,
    final_positions: &[f64],
    final_prices: &[Option<f64>],
    funding: &[f64],
) -> Vec<Attribution> {
    let mut out = vec![Attribution::default(); markets];
    let mut pos = vec![0.0; markets];
    // Cash flow of the open round trip, per market.
    let mut open = vec![0.0; markets];
    for f in fills {
        let a = &mut out[f.market];
        let flow = -f.size * f.price - f.fee;
        a.pnl += flow;
        a.fees += f.fee;
        a.volume += (f.size * f.price).abs();
        let m: MarketId = f.market;
        let before = pos[m];
        let after = before + f.size;
        if before != 0.0 && (after == 0.0 || after.signum() != before.signum()) {
            // Closes (and maybe flips): the closed part's share of this fill ends the trip.
            let closed = if after == 0.0 || f.liquidity == Liquidity::Liquidation {
                1.0
            } else {
                before.abs() / f.size.abs()
            };
            let trip = open[m] + flow * closed;
            a.trades += 1;
            if trip > 0.0 {
                a.wins += 1;
            }
            open[m] = flow * (1.0 - closed);
        } else {
            open[m] += flow;
        }
        pos[m] = after;
    }
    for m in 0..markets {
        if let Some(px) = final_prices[m] {
            out[m].pnl += final_positions[m] * px;
        }
        out[m].pnl -= funding[m];
        out[m].funding = funding[m];
    }
    out
}

/// A strategy against a benchmark, from their daily returns over the same days.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Versus {
    /// The strategy's return less the benchmark's.
    pub excess_return: f64,
    /// The strategy's beta to the benchmark.
    pub beta: Option<f64>,
    /// Annualised: the mean daily return beyond beta times the benchmark's.
    pub alpha: Option<f64>,
    pub correlation: Option<f64>,
    /// Annualised standard deviation of the daily return difference.
    pub tracking_error: Option<f64>,
    /// Annualised mean daily return difference over the tracking error.
    pub information_ratio: Option<f64>,
}

/// The strategy's curve against the benchmark's, both from the same start.
pub fn versus(curve: &[(i64, f64)], bench: &[(i64, f64)]) -> Option<Versus> {
    let a = daily_equity(curve);
    let b = daily_equity(bench);
    let n = a.len().min(b.len());
    if n < 2 {
        return None;
    }
    let ra: Vec<f64> = a[..n].windows(2).map(|w| w[1] / w[0] - 1.0).collect();
    let rb: Vec<f64> = b[..n].windows(2).map(|w| w[1] / w[0] - 1.0).collect();
    let k = ra.len() as f64;
    let (ma, mb) = (sum(&ra) / k, sum(&rb) / k);
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for i in 0..ra.len() {
        cov += (ra[i] - ma) * (rb[i] - mb);
        va += (ra[i] - ma) * (ra[i] - ma);
        vb += (rb[i] - mb) * (rb[i] - mb);
    }
    let beta = (vb > 0.0).then(|| cov / vb);
    let diff: Vec<f64> = ra.iter().zip(&rb).map(|(x, y)| x - y).collect();
    let te = stdev(&diff).filter(|&s| s > 0.0);
    let year = 365f64.sqrt();
    let end = |c: &[(i64, f64)]| c[c.len() - 1].1 / c[0].1 - 1.0;
    Some(Versus {
        excess_return: end(curve) - end(bench),
        beta,
        alpha: beta.map(|b| (ma - b * mb) * 365.0),
        correlation: (va > 0.0 && vb > 0.0).then(|| cov / (va * vb).sqrt()),
        tracking_error: te.map(|s| s * year),
        information_ratio: te.map(|s| (ma - mb) / s * year),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daily_sharpe_and_drawdown() {
        let m = 60_000;
        // Flat for two days, then +1%, -0.5%, +2%, +1% a day.
        let mut curve = vec![];
        let mut eq = 100.0;
        let moves = [0.0, 0.0, 0.01, -0.005, 0.02, 0.01];
        for (d, r) in moves.iter().enumerate() {
            for i in 0..1440 {
                curve.push((d as i64 * DAY_MS + i * m, eq));
            }
            eq *= 1.0 + r;
        }
        curve.push((6 * DAY_MS, eq));
        let s = stats(&curve).unwrap();
        assert!((s.ret - (eq / 100.0 - 1.0)).abs() < 1e-15);
        assert!((s.max_drawdown - 0.005).abs() < 1e-12);
        let rets = [0.01, -0.005, 0.02, 0.01];
        let want = sum(&rets) / 4.0 / stdev(&rets).unwrap() * 365f64.sqrt();
        assert!((s.sharpe.unwrap() - want).abs() < 1e-9, "{:?}", s.sharpe);
        let down = (0.005f64 * 0.005 / 4.0).sqrt();
        let want = sum(&rets) / 4.0 / down * 365f64.sqrt();
        assert!((s.sortino.unwrap() - want).abs() < 1e-9);
        assert_eq!(s.hit_rate, Some(0.75));
        let a = s.annual_return.unwrap();
        assert!((a - oxs_math::pow(eq / 100.0, 365.0 / 6.0) + 1.0).abs() < 1e-12);
        assert!((s.calmar.unwrap() - a / 0.005).abs() < 1e-6);
        // Against itself: beta 1, no tracking error.
        let v = versus(&curve, &curve).unwrap();
        assert!((v.beta.unwrap() - 1.0).abs() < 1e-12 && v.tracking_error.is_none());
    }

    #[test]
    fn attribution_sums_fills_marks_and_funding() {
        let fill = |market, size: f64, price, fee| Fill {
            ts: 0,
            market,
            order_id: Some(1),
            size,
            price,
            fee,
            liquidity: Liquidity::Taker,
        };
        // A: buy 2 at 10, sell 2 at 12 (a win); B: buy 1 at 5, still open at 4.
        let fills = [
            fill(0, 2.0, 10.0, 0.1),
            fill(1, 1.0, 5.0, 0.1),
            fill(0, -2.0, 12.0, 0.1),
        ];
        let a = attribution(
            &fills,
            2,
            &[0.0, 1.0],
            &[Some(12.0), Some(4.0)],
            &[0.5, 0.0],
        );
        assert!((a[0].pnl - (4.0 - 0.2 - 0.5)).abs() < 1e-12);
        assert_eq!((a[0].trades, a[0].wins), (1, 1));
        assert!((a[1].pnl - (-1.0 - 0.1)).abs() < 1e-12);
        assert_eq!(a[1].trades, 0);
        assert!((a[0].volume - 44.0).abs() < 1e-12);
    }
}
