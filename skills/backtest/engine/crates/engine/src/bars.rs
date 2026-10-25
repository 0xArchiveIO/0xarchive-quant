//! What a strategy reads: minute rows resampled to bars (`BarBuilder`), the last bars of
//! every market (`History`) with the rolling and point-in-time reads strategies share, and
//! the clock that says when a strategy runs (`Clock`).
//!
//! A bar is closed on its boundary and only then visible, so nothing here looks ahead. Sums
//! are plain loops in a fixed order (oldest first), and the only functions used besides
//! + - * / are `sqrt` and `oxs_math`'s (log, pow), so every platform gives the same bits.

use std::collections::VecDeque;

use oxs_schema::Row;

use crate::calendar::{self, DAY_MS, HOUR_MS, Session, Zone};

pub const MINUTE_MS: i64 = 60_000;

/// How a column's minute values combine into a bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Agg {
    /// The first value (open).
    First,
    Max,
    Min,
    /// The last value; state such as a price or open interest. Carried forward over bars
    /// with none, up to `History::carry_bars`.
    Last,
    /// Volumes, counts and settled amounts.
    Sum,
    Mean,
    /// The bar's quote volume over its volume.
    Vwap,
}

macro_rules! fields {
    ($($name:ident: $ty:ty => $agg:ident),* $(,)?) => {
        /// Every column a bar has, with how it aggregates, in the dataset's order.
        pub const FIELDS: &[(&str, Agg)] = &[$((stringify!($name), Agg::$agg)),*];

        #[allow(clippy::unnecessary_cast)]
        const GET: &[fn(&Row) -> Option<f64>] = &[$(|r: &Row| r.$name.map(|v| v as f64)),*];

        #[allow(clippy::unnecessary_cast)]
        const SET: &[fn(&mut Row, Option<f64>)] = &[$(|r: &mut Row, v: Option<f64>| r.$name = v.map(|x| x as $ty)),*];
    };
}

fields!(
    open: f64 => First, high: f64 => Max, low: f64 => Min, close: f64 => Last,
    volume: f64 => Sum, quote_volume: f64 => Sum, trade_count: i32 => Sum, vwap: f64 => Vwap,
    taker_buy_quote: f64 => Sum, taker_sell_quote: f64 => Sum, cvd_delta: f64 => Sum,
    limit_orders_placed: i32 => Sum, orders_canceled: i32 => Sum, orders_filled: i32 => Sum,
    trigger_orders_placed: i32 => Sum, tpsl_orders_placed: i32 => Sum,
    orders_triggered: i32 => Sum, orders_force_canceled: i32 => Sum,
    best_bid: f64 => Last, best_ask: f64 => Last, spread_bps: f64 => Last,
    spread_bps_mean: f64 => Mean,
    depth_bid_10bps: f64 => Last, depth_ask_10bps: f64 => Last,
    depth_bid_50bps: f64 => Last, depth_ask_50bps: f64 => Last,
    imbalance_10bps: f64 => Last, book_snapshots: i16 => Sum,
    open_interest: f64 => Last, mark_price: f64 => Last, oracle_price: f64 => Last,
    mid_price: f64 => Last, impact_bid: f64 => Last, impact_ask: f64 => Last,
    funding_rate: f64 => Last, premium: f64 => Last, funding_settled: f64 => Sum,
    liq_long_usd: f64 => Sum, liq_short_usd: f64 => Sum,
    liq_long_count: i32 => Sum, liq_short_count: i32 => Sum,
    liq_levels_long_1pct_usd: f64 => Last, liq_levels_short_1pct_usd: f64 => Last,
    liq_levels_age_ms: i32 => Last,
);

/// Column `f` (an index into `FIELDS`) of a row, as f64.
pub fn get(r: &Row, f: usize) -> Option<f64> {
    GET[f](r)
}

/// Sets column `f` of a row from an f64 (counts are truncated to integers).
pub fn set(r: &mut Row, f: usize, v: Option<f64>) {
    SET[f](r, v)
}

/// The index of a column in `FIELDS`.
pub fn field_id(name: &str) -> Option<usize> {
    FIELDS.iter().position(|(n, _)| *n == name)
}

/// The columns a bar of `fields` needs read from the minute rows.
pub fn columns_for(fields: &[usize]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for &f in fields {
        let need: &[&str] = match FIELDS[f].1 {
            Agg::Vwap => &["quote_volume", "volume"],
            _ => &[FIELDS[f].0],
        };
        for c in need {
            if !out.contains(c) {
                out.push(*c);
            }
        }
    }
    out
}

#[derive(Clone, Copy, Default)]
struct Acc {
    v: Option<f64>,
    /// Mean: values seen. Vwap: the volume.
    w: f64,
}

/// Builds one bar per market from its minute rows.
#[derive(Clone)]
pub struct BarBuilder {
    fields: Vec<usize>,
    markets: usize,
    accs: Vec<Acc>,
    seen: Vec<bool>,
}

impl BarBuilder {
    pub fn new(fields: Vec<usize>, markets: usize) -> Self {
        BarBuilder {
            accs: vec![Acc::default(); fields.len() * markets],
            seen: vec![false; markets],
            fields,
            markets,
        }
    }

