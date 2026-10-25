//! Exits the run checks every minute, whatever the strategy's clock: per position
//! (stop-loss, take-profit, trailing stop, time stop) and per book (a drawdown stop).
//!
//! A position's entry is its average fill price since it opened from flat (or flipped);
//! adding to it averages in, reducing leaves the entry. Each minute, after the engine has
//! filled that minute's orders, the bar's high and low are checked against the levels: a
//! level reached closes the position with a reduce-only market order, which fills at the
//! next minute's open, as taker. A stop and a target reached in the same bar count as the
//! stop. The market stays flat until the strategy trades it again.
//!
//! The drawdown stop compares equity at each minute's close with its peak since the start
//! (or the last resume): past the limit it cancels every order, closes every position and
//! halts the strategy, for good or until `cooldown` has passed.

use oxs_schema::Row;

use crate::{Engine, Fill, MarketId, OrderId, OrderRequest, OrderType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Exit {
    /// Close when price moves this fraction against the entry.
    StopLoss(f64),
    /// Close when price moves this fraction in favour of the entry.
    TakeProfit(f64),
    /// Close when price falls this fraction from its best since entry (rises, for a short).
    Trailing(f64),
    /// Close this many ms after entry.
    Time(i64),
}

/// The book's drawdown stop: the fraction of peak equity, and how long to halt (None: for
/// good).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawdownStop {
    pub max: f64,
    pub cooldown_ms: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Entry {
    size: f64,
    price: f64,
    since: i64,
    /// The best price since entry, before this minute: the highest high of a long, the
    /// lowest low of a short.
    best: f64,
    /// The exit order sent and not yet filled or cancelled.
    exiting: Option<OrderId>,
}

/// Why the run closed a position or the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    StopLoss,
    TakeProfit,
    Trailing,
    Time,
    Drawdown,
}

pub struct Exits {
    rules: Vec<Exit>,
    drawdown: Option<DrawdownStop>,
    entries: Vec<Entry>,
    seen: usize,
    peak: f64,
    halted_until: Option<i64>,
    /// (minute close, market, reason) of every exit sent; market None for the drawdown stop.
    pub log: Vec<(i64, Option<MarketId>, Reason)>,
}

