//! Point-in-time universes: which markets a strategy may trade at each moment, picked by a
//! rule from data before each pick only, so a backtest never trades a market because of
//! what it later became.
//!
//! `TopN` is our factor research's rule, which the
//! terminal's factor baskets also use: the top N markets by trailing dollar volume among
//! those with enough history, re-picked on a fixed schedule from an anchor. Markets that
//! were later delisted take part while they traded; leaving them out builds a universe of
//! survivors.

use std::collections::BTreeMap;

use oxs_schema::Row;
use serde::{Deserialize, Serialize};

pub const HOUR_MS: i64 = 3_600_000;

/// One hour of a market.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hour {
    /// The hour's start, ms.
    pub start: i64,
    /// The last close in the hour, if any traded.
    pub close: Option<f64>,
    /// Quote volume in the hour.
    pub volume: f64,
    /// The last open interest in the hour, in coins.
    pub open_interest: Option<f64>,
    /// The last funding rate in the hour.
    pub funding: Option<f64>,
}

/// One market's hourly series on its own hours, built from its minute rows.
#[derive(Clone, Default)]
pub struct Hourly {
    hours: Vec<Hour>,
}

impl Hourly {
    /// The hours that end at or before `at` and start at or after `at - hours` hours:
    /// what a pick at `at` may look at.
    pub fn before(&self, at: i64, hours: i64) -> &[Hour] {
        let lo = self
            .hours
            .partition_point(|x| x.start < at - hours * HOUR_MS);
        let hi = self.hours.partition_point(|x| x.start < at);
        &self.hours[lo..hi.max(lo)]
    }

    /// The first hour with a price, if any.
    pub fn first_price(&self) -> Option<i64> {
        self.hours
            .iter()
            .find(|x| x.close.is_some())
            .map(|x| x.start)
    }

    /// The hour after the last row: where the series ends.
    pub fn end(&self) -> Option<i64> {
        self.hours.last().map(|x| x.start + HOUR_MS)
    }

    /// Open interest in dollars at `at`: the latest open interest and close before it.
    pub fn open_interest_usd(&self, at: i64) -> Option<f64> {
        let hi = self.hours.partition_point(|x| x.start < at);
        let past = &self.hours[..hi];
        let oi = past.iter().rev().find_map(|h| h.open_interest)?;
        let px = past.iter().rev().find_map(|h| h.close)?;
        Some(oi * px)
    }

    /// Adds a minute row. Rows must come in increasing `ts` order.
    pub fn push(&mut self, r: &Row) {
        let h = r.ts - r.ts.rem_euclid(HOUR_MS);
        if self.hours.last().is_none_or(|x| x.start != h) {
            self.hours.push(Hour {
                start: h,
                ..Hour::default()
            });
        }
        let last = self.hours.last_mut().expect("pushed above");
        if let Some(c) = r.close {
            last.close = Some(c);
        }
        if let Some(q) = r.quote_volume {
            last.volume += q;
        }
        if let Some(oi) = r.open_interest {
            last.open_interest = Some(oi);
        }
        if let Some(f) = r.funding_rate {
            last.funding = Some(f);
        }
    }
}

/// What `TopN` ranks by.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum By {
    /// Dollar volume over the trailing window.
    #[default]
    Volume,
    /// Open interest in dollars at the pick.
    OpenInterest,
}

/// The top `n` markets by dollar volume over the trailing `window_hours` (or by open
/// interest, `by`), among markets with at least `min_history_hours` hours of prices,
/// re-picked every `every_hours` from `anchor_ms`. Each pick uses the hours before it only.
/// A pick with fewer than `n / 2` eligible markets is skipped, as at the start of the data.
/// Filters: at least `min_volume` dollars traded over the window, none of `exclude`, and at
/// most `per_sector` markets of each sector in `sectors` (0: no limit).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TopN {
    pub n: usize,
    pub window_hours: i64,
    pub min_history_hours: i64,
    pub every_hours: i64,
    pub anchor_ms: i64,
    /// An hour with no trade still counts as history when a price is at most this many
    /// hours old (our factor research carries prices forward six hours).
    pub carry_hours: i64,
    #[serde(default)]
    pub by: By,
    #[serde(default)]
    pub min_volume: f64,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub sectors: BTreeMap<String, String>,
    #[serde(default)]
    pub per_sector: usize,
}

impl Default for TopN {
    /// our factor research's: top 25 by 30-day notional, 60 days of history, every 30 days from
    /// 2025-03-22 00:00 UTC (the first clean hour of Hyperliquid candles).
    fn default() -> Self {
        TopN {
            n: 25,
            window_hours: 30 * 24,
            min_history_hours: 60 * 24,
            every_hours: 30 * 24,
            anchor_ms: 1_742_601_600_000,
            carry_hours: 6,
            by: By::Volume,
            min_volume: 0.0,
            exclude: Vec::new(),
            sectors: BTreeMap::new(),
            per_sector: 0,
        }
    }
}