    pub fn push(&mut self, m: usize, r: &Row) {
        self.seen[m] = true;
        let nf = self.fields.len();
        for (k, &f) in self.fields.iter().enumerate() {
            let a = &mut self.accs[m * nf + k];
            let agg = FIELDS[f].1;
            if agg == Agg::Vwap {
                if let (Some(q), Some(v)) = (r.quote_volume, r.volume) {
                    a.v = Some(a.v.unwrap_or(0.0) + q);
                    a.w += v;
                }
                continue;
            }
            let Some(x) = GET[f](r) else { continue };
            a.v = Some(match (agg, a.v) {
                (_, None) => match agg {
                    Agg::Sum | Agg::Mean => 0.0 + x,
                    _ => x,
                },
                (Agg::First, Some(v)) => v,
                (Agg::Max, Some(v)) => v.max(x),
                (Agg::Min, Some(v)) => v.min(x),
                (Agg::Last, Some(_)) => x,
                (Agg::Sum | Agg::Mean, Some(v)) => v + x,
                (Agg::Vwap, _) => unreachable!(),
            });
            if agg == Agg::Mean {
                a.w += 1.0;
            }
        }
    }

    /// The finished bar of every market, `fields.len()` values per market (all None for a
    /// market with no row), and resets for the next bar.
    pub fn finish(&mut self) -> Vec<Option<f64>> {
        let nf = self.fields.len();
        let mut out = vec![None; nf * self.markets];
        for m in 0..self.markets {
            if !self.seen[m] {
                continue;
            }
            for (k, &f) in self.fields.iter().enumerate() {
                let a = self.accs[m * nf + k];
                out[m * nf + k] = match FIELDS[f].1 {
                    Agg::Mean => a.v.map(|v| v / a.w),
                    Agg::Vwap => a.v.filter(|_| a.w > 0.0).map(|q| q / a.w),
                    _ => a.v,
                };
            }
        }
        self.accs.iter_mut().for_each(|a| *a = Acc::default());
        self.seen.iter_mut().for_each(|s| *s = false);
        out
    }

    /// Whether any market has a row in the open bar.
    pub fn any(&self) -> bool {
        self.seen.iter().any(|&s| s)
    }

    /// Writes a finished bar of market `m` into a row (columns not in `fields` stay None).
    pub fn to_row(&self, bar: &[Option<f64>], m: usize, ts: i64) -> Row {
        let nf = self.fields.len();
        let mut r = Row {
            ts,
            ..Row::default()
        };
        for (k, &f) in self.fields.iter().enumerate() {
            SET[f](&mut r, bar[m * nf + k]);
        }
        r
    }
}

/// The reads a strategy makes on one column of the history.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Op {
    /// The latest bar's value.
    Last,
    /// The value `n` bars before the latest.
    Ago,
    /// The return over the last `n` bars.
    Ret,
    /// The sample standard deviation of the last `n` one-bar returns. A missing return
    /// counts as 0, and so does one whose size is `clip` or more (a bad print).
    Vol {
        clip: Option<f64>,
    },
    /// Over the values of the last `n` bars that have one.
    Mean,
    Sum,
    Std,
    Min,
    Max,
    /// The mean one-bar return, over the last `n` periods of `period` bars, of the bars at
    /// the same point in the period as the bar `ahead` bars from the latest: with `ahead`
    /// 1, the coming bar's typical move (this hour's, over the last n days); with 0, the
    /// latest bar's, from the periods before it. Returns of `clip` or more are skipped as
    /// bad prints, like missing ones.
    Seasonal {
        period: usize,
        ahead: usize,
        clip: Option<f64>,
    },
    /// The log return over the last `n` bars.
    LogRet,
    /// The exponentially weighted mean of the last `n` bars' values, the weight halving
    /// every `halflife` bars back from the latest. Missing values are left out.
    Ewma {
        halflife: f64,
    },
    /// The beta of the last `n` one-bar returns to `to`'s: their covariance over `to`'s
    /// variance, as population moments. Returns as in `Vol`: missing and
    /// bad prints count as 0.
    Beta {
        to: Ref,
        clip: Option<f64>,
    },
    /// The correlation of the last `n` one-bar returns with `to`'s.
    Corr {
        to: Ref,
        clip: Option<f64>,
    },
    /// The sum of the last `n` residual returns against `to`: each bar's return less its
    /// beta times `to`'s, the beta fitted on the `fit` returns before that bar (residual
    /// momentum usually takes a 60-day beta and a 15-day sum).
    Resid {
        to: Ref,
        fit: usize,
        clip: Option<f64>,
    },
}

/// What a beta, correlation or residual is taken against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ref {
    /// One market, by its index in the history.
    Market(usize),
    /// The equal-weight mean return of the markets read.
    Basket,
}

impl Op {
    /// Bars the read spans, counting the latest.
    pub fn bars(self, n: usize) -> usize {
        match self {
            Op::Last => 1,
            Op::Ago | Op::Ret | Op::LogRet | Op::Vol { .. } => n + 1,
            Op::Beta { .. } | Op::Corr { .. } => n + 1,
            Op::Resid { fit, .. } => n + fit + 1,
            Op::Seasonal { period, ahead, .. } => n * period + 2 - ahead,
            _ => n,
        }
    }

    /// The reference a read is taken against, if any.
    pub fn reference(self) -> Option<Ref> {
        match self {
            Op::Beta { to, .. } | Op::Corr { to, .. } | Op::Resid { to, .. } => Some(to),
            _ => None,
        }
    }
}

/// A read that reaches past the bars the history keeps.
#[derive(Debug, PartialEq)]
pub struct TooShort {
    pub need: usize,
    pub kept: usize,
}