impl Exits {
    pub fn new(rules: Vec<Exit>, drawdown: Option<DrawdownStop>, markets: usize) -> Self {
        Exits {
            rules,
            drawdown,
            entries: vec![Entry::default(); markets],
            seen: 0,
            peak: f64::NEG_INFINITY,
            halted_until: None,
            log: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty() && self.drawdown.is_none()
    }

    /// Whether the drawdown stop has halted the strategy at minute close `t`.
    pub fn halted(&self, t: i64) -> bool {
        self.halted_until.is_some_and(|u| t < u)
    }

    fn observe(&mut self, fills: &[Fill]) {
        for f in fills {
            let e = &mut self.entries[f.market];
            let old = e.size;
            let new = old + f.size;
            if new.abs() < 1e-12 {
                *e = Entry::default();
                continue;
            }
            if old == 0.0 || old.signum() != new.signum() {
                *e = Entry {
                    size: new,
                    price: f.price,
                    since: f.ts,
                    best: f.price,
                    exiting: None,
                };
            } else if new.abs() > old.abs() {
                e.price = (old * e.price + f.size * f.price) / new;
                e.size = new;
            } else {
                e.size = new;
            }
        }
    }

    fn close(engine: &mut Engine, m: MarketId) -> Option<OrderId> {
        let pos = engine.position_of(m);
        if pos == 0.0 {
            return None;
        }
        engine
            .submit_to(
                m,
                OrderRequest {
                    size: -pos,
                    kind: OrderType::Market,
                    reduce_only: true,
                },
            )
            .ok()
    }

    /// Checks minute `rows` (closing at `t`), after the engine has stepped them, and sends
    /// any exits. Call every minute from the strategy's start.
    pub fn check(&mut self, engine: &mut Engine, rows: &[Option<&Row>], t: i64) {
        let fills = engine.fills();
        let new = fills[self.seen..].to_vec();
        self.seen = fills.len();
        self.observe(&new);
        // Keep the entries' sizes true to the engine (a liquidation, say).
        for (m, e) in self.entries.iter_mut().enumerate() {
            let pos = engine.position_of(m);
            if pos == 0.0 {
                *e = Entry::default();
            } else {
                e.size = pos;
            }
            if let Some(id) = e.exiting
                && !engine.open_orders().iter().any(|o| o.id == id)
            {
                e.exiting = None;
            }
        }

        if let Some(dd) = self.drawdown {
            if let Some(u) = self.halted_until
                && t >= u
            {
                self.halted_until = None;
                self.peak = f64::NEG_INFINITY;
            }
            let eq = engine.equity();
            if self.halted_until.is_none() {
                self.peak = self.peak.max(eq);
                if eq <= self.peak * (1.0 - dd.max) {
                    engine.cancel_all();
                    for m in 0..self.entries.len() {
                        self.entries[m].exiting = Self::close(engine, m);
                    }
                    self.halted_until = Some(dd.cooldown_ms.map_or(i64::MAX, |c| t + c));
                    self.log.push((t, None, Reason::Drawdown));
                    return;
                }
            }
        }

        for (m, row) in rows.iter().enumerate() {
            let e = self.entries[m];
            if e.size == 0.0 || e.exiting.is_some() {
                continue;
            }
            let Some((hi, lo)) = row.and_then(|r| r.high.zip(r.low)) else {
                if let Some(Exit::Time(d)) = self.rules.iter().find(|r| matches!(r, Exit::Time(_)))
                    && t - e.since >= *d
                {
                    self.entries[m].exiting = Self::close(engine, m);
                    self.log.push((t, Some(m), Reason::Time));
                }
                continue;
            };
            let long = e.size > 0.0;
            // The adverse and favourable extremes of the bar.
            let (worst, best) = if long { (lo, hi) } else { (hi, lo) };
            let against = |frac: f64| {
                if long {
                    worst <= e.price * (1.0 - frac)
                } else {
                    worst >= e.price * (1.0 + frac)
                }
            };
            let mut reason = None;
            for r in &self.rules {
                let hit = match *r {
                    Exit::StopLoss(x) => against(x).then_some(Reason::StopLoss),
                    Exit::Trailing(x) => (if long {
                        worst <= e.best * (1.0 - x)
                    } else {
                        worst >= e.best * (1.0 + x)
                    })
                    .then_some(Reason::Trailing),
                    Exit::TakeProfit(x) => (if long {
                        best >= e.price * (1.0 + x)
                    } else {
                        best <= e.price * (1.0 - x)
                    })
                    .then_some(Reason::TakeProfit),
                    Exit::Time(d) => (t - e.since >= d).then_some(Reason::Time),
                };
                // A stop counts before a target in the same bar.
                reason = match (reason, hit) {
                    (Some(Reason::TakeProfit), Some(h)) | (None, Some(h)) => Some(h),
                    (r, _) => r,
                };
            }
            let en = &mut self.entries[m];
            en.best = if long {
                en.best.max(hi)
            } else {
                en.best.min(lo)
            };
            if let Some(reason) = reason {
                self.entries[m].exiting = Self::close(engine, m);
                self.log.push((t, Some(m), reason));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, MarketSpec};

    const MIN: i64 = 60_000;

    fn bar(ts: i64, o: f64, h: f64, l: f64, c: f64) -> Row {
        Row {
            ts,
            open: Some(o),
            high: Some(h),
            low: Some(l),
            close: Some(c),
            ..Row::default()
        }
    }

    type Log = Vec<(i64, Option<usize>, Reason)>;

    /// Long 1 at the open of minute 1, then the given bars; returns (exit log, fills).
    fn run(rules: Vec<Exit>, bars: &[(f64, f64, f64, f64)]) -> (Log, Vec<Fill>) {
        let mut e = Engine::with_markets(
            Config::default(),
            vec![
                MarketSpec::with_leverage("A", 10.0),
                MarketSpec::with_leverage("B", 10.0),
            ],
        );
        let mut x = Exits::new(rules, None, 2);
        let flat = bar(0, 100.0, 100.0, 100.0, 100.0);
        e.step_markets(&[Some(&flat), None]);
        e.submit_to(
            0,
            OrderRequest {
                size: 1.0,
                kind: OrderType::Market,
                reduce_only: false,
            },
        )
        .unwrap();
        for (i, &(o, h, l, c)) in bars.iter().enumerate() {
            let ts = (i as i64 + 1) * MIN;
            let r = bar(ts, o, h, l, c);
            e.step_markets(&[Some(&r), None]);
            x.check(&mut e, &[Some(&r), None], ts + MIN);
        }
        (x.log, e.fills().to_vec())
    }

    #[test]
    fn stops_targets_trailing_and_time() {
        // Entry at 100: a 5% stop is hit by the third bar's low of 94.
        let bars = [
            (100.0, 101.0, 99.0, 100.0),
            (100.0, 100.0, 97.0, 98.0),
            (98.0, 98.0, 94.0, 95.0),
            (95.0, 95.0, 95.0, 95.0),
        ];
        let (log, fills) = run(vec![Exit::StopLoss(0.05)], &bars);
        assert_eq!(log, vec![(4 * MIN, Some(0), Reason::StopLoss)]);
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[1].size, -1.0);
        assert_eq!(fills[1].ts, 4 * MIN, "at the next minute's open");
        // A 3% trailing stop: the best high is 110, so 106.7 triggers it.
        let bars = [
            (100.0, 110.0, 100.0, 109.0),
            (109.0, 109.0, 106.0, 107.0),
            (107.0, 107.0, 107.0, 107.0),
        ];
        let (log, _) = run(vec![Exit::Trailing(0.03), Exit::TakeProfit(0.2)], &bars);
        assert_eq!(log, vec![(3 * MIN, Some(0), Reason::Trailing)]);
        // Both a stop and a target in one bar: the stop.
        let bars = [(100.0, 111.0, 94.0, 100.0), (100.0, 100.0, 100.0, 100.0)];
        let (log, _) = run(vec![Exit::TakeProfit(0.1), Exit::StopLoss(0.05)], &bars);
        assert_eq!(log[0].2, Reason::StopLoss);
        // Time: two minutes after the entry fill (at minute 1).
        let bars = [(100.0, 100.0, 100.0, 100.0); 4];
        let (log, _) = run(vec![Exit::Time(2 * MIN)], &bars);
        assert_eq!(log, vec![(3 * MIN, Some(0), Reason::Time)]);
    }

    #[test]
    fn drawdown_stop_flattens_and_halts() {
        let mut e = Engine::with_markets(
            Config::default(),
            vec![MarketSpec::with_leverage("A", 10.0)],
        );
        let mut x = Exits::new(
            vec![],
            Some(DrawdownStop {
                max: 0.1,
                cooldown_ms: Some(5 * MIN),
            }),
            1,
        );
        let r0 = bar(0, 100.0, 100.0, 100.0, 100.0);
        e.step_markets(&[Some(&r0)]);
        x.check(&mut e, &[Some(&r0)], MIN);
        e.submit_to(
            0,
            OrderRequest {
                size: 400.0,
                kind: OrderType::Market,
                reduce_only: false,
            },
        )
        .unwrap();
        let mut halted_at = None;
        for i in 1..12 {
            let px = if i < 3 { 100.0 } else { 97.0 };
            let r = bar(i * MIN, px, px, px, px);
            e.step_markets(&[Some(&r)]);
            x.check(&mut e, &[Some(&r)], (i + 1) * MIN);
            if halted_at.is_none() && x.halted((i + 1) * MIN) {
                halted_at = Some((i + 1) * MIN);
            }
        }
        // 4x long: -3% is -12% of equity.
        assert_eq!(halted_at, Some(4 * MIN));
        assert_eq!(e.position(), 0.0);
        assert!(!x.halted(9 * MIN), "resumes after the cooldown");
        assert_eq!(x.log, vec![(4 * MIN, None, Reason::Drawdown)]);
    }
}