/// Members from `at` (ms, an hour boundary) until the next pick.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pick {
    pub at: i64,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Universe {
    /// The built-in rule that made it, or None for a user's own rule.
    pub rule: Option<TopN>,
    pub picks: Vec<Pick>,
}

/// Pick times: every `every_hours` from `anchor_ms`, up to the last hour any market has.
pub fn schedule(anchor_ms: i64, every_hours: i64, markets: &[(String, Hourly)]) -> Vec<i64> {
    let end = markets
        .iter()
        .filter_map(|(_, h)| h.end())
        .max()
        .unwrap_or(anchor_ms);
    let mut out = Vec::new();
    let mut at = anchor_ms;
    while at < end {
        out.push(at);
        at += every_hours.max(1) * HOUR_MS;
    }
    out
}

impl Universe {
    /// The members at `ts` (ms): the latest pick at or before it, in rank order. Empty
    /// before the first pick.
    pub fn members_at(&self, ts: i64) -> &[String] {
        let i = self.picks.partition_point(|p| p.at <= ts);
        if i == 0 {
            &[]
        } else {
            &self.picks[i - 1].members
        }
    }
}

impl TopN {
    /// Picks from `markets` (name, hourly series), on the hourly grid from `anchor_ms` to the
    /// last hour any market has.
    pub fn select(&self, markets: &[(String, Hourly)]) -> Universe {
        Universe {
            rule: Some(self.clone()),
            picks: schedule(self.anchor_ms, self.every_hours, markets)
                .into_iter()
                .filter_map(|at| self.pick_at(markets, at))
                .collect(),
        }
    }

    /// The pick at `at` (ms), from the hours before it; None when fewer than `n / 2`
    /// markets are eligible.
    pub fn pick_at(&self, markets: &[(String, Hourly)], at: i64) -> Option<Pick> {
        // The last hour used is the one ending at `at`.
        let mut ranked: Vec<(f64, usize)> = Vec::new();
        for (k, (name, h)) in markets.iter().enumerate() {
            if self.exclude.contains(name) {
                continue;
            }
            let (history, notional) = self.stats(h, at);
            if history < self.min_history_hours || notional <= 0.0 || notional < self.min_volume {
                continue;
            }
            let score = match self.by {
                By::Volume => Some(notional),
                By::OpenInterest => h.open_interest_usd(at),
            };
            if let Some(score) = score {
                ranked.push((score, k));
            }
        }
        if ranked.len() < self.n / 2 {
            return None;
        }
        // Largest first; equal scores keep the input order.
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut per: BTreeMap<&str, usize> = BTreeMap::new();
        let mut members = Vec::new();
        for &(_, k) in &ranked {
            if members.len() == self.n {
                break;
            }
            let name = &markets[k].0;
            if self.per_sector > 0
                && let Some(sector) = self.sectors.get(name)
            {
                let c = per.entry(sector).or_insert(0);
                if *c == self.per_sector {
                    continue;
                }
                *c += 1;
            }
            members.push(name.clone());
        }
        Some(Pick { at, members })
    }