/// The last `capacity` bars of every market, for the columns `fields`. Bars are on a fixed
/// grid of `bar_ms` from the Unix epoch; a bar in which no market traded is kept as an empty
/// bar, so "n bars ago" is always n * bar_ms ago.
pub struct History {
    pub bar_ms: i64,
    pub fields: Vec<usize>,
    pub markets: usize,
    pub carry_bars: usize,
    capacity: usize,
    /// (bar start, values: market-major, `fields.len()` per market), newest last.
    slots: VecDeque<(i64, Vec<Option<f64>>)>,
    /// Every bar closed so far, including those no longer kept.
    pub closed: usize,
    open: Option<i64>,
    last_closed: Option<i64>,
    builder: BarBuilder,
    /// Bad-print repair (`set_repair`): (ratio, the close's index in `fields`).
    repair: Option<(f64, usize)>,
}

/// Bars before a new one whose closes make the repair rule's median.
pub const REPAIR_WINDOW: usize = 48;

impl History {
    pub fn new(
        bar_ms: i64,
        fields: Vec<usize>,
        markets: usize,
        capacity: usize,
        carry_bars: usize,
    ) -> Self {
        History {
            bar_ms,
            builder: BarBuilder::new(fields.clone(), markets),
            fields,
            markets,
            carry_bars,
            capacity: capacity.max(1),
            slots: VecDeque::new(),
            closed: 0,
            open: None,
            last_closed: None,
            repair: None,
        }
    }

    /// Repairs bad prints as each bar closes, from earlier bars only: a price (open, high,
    /// low, close) more than `ratio` times from the median close of the last
    /// `REPAIR_WINDOW` bars, or not above zero, becomes missing, so the close carries
    /// forward; a repaired high or low becomes the extreme of what is left of the open and
    /// close. Centred repairs take the median around the bar,
    /// which looks ahead; this is the trailing version. Needs `close` among the fields and
    /// at least five earlier closes. Kept bars grow to the window.
    pub fn set_repair(&mut self, ratio: f64) -> Result<(), &'static str> {
        let close = field_id("close").unwrap();
        let k = self
            .fields
            .iter()
            .position(|&f| f == close)
            .ok_or("repair needs close among the fields")?;
        self.repair = Some((ratio, k));
        self.capacity = self.capacity.max(REPAIR_WINDOW + 1);
        Ok(())
    }

    fn repair_bar(&self, values: &mut [Option<f64>]) {
        let Some((ratio, kc)) = self.repair else {
            return;
        };
        let nf = self.fields.len();
        let id = |name| field_id(name).and_then(|f| self.fields.iter().position(|&x| x == f));
        let (ko, kh, kl) = (id("open"), id("high"), id("low"));
        let n = self.slots.len();
        for m in 0..self.markets {
            let mut closes: Vec<f64> = (0..REPAIR_WINDOW.min(n))
                .filter_map(|b| self.slots[n - 1 - b].1[m * nf + kc])
                .collect();
            if closes.len() < 5 {
                continue;
            }
            closes.sort_by(f64::total_cmp);
            let h = closes.len() / 2;
            let a = if closes.len() % 2 == 1 {
                closes[h]
            } else {
                (closes[h - 1] + closes[h]) / 2.0
            };
            let bad =
                |v: Option<f64>| v.is_some_and(|v| v <= 0.0 || v < a / ratio || v > a * ratio);
            let slot = |k: Option<usize>| k.map(|k| m * nf + k);
            let (o, hi, lo, c) = (slot(ko), slot(kh), slot(kl), Some(m * nf + kc));
            let flags: Vec<(Option<usize>, bool)> = [o, hi, lo, c]
                .iter()
                .map(|&i| (i, i.is_some_and(|i| bad(values[i]))))
                .collect();
            if !flags.iter().any(|f| f.1) {
                continue;
            }
            for &(i, b) in &flags {
                if b && let Some(i) = i {
                    values[i] = None;
                }
            }
            let kept: Vec<f64> = [o, c].iter().flatten().filter_map(|&i| values[i]).collect();
            let fix = |reduce: fn(f64, f64) -> f64| kept.iter().copied().reduce(reduce);
            if flags[1].1
                && let Some(i) = hi
            {
                values[i] = fix(f64::max);
            }
            if flags[2].1
                && let Some(i) = lo
            {
                values[i] = fix(f64::min);
            }
        }
    }

    fn close_bar(&mut self, start: i64) {
        let mut values = self.builder.finish();
        self.repair_bar(&mut values);
        self.push_slot(start, values);
    }

    fn push_slot(&mut self, start: i64, values: Vec<Option<f64>>) {
        if self.slots.len() == self.capacity {
            self.slots.pop_front();
        }
        self.slots.push_back((start, values));
        self.closed += 1;
        self.last_closed = Some(start);
    }

    /// Adds one minute (`ts`, its start) of rows, one slot per market. Closes the bar the
    /// minute ends, so a read right after sees it.
    pub fn step(&mut self, ts: i64, rows: &[Option<&Row>]) {
        let b = ts - ts.rem_euclid(self.bar_ms);
        if let Some(o) = self.open
            && b > o
        {
            self.close_bar(o);
            self.open = None;
        }
        if self.open.is_none()
            && let Some(l) = self.last_closed
        {
            // Bars with no rows at all, between the last closed one and this.
            let missing = ((b - l) / self.bar_ms - 1).max(0) as usize;
            let empty = vec![None; self.fields.len() * self.markets];
            for i in 0..missing {
                let start = b - (missing - i) as i64 * self.bar_ms;
                if missing - i <= self.capacity {
                    self.push_slot(start, empty.clone());
                } else {
                    self.closed += 1;
                }
            }
        }
        self.open = Some(b);
        for (m, r) in rows.iter().enumerate() {
            if let Some(r) = r {
                self.builder.push(m, r);
            }
        }
        if (ts + MINUTE_MS).rem_euclid(self.bar_ms) == 0 {
            self.close_bar(b);
            self.open = None;
        }
    }

    /// The close time of the latest closed bar.
    pub fn now(&self) -> Option<i64> {
        self.last_closed.map(|s| s + self.bar_ms)
    }

    /// The bars kept.
    pub fn kept(&self) -> usize {
        self.capacity
    }

    fn raw(&self, m: usize, k: usize, back: usize) -> Option<f64> {
        let n = self.slots.len();
        if back >= n {
            return None;
        }
        self.slots[n - 1 - back].1[m * self.fields.len() + k]
    }

    /// The value `back` bars before the latest; a `Last` column carries the latest value up
    /// to `carry_bars` back over bars with none.
    pub fn value(&self, m: usize, k: usize, back: usize) -> Option<f64> {
        if FIELDS[self.fields[k]].1 != Agg::Last {
            return self.raw(m, k, back);
        }
        (0..=self.carry_bars).find_map(|c| self.raw(m, k, back + c))
    }

    /// `op` over the last `n` bars of column `k` (an index into `fields`), as of `shift` bars
    /// before the latest, for each of `markets`. All None until the history has closed
    /// enough bars.
    pub fn read(
        &self,
        op: Op,
        k: usize,
        n: usize,
        shift: usize,
        markets: &[usize],
    ) -> Result<Vec<Option<f64>>, TooShort> {
        let span = shift + op.bars(n);
        let need = span
            + if FIELDS[self.fields[k]].1 == Agg::Last {
                self.carry_bars
            } else {
                0
            };
        if need > self.capacity {
            return Err(TooShort {
                need,
                kept: self.capacity,
            });
        }
        if self.closed < span {
            return Ok(vec![None; markets.len()]);
        }
        if let Some(to) = op.reference() {
            // `to`'s one-bar returns, newest first, over every bar the read needs.
            let backs = op.bars(n) - 1;
            let rets: Vec<f64> = match op {
                Op::Beta { clip, .. } | Op::Corr { clip, .. } | Op::Resid { clip, .. } => (0
                    ..backs)
                    .map(|b| self.ref_ret(to, k, shift + b, clip, markets))
                    .collect(),
                _ => unreachable!(),
            };
            return Ok(markets
                .iter()
                .map(|&m| self.read_vs(op, m, k, n, shift, &rets))
                .collect());
        }
        Ok(markets
            .iter()
            .map(|&m| self.read_one(op, m, k, n, shift))
            .collect())
    }

    fn read_one(&self, op: Op, m: usize, k: usize, n: usize, s: usize) -> Option<f64> {
        let v = |back: usize| self.value(m, k, back);
        match op {
            Op::Last => v(s),
            Op::Ago => v(s + n),
            Op::Ret => Some(v(s)? / v(s + n)? - 1.0),
            Op::LogRet => Some(oxs_math::ln(v(s)? / v(s + n)?)),
            Op::Ewma { halflife } => {
                let decay = oxs_math::pow(0.5, 1.0 / halflife);
                // Weight 1 for the latest bar, `decay` times less for each bar back.
                let (mut num, mut den) = (0.0, 0.0);
                let mut weights = vec![0.0; n];
                let mut w = 1.0;
                for x in weights.iter_mut() {
                    *x = w;
                    w *= decay;
                }
                for back in (s..s + n).rev() {
                    if let Some(x) = v(back) {
                        num += weights[back - s] * x;
                        den += weights[back - s];
                    }
                }
                (den > 0.0).then(|| num / den)
            }
            Op::Vol { clip } => {
                if n < 2 {
                    return None;
                }
                let mut rets = Vec::with_capacity(n);
                let mut prev = v(s + n);
                for back in (s..s + n).rev() {
                    let c = v(back);
                    let mut r = 0.0;
                    if let (Some(c), Some(p)) = (c, prev) {
                        r = c / p - 1.0;
                        if clip.is_some_and(|x| r.abs() >= x) {
                            r = 0.0;
                        }
                    }
                    rets.push(r);
                    prev = c;
                }
                stdev(&rets)
            }
            Op::Seasonal {
                period,
                ahead,
                clip,
            } => {
                let mut xs = Vec::with_capacity(n);
                for k in (1..=n).rev() {
                    let b = s + k * period - ahead;
                    if let (Some(c), Some(p)) = (v(b), v(b + 1)) {
                        let r = c / p - 1.0;
                        if !clip.is_some_and(|x| r.abs() >= x) {
                            xs.push(r);
                        }
                    }
                }
                (!xs.is_empty()).then(|| sum(&xs) / xs.len() as f64)
            }
            _ => {
                let xs: Vec<f64> = (s..s + n).rev().filter_map(v).collect();
                if xs.is_empty() {
                    return None;
                }
                match op {
                    Op::Mean => Some(sum(&xs) / xs.len() as f64),
                    Op::Sum => Some(sum(&xs)),
                    Op::Std => stdev(&xs),
                    Op::Min => xs.iter().copied().reduce(f64::min),
                    Op::Max => xs.iter().copied().reduce(f64::max),
                    _ => unreachable!(),
                }
            }
        }
    }

    /// Market `m`'s one-bar return ending `back` bars before the latest; 0 when missing or
    /// a bad print (`clip` or more).
    fn ret_at(&self, m: usize, k: usize, back: usize, clip: Option<f64>) -> f64 {
        match (self.value(m, k, back), self.value(m, k, back + 1)) {
            (Some(c), Some(p)) => {
                let r = c / p - 1.0;
                if clip.is_some_and(|x| r.abs() >= x) {
                    0.0
                } else {
                    r
                }
            }
            _ => 0.0,
        }
    }

    fn ref_ret(&self, to: Ref, k: usize, back: usize, clip: Option<f64>, markets: &[usize]) -> f64 {
        match to {
            Ref::Market(r) => self.ret_at(r, k, back, clip),
            Ref::Basket => {
                let rs: Vec<f64> = markets
                    .iter()
                    .map(|&m| self.ret_at(m, k, back, clip))
                    .collect();
                if rs.is_empty() {
                    0.0
                } else {
                    sum(&rs) / rs.len() as f64
                }
            }
        }
    }

    /// A read against a reference: `rets[b]` is its return `shift + b` bars back.
    fn read_vs(&self, op: Op, m: usize, k: usize, n: usize, s: usize, rets: &[f64]) -> Option<f64> {
        // A market with no price at the latest bar has no beta or residual.
        self.value(m, k, s)?;
        let clip = match op {
            Op::Beta { clip, .. } | Op::Corr { clip, .. } | Op::Resid { clip, .. } => clip,
            _ => unreachable!(),
        };
        let own: Vec<f64> = (0..rets.len())
            .map(|b| self.ret_at(m, k, s + b, clip))
            .collect();
        match op {
            Op::Beta { .. } | Op::Corr { .. } => {
                // Oldest first: indices n-1 down to 0.
                let mut w = Moments::default();
                for b in (0..n).rev() {
                    w.add(own[b], rets[b]);
                }
                if matches!(op, Op::Beta { .. }) {
                    w.beta()
                } else {
                    w.corr()
                }
            }
            Op::Resid { fit, .. } => {
                // The fit window for the oldest residual, then slid one bar at a time.
                let mut w = Moments::default();
                for b in (n..n + fit).rev() {
                    w.add(own[b], rets[b]);
                }
                let mut total = 0.0;
                for b in (0..n).rev() {
                    let beta = w.beta().unwrap_or(0.0);
                    total += own[b] - beta * rets[b];
                    w.add(own[b], rets[b]);
                    w.remove(own[b + fit], rets[b + fit]);
                }
                Some(total)
            }
            _ => unreachable!(),
        }
    }
    /// The values of the last `n` bars of column `k`, oldest first, as of `shift` bars
    /// before the latest (carried as `value` does).
    pub fn window(
        &self,
        k: usize,
        n: usize,
        shift: usize,
        markets: &[usize],
    ) -> Result<Vec<Vec<Option<f64>>>, TooShort> {
        let need = shift
            + n
            + if FIELDS[self.fields[k]].1 == Agg::Last {
                self.carry_bars
            } else {
                0
            };
        if need > self.capacity {
            return Err(TooShort {
                need,
                kept: self.capacity,
            });
        }
        Ok(markets
            .iter()
            .map(|&m| {
                (shift..shift + n)
                    .rev()
                    .map(|b| self.value(m, k, b))
                    .collect()
            })
            .collect())
    }
}