    /// Hours of history on the grid from the anchor up to `at`, and dollar volume over the
    /// window before `at`.
    fn stats(&self, h: &Hourly, at: i64) -> (i64, f64) {
        let from = at - self.window_hours * HOUR_MS;
        let mut history = 0;
        let mut notional = 0.0;
        let mut last_price: Option<i64> = None;
        let mut grid = self.anchor_ms;
        for &Hour {
            start: hour,
            close,
            volume: qv,
            ..
        } in &h.hours
        {
            if hour >= at {
                break;
            }
            if hour >= from {
                notional += qv;
            }
            if hour < self.anchor_ms {
                if close.is_some() {
                    last_price = Some(hour);
                }
                continue;
            }
            // Grid hours with no row of their own, between the previous row and this one.
            if let Some(p) = last_price {
                let carried_to = (p + self.carry_hours * HOUR_MS).min(hour - HOUR_MS);
                if carried_to >= grid {
                    history += (carried_to - grid) / HOUR_MS + 1;
                }
            }
            grid = hour + HOUR_MS;
            if close.is_some() {
                last_price = Some(hour);
                history += 1;
            } else if last_price.is_some_and(|p| hour - p <= self.carry_hours * HOUR_MS) {
                history += 1;
            }
        }
        // Hours after the market's last row, up to the pick.
        if let Some(p) = last_price {
            let carried_to = (p + self.carry_hours * HOUR_MS).min(at - HOUR_MS);
            if carried_to >= grid {
                history += (carried_to - grid) / HOUR_MS + 1;
            }
        }
        (history, notional)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `hours` hours of rows from `start`, every minute, with this close and quote volume.
    fn market(start: i64, hours: i64, close: f64, qv_per_minute: f64) -> Hourly {
        let mut h = Hourly::default();
        for m in 0..hours * 60 {
            h.push(&Row {
                ts: start + m * 60_000,
                close: Some(close),
                quote_volume: Some(qv_per_minute),
                ..Row::default()
            });
        }
        h
    }

    #[test]
    fn picks_by_trailing_volume_from_earlier_hours_only() {
        let rule = TopN {
            n: 2,
            window_hours: 24,
            min_history_hours: 48,
            every_hours: 24,
            anchor_ms: 0,
            carry_hours: 6,
            ..TopN::default()
        };
        let u = rule.select(&[
            ("A".into(), market(0, 24 * 5, 1.0, 1.0)),
            ("B".into(), market(0, 24 * 5, 1.0, 3.0)),
            ("C".into(), market(0, 24 * 5, 1.0, 2.0)),
            // Busiest, but listed on day 2: eligible once it has 48 hours, on day 4.
            ("D".into(), market(24 * HOUR_MS, 24 * 4, 1.0, 9.0)),
        ]);
        let at = |d: i64| d * 24 * HOUR_MS;
        // Days 0 and 1: nobody has 48 hours yet, so no pick.
        assert_eq!(u.picks[0].at, at(2));
        assert_eq!(u.picks[0].members, ["B", "C"]);
        assert_eq!(u.picks[1].members, ["D", "B"]);
        assert!(u.members_at(at(2) - 1).is_empty());
        assert_eq!(u.members_at(at(3) - 1), ["B", "C"]);
        assert_eq!(u.members_at(at(3)), ["D", "B"]);
    }

    #[test]
    fn a_delisted_market_ages_out_with_its_volume() {
        let rule = TopN {
            n: 2,
            window_hours: 24,
            min_history_hours: 24,
            every_hours: 24,
            anchor_ms: 0,
            carry_hours: 6,
            ..TopN::default()
        };
        let u = rule.select(&[
            ("A".into(), market(0, 24 * 4, 1.0, 1.0)),
            ("B".into(), market(0, 24 * 4, 1.0, 2.0)),
            ("GONE".into(), market(0, 24 * 2, 1.0, 5.0)), // stops after day 2
        ]);
        assert_eq!(u.picks[0].members, ["GONE", "B"]); // day 1
        assert_eq!(u.picks[1].members, ["GONE", "B"]); // day 2: last day's volume still counts
        assert_eq!(u.picks[2].members, ["B", "A"]); // day 3: no volume in the window
    }

    #[test]
    fn filters_and_open_interest() {
        let with_oi = |mut h: Hourly, oi: f64| {
            for x in &mut h.hours {
                x.open_interest = Some(oi);
            }
            h
        };
        let markets = [
            ("A".into(), with_oi(market(0, 24 * 3, 1.0, 1.0), 50.0)),
            ("B".into(), with_oi(market(0, 24 * 3, 2.0, 3.0), 10.0)),
            ("C".into(), with_oi(market(0, 24 * 3, 1.0, 2.0), 30.0)),
            ("D".into(), with_oi(market(0, 24 * 3, 1.0, 0.5), 5.0)),
        ];
        let base = TopN {
            n: 2,
            window_hours: 24,
            min_history_hours: 24,
            every_hours: 24,
            anchor_ms: 0,
            ..TopN::default()
        };
        let pick = |r: TopN| r.pick_at(&markets, 2 * 24 * HOUR_MS).unwrap().members;
        assert_eq!(pick(base.clone()), ["B", "C"]);
        // By dollar open interest: A 50, C 30, B 20, D 5.
        assert_eq!(
            pick(TopN {
                by: By::OpenInterest,
                ..base.clone()
            }),
            ["A", "C"]
        );
        assert_eq!(
            pick(TopN {
                exclude: vec!["B".into()],
                ..base.clone()
            }),
            ["C", "A"]
        );
        // At least 2,000 a day: only B (4,320) and C (2,880).
        let r = TopN {
            n: 4,
            min_volume: 2_000.0,
            ..base.clone()
        };
        assert_eq!(pick(r), ["B", "C"]);
        let sectors = [("B", "l1"), ("C", "l1"), ("A", "meme")]
            .iter()
            .map(|(m, s)| (m.to_string(), s.to_string()))
            .collect();
        let r = TopN {
            n: 3,
            sectors,
            per_sector: 1,
            ..base
        };
        assert_eq!(pick(r), ["B", "A", "D"]);
    }
}