/// Running sums of (x, y) pairs for a beta or correlation.
#[derive(Default)]
struct Moments {
    n: f64,
    x: f64,
    y: f64,
    xy: f64,
    yy: f64,
    xx: f64,
}

impl Moments {
    fn add(&mut self, x: f64, y: f64) {
        self.n += 1.0;
        self.x += x;
        self.y += y;
        self.xy += x * y;
        self.yy += y * y;
        self.xx += x * x;
    }

    fn remove(&mut self, x: f64, y: f64) {
        self.n -= 1.0;
        self.x -= x;
        self.y -= y;
        self.xy -= x * y;
        self.yy -= y * y;
        self.xx -= x * x;
    }

    /// cov(x, y) / var(y), from means: E[xy] - E[x]E[y] over E[y^2] - E[y]^2.
    fn beta(&self) -> Option<f64> {
        let (mx, my) = (self.x / self.n, self.y / self.n);
        let var = self.yy / self.n - my * my;
        (var > 0.0).then(|| (self.xy / self.n - mx * my) / var)
    }

    fn corr(&self) -> Option<f64> {
        let (mx, my) = (self.x / self.n, self.y / self.n);
        let vy = self.yy / self.n - my * my;
        let vx = self.xx / self.n - mx * mx;
        (vx > 0.0 && vy > 0.0).then(|| (self.xy / self.n - mx * my) / (vx * vy).sqrt())
    }
}

/// Plain left-to-right sum.
pub fn sum(xs: &[f64]) -> f64 {
    let mut s = 0.0;
    for &x in xs {
        s += x;
    }
    s
}

/// Sample standard deviation; None for fewer than two values.
pub fn stdev(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = sum(xs) / xs.len() as f64;
    let mut v = 0.0;
    for &x in xs {
        v += (x - m) * (x - m);
    }
    Some((v / (xs.len() - 1) as f64).sqrt())
}

/// Which days a calendar clock ticks on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Days {
    Daily,
    /// Monday to Friday.
    Weekdays,
    /// One weekday, 0 = Monday.
    Weekly(u32),
    /// One day of the month, 1 to 28, or counted from the end: -1 is the last day.
    Monthly(i32),
}

impl Days {
    fn has(self, d: i64) -> bool {
        match self {
            Days::Daily => true,
            Days::Weekdays => calendar::weekday(d) < 5,
            Days::Weekly(w) => calendar::weekday(d) == w,
            Days::Monthly(n) => {
                let (y, m, day) = calendar::civil_from_days(d);
                if n > 0 {
                    day == n as u32
                } else {
                    day as i64 == calendar::days_in_month(y, m) as i64 + 1 + n as i64
                }
            }
        }
    }
}

/// An exchange session's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Open,
    Close,
}

/// When a strategy runs, at minute closes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clock {
    /// Every `every_ms` from `anchor_ms`.
    Every { every_ms: i64, anchor_ms: i64 },
    /// At `at_ms` after local midnight in `zone`, on `days` (dates in that zone).
    Calendar { days: Days, at_ms: i64, zone: Zone },
    /// `offset_ms` after (or before, if negative) each open or close of an exchange's
    /// session, so only on the days it trades.
    Session {
        session: Session,
        edge: Edge,
        offset_ms: i64,
    },
}

impl Clock {
    pub fn every(every_ms: i64, anchor_ms: i64) -> Clock {
        Clock::Every {
            every_ms,
            anchor_ms,
        }
    }

    /// The clock's ticks on local date `d` of its zone, in order.
    fn ticks_on(&self, d: i64) -> Vec<i64> {
        match *self {
            Clock::Every { .. } => unreachable!(),
            Clock::Calendar { days, at_ms, zone } => {
                if days.has(d) {
                    vec![zone.instant(d * DAY_MS + at_ms)]
                } else {
                    vec![]
                }
            }
            Clock::Session {
                session,
                edge,
                offset_ms,
            } => session
                .spans(d)
                .into_iter()
                .map(|(a, b)| {
                    (match edge {
                        Edge::Open => a,
                        Edge::Close => b,
                    }) + offset_ms
                })
                .collect(),
        }
    }

    fn zone(&self) -> Zone {
        match *self {
            Clock::Every { .. } => Zone::Utc,
            Clock::Calendar { zone, .. } => zone,
            Clock::Session { session, .. } => session.zone(),
        }
    }

    /// Local dates whose ticks can fall at `t` (an offset can move a tick across midnight).
    fn days_near(&self, t: i64) -> std::ops::RangeInclusive<i64> {
        let d = calendar::Local::at(t, self.zone()).days;
        d - 1..=d + 1
    }

    pub fn is_tick(&self, t: i64) -> bool {
        match *self {
            Clock::Every {
                every_ms,
                anchor_ms,
            } => (t - anchor_ms).rem_euclid(every_ms.max(1)) == 0,
            _ => {
                // Only minute closes can tick; most minutes are rejected cheaply here.
                if self.minute_of_day_misses(t) {
                    return false;
                }
                self.days_near(t).any(|d| self.ticks_on(d).contains(&t))
            }
        }
    }

    /// True when `t` can't be a calendar clock's tick: every zone here is a whole number of
    /// hours from UTC, so a tick's minute within the hour is `at`'s. Most minutes stop here.
    fn minute_of_day_misses(&self, t: i64) -> bool {
        let at = match *self {
            Clock::Calendar { at_ms, .. } => at_ms,
            _ => return false,
        };
        (t - at).rem_euclid(HOUR_MS) != 0
    }

    /// The latest tick at or before `t` (within 70 days for a calendar or session clock).
    pub fn last_tick(&self, t: i64) -> Option<i64> {
        match *self {
            Clock::Every {
                every_ms,
                anchor_ms,
            } => Some(t - (t - anchor_ms).rem_euclid(every_ms.max(1))),
            _ => {
                let d = calendar::Local::at(t, self.zone()).days;
                (d - 70..=d + 1)
                    .rev()
                    .flat_map(|d| self.ticks_on(d).into_iter().rev())
                    .find(|&x| x <= t)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    fn row(ts: i64, close: Option<f64>, qv: f64) -> Row {
        Row {
            ts,
            open: close,
            high: close,
            low: close,
            close,
            quote_volume: Some(qv),
            volume: Some(qv / close.unwrap_or(1.0)),
            ..Row::default()
        }
    }

    #[test]
    fn bars_close_on_the_boundary_and_aggregate() {
        let f: Vec<usize> = ["open", "high", "low", "close", "quote_volume", "vwap"]
            .iter()
            .map(|n| field_id(n).unwrap())
            .collect();
        let mut h = History::new(H, f, 1, 10, 6);
        for (i, c) in [10.0, 12.0, 9.0].iter().enumerate() {
            h.step(
                i as i64 * MINUTE_MS,
                &[Some(&row(i as i64 * MINUTE_MS, Some(*c), 1.0))],
            );
        }
        assert_eq!(h.closed, 0, "the hour isn't over");
        h.step(H - MINUTE_MS, &[Some(&row(H - MINUTE_MS, Some(11.0), 1.0))]);
        assert_eq!(h.closed, 1);
        assert_eq!(h.now(), Some(H));
        let all = [0];
        let get = |k| h.read(Op::Last, k, 1, 0, &all).unwrap()[0];
        assert_eq!(get(0), Some(10.0));
        assert_eq!(get(1), Some(12.0));
        assert_eq!(get(2), Some(9.0));
        assert_eq!(get(3), Some(11.0));
        assert_eq!(get(4), Some(4.0));
    }

    #[test]
    fn empty_bars_keep_the_grid_and_prices_carry() {
        let f = vec![
            field_id("close").unwrap(),
            field_id("quote_volume").unwrap(),
        ];
        let mut h = History::new(H, f, 1, 50, 2);
        h.step(0, &[Some(&row(0, Some(100.0), 5.0))]);
        // Nothing for hours 1 to 4; a row in hour 5.
        h.step(5 * H, &[Some(&row(5 * H, Some(110.0), 5.0))]);
        assert_eq!(h.closed, 5, "hour 0 and four empty hours");
        h.step(6 * H - MINUTE_MS, &[None]);
        assert_eq!(h.closed, 6);
        let m = [0];
        // Close carried 2 bars: hours 1 and 2 see 100, hours 3 and 4 nothing.
        let at = |back| h.read(Op::Ago, 0, back, 0, &m).unwrap()[0];
        assert_eq!(at(0), Some(110.0));
        assert_eq!(at(1), None);
        assert_eq!(at(2), None);
        assert_eq!(at(3), Some(100.0));
        assert_eq!(at(5), Some(100.0));
        // Volume never carries.
        assert_eq!(h.read(Op::Ago, 1, 3, 0, &m).unwrap()[0], None);
        assert_eq!(h.read(Op::Sum, 1, 6, 0, &m).unwrap()[0], Some(10.0));
        assert_eq!(
            h.read(Op::Ret, 0, 5, 0, &m).unwrap()[0],
            Some(110.0 / 100.0 - 1.0)
        );
        // Not enough bars yet: None, not an error.
        assert_eq!(h.read(Op::Ret, 0, 6, 0, &m).unwrap()[0], None);
        // More than kept: an error.
        assert_eq!(
            h.read(Op::Ret, 0, 48, 0, &m),
            Err(TooShort { need: 51, kept: 50 })
        );
    }

    #[test]
    fn vol_matches_a_hand_computation() {
        let f = vec![field_id("close").unwrap()];
        let mut h = History::new(H, f, 1, 20, 0);
        let closes = [100.0, 101.0, 99.0, 250.0, 102.0, 103.0];
        for (i, c) in closes.iter().enumerate() {
            let ts = i as i64 * H + H - MINUTE_MS;
            h.step(ts, &[Some(&row(ts, Some(*c), 1.0))]);
        }
        let got = h.read(Op::Vol { clip: Some(0.5) }, 0, 5, 0, &[0]).unwrap()[0].unwrap();
        // 99 -> 250 and 250 -> 102 are bad prints.
        let rets = [
            101.0 / 100.0 - 1.0,
            99.0 / 101.0 - 1.0,
            0.0,
            0.0,
            103.0 / 102.0 - 1.0,
        ];
        assert_eq!(got, stdev(&rets).unwrap());
        // shift 1: the window ends a bar earlier.
        let got = h.read(Op::Vol { clip: Some(0.5) }, 0, 4, 1, &[0]).unwrap()[0].unwrap();
        assert_eq!(got, stdev(&rets[..4]).unwrap());
    }

    #[test]
    fn seasonal_is_the_same_hours_mean_return() {
        let f = vec![field_id("close").unwrap()];
        // Four-bar "days"; the close rises 1% in the bar at phase 2 of each day, else flat.
        let mut h = History::new(H, f, 1, 40, 0);
        let mut c = 100.0;
        for i in 0..14 {
            if i % 4 == 2 {
                c *= 1.01;
            }
            let ts = i as i64 * H + H - MINUTE_MS;
            h.step(ts, &[Some(&row(ts, Some(c), 1.0))]);
        }
        // The latest bar is 13 (phase 1); the coming one is phase 2.
        let read = |ahead| {
            let op = Op::Seasonal {
                period: 4,
                ahead,
                clip: Some(0.5),
            };
            h.read(op, 0, 3, 0, &[0]).unwrap()[0]
        };
        assert!((read(1).unwrap() - 0.01).abs() < 1e-12);
        assert_eq!(read(0), Some(0.0));
        assert_eq!(
            Op::Seasonal {
                period: 4,
                ahead: 1,
                clip: None
            }
            .bars(3),
            13
        );
    }

    fn closes(h: &mut History, series: &[Vec<f64>]) {
        for i in 0..series[0].len() {
            let ts = i as i64 * H + H - MINUTE_MS;
            let rs: Vec<Row> = series.iter().map(|c| row(ts, Some(c[i]), 1.0)).collect();
            let refs: Vec<Option<&Row>> = rs.iter().map(Some).collect();
            h.step(ts, &refs);
        }
    }

    #[test]
    fn log_returns_ewma_beta_and_residuals() {
        let f = vec![field_id("close").unwrap()];
        // The reference wiggles; market 1 moves twice as much plus a steady 0.1% an hour.
        let (mut a, mut b) = (vec![100.0], vec![50.0]);
        for i in 1..120 {
            let r = 0.01 * ((i * 7 % 11) as f64 - 5.0) / 5.0;
            a.push(a[i - 1] * (1.0 + r));
            b.push(b[i - 1] * (1.0 + 2.0 * r + 0.001));
        }
        let mut h = History::new(H, f, 2, 120, 0);
        closes(&mut h, &[a.clone(), b.clone()]);
        let both = [0, 1];
        let lr = h.read(Op::LogRet, 0, 10, 0, &both).unwrap();
        assert_eq!(lr[0], Some(oxs_math::ln(a[119] / a[109])));
        let to = Ref::Market(0);
        let beta = h
            .read(Op::Beta { to, clip: None }, 0, 48, 0, &both)
            .unwrap();
        assert!((beta[0].unwrap() - 1.0).abs() < 1e-9);
        assert!((beta[1].unwrap() - 2.0).abs() < 1e-6, "{beta:?}");
        let corr = h
            .read(Op::Corr { to, clip: None }, 0, 48, 0, &both)
            .unwrap();
        assert!((corr[1].unwrap() - 1.0).abs() < 1e-6);
        // Each residual is the steady 0.1%: 24 of them.
        let op = Op::Resid {
            to,
            fit: 48,
            clip: None,
        };
        let res = h.read(op, 0, 24, 0, &both).unwrap();
        assert!(res[0].unwrap().abs() < 1e-12);
        assert!((res[1].unwrap() - 0.024).abs() < 1e-6, "{res:?}");
        // Against the basket of both, market 0 moves (1 + 2) / 2 = 1.5 times less.
        let basket = h
            .read(
                Op::Beta {
                    to: Ref::Basket,
                    clip: None,
                },
                0,
                48,
                0,
                &both,
            )
            .unwrap();
        assert!((basket[0].unwrap() - 1.0 / 1.5).abs() < 1e-3, "{basket:?}");
        // EWMA with a 1-bar half-life over 3 bars: weights 1/4, 1/2, 1, oldest first.
        let e = h.read(Op::Ewma { halflife: 1.0 }, 0, 3, 0, &[0]).unwrap()[0].unwrap();
        let want = (0.25 * a[117] + 0.5 * a[118] + a[119]) / 1.75;
        assert!((e - want).abs() < 1e-12);
        assert_eq!(op.bars(24), 73);
    }

    #[test]
    fn repair_drops_a_bad_print_and_carries_the_close() {
        let f: Vec<usize> = ["open", "high", "low", "close"]
            .iter()
            .map(|n| field_id(n).unwrap())
            .collect();
        let mut h = History::new(H, f, 1, 10, 3);
        h.set_repair(2.5).unwrap();
        let mut c: Vec<f64> = (0..20).map(|i| 100.0 + i as f64).collect();
        c[15] = 1000.0;
        closes(&mut h, &[c]);
        let at = |k, back| h.read(Op::Ago, k, back, 0, &[0]).unwrap()[0];
        // Bar 15 is 4 bars back: its close is gone and carries bar 14's.
        assert_eq!(at(3, 4), Some(114.0));
        assert_eq!(at(1, 4), None, "high gone with nothing left to rebuild it");
        assert_eq!(at(3, 3), Some(116.0));
        assert!(h.kept() > REPAIR_WINDOW);
    }

    #[test]
    fn clock_ticks_from_its_anchor() {
        let c = Clock::every(168 * H, H);
        assert!(c.is_tick(H));
        assert!(c.is_tick(169 * H));
        assert!(!c.is_tick(168 * H));
        assert_eq!(c.last_tick(200 * H), Some(169 * H));
    }

    #[test]
    fn calendar_and_session_clocks() {
        use crate::calendar::days_from_civil;
        let t =
            |y, m, d, h: i64, mi: i64| days_from_civil(y, m, d) * DAY_MS + h * H + mi * MINUTE_MS;
        // Mondays at 08:00 New York time: 12:00 UTC in summer, 13:00 in winter.
        let weekly = Clock::Calendar {
            days: Days::Weekly(0),
            at_ms: 8 * H,
            zone: Zone::NewYork,
        };
        assert!(weekly.is_tick(t(2026, 10, 5, 12, 0)));
        assert!(!weekly.is_tick(t(2026, 10, 5, 13, 0)));
        assert!(weekly.is_tick(t(2026, 11, 2, 13, 0)));
        assert_eq!(
            weekly.last_tick(t(2026, 10, 7, 0, 0)),
            Some(t(2026, 10, 5, 12, 0))
        );
        // The last day of each month at 00:00 UTC.
        let monthly = Clock::Calendar {
            days: Days::Monthly(-1),
            at_ms: 0,
            zone: Zone::Utc,
        };
        assert!(monthly.is_tick(t(2026, 2, 28, 0, 0)));
        assert!(!monthly.is_tick(t(2026, 3, 28, 0, 0)));
        assert_eq!(
            monthly.last_tick(t(2026, 10, 7, 0, 0)),
            Some(t(2026, 9, 30, 0, 0))
        );
        // 15 minutes before the NYSE close, on trading days only.
        let close = Clock::Session {
            session: Session::Us,
            edge: Edge::Close,
            offset_ms: -15 * MINUTE_MS,
        };
        assert!(close.is_tick(t(2026, 10, 7, 19, 45)));
        assert!(
            close.is_tick(t(2026, 11, 27, 17, 45)),
            "the 13:00 early close"
        );
        assert!(!close.is_tick(t(2026, 11, 26, 20, 45)), "Thanksgiving");
        let ticks = (0..7 * 1440)
            .map(|i| t(2026, 10, 5, 0, 0) + i * MINUTE_MS)
            .filter(|&x| close.is_tick(x))
            .count();
        assert_eq!(ticks, 5);
    }
}
