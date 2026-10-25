//! The engine: a stepper that takes one closed 1-minute row at a time and fills the
//! orders resting from before it, under the `bar-v2` model.
//!
//! Hosts (the Rust `run` loop, the Python package, a WebAssembly build) all drive it the same
//! way: `step(row)`, then let the strategy see the row and submit orders, repeat.
//! An order submitted after row `t` is first eligible on row `t + 1`, because row
//! `t` is only known at its close.
//!
//! `bar-v2`, conservative wherever a bar hides the order of events:
//! - Within a bar, orders are tried in the order market, stop, limit, so a stop
//!   below and a target above that a bar both reaches count as the stop first.
//! - Market orders fill at the open plus slippage, as taker.
//! - Stops trigger when the bar trades at or through the trigger and fill at the
//!   worse of the open and the trigger, plus slippage, as taker.
//! - Limits fill only when price trades *through* them, never on touch. A limit that
//!   is marketable on arrival fills at the open as taker, no worse than its price.
//!   A resting limit fills at its own price as maker.
//! - Slippage comes from the previous row's book depth (known when the order was
//!   sent), or a fixed fallback when depth is null.
//! - An order that would raise exposure past `max_leverage` is cut to fit; a
//!   reduce-only order never flips the position. Whatever can't fill is cancelled.
//! - In a portfolio, a market order the leverage cap would cut waits until every other
//!   market's orders of the minute have filled, so a rebalance's sells free margin for its
//!   buys whatever the markets' order (new in bar-v2; bar-v1 cut it straight away). It
//!   fills at its own bar's price either way.
//! - After fills and funding, a position whose equity at the bar's adverse extreme
//!   is below maintenance margin is liquidated at that extreme. Losses past zero
//!   equity are recorded as bad debt, and equity is floored at zero.
//!
//! Portfolios: one account trades several markets (`Engine::with_markets`) with shared
//! cash and cross margin. Each step takes the rows of one minute, at most one per market;
//! markets are processed in their index order. The leverage cap counts the margin the other
//! positions use at their last price. Liquidation compares the whole account, every
//! position at its own bar's adverse extreme (the last price for a market with no row
//! that minute), with the sum of maintenance margins, and closes every position. A
//! one-market engine gives exactly the results, and the hash, it gave before portfolios.

pub mod bars;
pub mod calendar;
pub mod exits;
pub mod markets;
pub mod receipt;
pub mod reference;
pub mod stats;
pub mod universe;

use oxs_schema::Row;

pub const ENGINE_VERSION: &str = "0.3.1";
pub const FILL_MODEL: &str = "bar-v2";

pub type OrderId = u64;
/// A market's index in the engine, in the order given to `Engine::with_markets`.
pub type MarketId = usize;

/// One market's limits.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketSpec {
    pub name: String,
    pub max_leverage: f64,
    /// Maintenance margin as a fraction of notional.
    pub maintenance_margin: f64,
}

impl MarketSpec {
    /// Hyperliquid's rule: maintenance margin is half the initial margin at max leverage.
    pub fn with_leverage(name: &str, max_leverage: f64) -> Self {
        MarketSpec {
            name: name.into(),
            max_leverage,
            maintenance_margin: 0.5 / max_leverage,
        }
    }

    /// The config's own limits, under `name`.
    pub fn from_config(name: &str, cfg: &Config) -> Self {
        MarketSpec {
            name: name.into(),
            max_leverage: cfg.max_leverage,
            maintenance_margin: cfg.maintenance_margin,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderType {
    Market,
    Limit {
        price: f64,
    },
    /// Stop-market: becomes a market order once price reaches `trigger`.
    Stop {
        trigger: f64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrderRequest {
    /// Signed base size: positive buys, negative sells.
    pub size: f64,
    pub kind: OrderType,
    pub reduce_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Order {
    pub id: OrderId,
    pub market: MarketId,
    pub size: f64,
    pub kind: OrderType,
    pub reduce_only: bool,
    /// False once the order has been through one bar; decides taker-on-arrival
    /// versus resting-maker for limits.
    fresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    ZeroSize,
    BadPrice,
    UnknownOrder,
    UnknownMarket,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    ByStrategy,
    NothingToReduce,
    MarginLimit,
    Liquidated,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cancel {
    pub ts: i64,
    pub order_id: OrderId,
    pub reason: CancelReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liquidity {
    Maker,
    Taker,
    Liquidation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fill {
    pub ts: i64,
    pub market: MarketId,
    /// None for a liquidation.
    pub order_id: Option<OrderId>,
    pub size: f64,
    pub price: f64,
    pub fee: f64,
    pub liquidity: Liquidity,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    pub starting_cash: f64,
    pub taker_fee_bps: f64,
    pub maker_fee_bps: f64,
    /// Used when the previous row has no depth columns (every `api-v0` row).
    pub fallback_slippage_bps: f64,
    /// The limits of a market given no `MarketSpec` of its own (`Engine::new`).
    pub max_leverage: f64,
    /// Maintenance margin as a fraction of notional.
    pub maintenance_margin: f64,
}

impl Default for Config {
    /// Hyperliquid's base fee tier and BTC's 40x limit.
    fn default() -> Self {
        Config {
            starting_cash: 10_000.0,
            taker_fee_bps: 4.5,
            maker_fee_bps: 1.5,
            fallback_slippage_bps: 2.0,
            max_leverage: 40.0,
            maintenance_margin: 0.0125,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Depth {
    half_spread_bps: f64,
    bid_10: f64,
    ask_10: f64,
    bid_50: f64,
    ask_50: f64,
}

/// One market's state inside the account.
#[derive(Debug, Clone)]
struct Book {
    spec: MarketSpec,
    position: f64,
    last_price: Option<f64>,
    prev_depth: Option<Depth>,
    /// Funding this market's position has paid (negative: received).
    funding: f64,
}

#[derive(Debug, Clone)]
pub struct Engine {
    cfg: Config,
    cash: f64,
    books: Vec<Book>,
    orders: Vec<Order>,
    next_id: OrderId,
    fills: Vec<Fill>,
    cancels: Vec<Cancel>,
    funding_paid: f64,
    liquidations: u32,
    bad_debt: f64,
    equity_curve: Vec<f64>,
    ts: i64,
}

impl Engine {
    /// One market with the config's limits.
    pub fn new(cfg: Config) -> Self {
        Self::with_markets(cfg, vec![MarketSpec::from_config("default", &cfg)])
    }

    /// A portfolio: one account across these markets.
    pub fn with_markets(cfg: Config, markets: Vec<MarketSpec>) -> Self {
        assert!(!markets.is_empty(), "an engine needs at least one market");
        Engine {
            cfg,
            cash: cfg.starting_cash,
            books: markets
                .into_iter()
                .map(|spec| Book {
                    spec,
                    position: 0.0,
                    last_price: None,
                    prev_depth: None,
                    funding: 0.0,
                })
                .collect(),
            orders: Vec::new(),
            next_id: 1,
            fills: Vec::new(),
            cancels: Vec::new(),
            funding_paid: 0.0,
            liquidations: 0,
            bad_debt: 0.0,
            equity_curve: Vec::new(),
            ts: i64::MIN,
        }
    }

    pub fn markets(&self) -> Vec<&MarketSpec> {
        self.books.iter().map(|b| &b.spec).collect()
    }

    /// The first market's position; the only one in a one-market engine.
    pub fn position(&self) -> f64 {
        self.books[0].position
    }

    pub fn position_of(&self, market: MarketId) -> f64 {
        self.books[market].position
    }

    /// A market's last close, or None before its first.
    pub fn price_of(&self, market: MarketId) -> Option<f64> {
        self.books[market].last_price
    }

    pub fn cash(&self) -> f64 {
        self.cash
    }

    /// Cash plus every position at its last known price.
    pub fn equity(&self) -> f64 {
        let mut e = self.cash;
        for b in &self.books {
            if let Some(p) = b.last_price {
                e += b.position * p;
            }
        }
        e
    }

    pub fn open_orders(&self) -> &[Order] {
        &self.orders
    }

    /// Every fill so far, oldest first. Hosts use its length to find new fills.
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    /// Submits to the first market.
    pub fn submit(&mut self, req: OrderRequest) -> Result<OrderId, Reject> {
        self.submit_to(0, req)
    }

    pub fn submit_to(&mut self, market: MarketId, req: OrderRequest) -> Result<OrderId, Reject> {
        if market >= self.books.len() {
            return Err(Reject::UnknownMarket);
        }
        if req.size == 0.0 || !req.size.is_finite() {
            return Err(Reject::ZeroSize);
        }
        let price_ok = |p: f64| p.is_finite() && p > 0.0;
        match req.kind {
            OrderType::Limit { price } if !price_ok(price) => return Err(Reject::BadPrice),
            OrderType::Stop { trigger } if !price_ok(trigger) => return Err(Reject::BadPrice),
            _ => {}
        }
        let id = self.next_id;
        self.next_id += 1;
        self.orders.push(Order {
            id,
            market,
            size: req.size,
            kind: req.kind,
            reduce_only: req.reduce_only,
            fresh: true,
        });
        Ok(id)
    }

    pub fn cancel(&mut self, id: OrderId) -> Result<(), Reject> {
        let i = self
            .orders
            .iter()
            .position(|o| o.id == id)
            .ok_or(Reject::UnknownOrder)?;
        self.orders.remove(i);
        self.cancels.push(Cancel {
            ts: self.ts,
            order_id: id,
            reason: CancelReason::ByStrategy,
        });
        Ok(())
    }

    pub fn cancel_all(&mut self) {
        for o in std::mem::take(&mut self.orders) {
            self.cancels.push(Cancel {
                ts: self.ts,
                order_id: o.id,
                reason: CancelReason::ByStrategy,
            });
        }
    }

    /// Slippage for a taker fill of `notional`, from the previous row's depth: half
    /// the spread, then a linear walk through the 10 bps and 50 bps bands.
    fn slippage_bps(&self, market: MarketId, notional: f64, buy: bool) -> f64 {
        let Some(d) = self.books[market].prev_depth else {
            return self.cfg.fallback_slippage_bps;
        };
        let (d10, d50) = if buy {
            (d.ask_10, d.ask_50)
        } else {
            (d.bid_10, d.bid_50)
        };
        if d10 <= 0.0 {
            return self.cfg.fallback_slippage_bps;
        }
        let walk = if notional <= d10 {
            10.0 * notional / d10
        } else if notional <= d50 && d50 > d10 {
            10.0 + 40.0 * (notional - d10) / (d50 - d10)
        } else {
            50.0 + 50.0 * (notional - d50.max(d10)) / d50.max(d10)
        };
        d.half_spread_bps + walk
    }

    /// The position a fill of `size` at `price` leaves, cut by the leverage cap when it
    /// raises exposure: the cap counts the margin the account's other positions use. Only
    /// a portfolio has those; one market computes exactly as before.
    fn cap_target(&self, m: MarketId, pos: f64, size: f64, price: f64) -> f64 {
        let mut target = pos + size;
        if target.abs() > pos.abs() {
            let mut free = self.cash + pos * price;
            if self.books.len() > 1 {
                for (j, b) in self.books.iter().enumerate() {
                    if j != m && b.position != 0.0 {
                        let p = b.last_price.unwrap_or(0.0);
                        free += b.position * p - b.position.abs() * p / b.spec.max_leverage;
                    }
                }
            }
            let lev = self.books[m].spec.max_leverage;
            let allowed = (free.max(0.0) * lev / price).max(0.0);
            if target.abs() > allowed {
                let capped = if target.signum() == pos.signum() {
                    allowed.max(pos.abs())
                } else {
                    allowed
                };
                target = target.signum() * capped.min(target.abs());
            }
        }
        target
    }

    /// The taker price and fee of a fill of `size` at `base_price` now.
    fn taker_price(&self, m: MarketId, size: f64, base_price: f64) -> f64 {
        let slip = self.slippage_bps(m, size.abs() * base_price, size > 0.0) / 10_000.0;
        if size > 0.0 {
            base_price * (1.0 + slip)
        } else {
            base_price * (1.0 - slip)
        }
    }

    /// Whether a market order of `size` filling at `base_price` would be cut by the
    /// leverage cap.
    fn would_cap(&self, ord: &Order, base_price: f64) -> bool {
        let pos = self.books[ord.market].position;
        if ord.reduce_only || (pos + ord.size).abs() <= pos.abs() {
            return false;
        }
        let price = self.taker_price(ord.market, ord.size, base_price);
        self.cap_target(ord.market, pos, ord.size, price) != pos + ord.size
    }

    /// What a market order of `size` would cost now, in quote currency: fees and slippage
    /// against the last price, from the previous row's depth. None before the market's
    /// first price.
    pub fn estimate_cost(&self, market: MarketId, size: f64) -> Option<f64> {
        let px = self.books[market].last_price?;
        let price = self.taker_price(market, size, px);
        let fee = size.abs() * price * self.cfg.taker_fee_bps / 10_000.0;
        Some(size.abs() * (price - px).abs() + fee)
    }

    /// Applies one fill, clipped by reduce-only and the leverage cap. Returns the size
    /// actually filled (0 if nothing could be).
    fn execute(
        &mut self,
        order: &Order,
        base_price: f64,
        liquidity: Liquidity,
        cap_price: Option<f64>,
    ) -> f64 {
        let m = order.market;
        let pos = self.books[m].position;
        let mut size = order.size;
        if order.reduce_only {
            if pos == 0.0 || size.signum() == pos.signum() {
                return 0.0;
            }
            size = size.signum() * size.abs().min(pos.abs());
        }
        let buy = size > 0.0;
        let price = if liquidity == Liquidity::Taker {
            let p = self.taker_price(m, size, base_price);
            match cap_price {
                Some(c) if buy => p.min(c),
                Some(c) => p.max(c),
                None => p,
            }
        } else {
            base_price
        };
        let target = self.cap_target(m, pos, size, price);
        let size = target - pos;
        if size == 0.0 || size.signum() != order.size.signum() {
            return 0.0;
        }
        let fee_bps = match liquidity {
            Liquidity::Maker => self.cfg.maker_fee_bps,
            _ => self.cfg.taker_fee_bps,
        };
        let fee = size.abs() * price * fee_bps / 10_000.0;
        self.cash -= size * price + fee;
        self.books[m].position = target;
        self.fills.push(Fill {
            ts: self.ts,
            market: m,
            order_id: Some(order.id),
            size,
            price,
            fee,
            liquidity,
        });
        size
    }

    /// Process one closed row of a one-market engine: fill resting orders, settle
    /// funding, check liquidation.
    pub fn step(&mut self, row: &Row) {
        self.step_markets(&[Some(row)]);
    }

    /// Process one minute of a portfolio: `rows[m]` is market `m`'s row, or None if it has
    /// none this minute. Every row given must have the same `ts`.
    pub fn step_markets(&mut self, rows: &[Option<&Row>]) {
        assert_eq!(rows.len(), self.books.len(), "one slot per market");
        let ts = rows
            .iter()
            .flatten()
            .map(|r| r.ts)
            .next()
            .expect("at least one row");
        assert!(
            rows.iter().flatten().all(|r| r.ts == ts),
            "rows of one step share a ts"
        );
        self.ts = ts;
        let mut deferred = Vec::new();
        for (m, row) in rows.iter().enumerate() {
            if let Some(row) = row {
                self.fill_market(m, row, &mut deferred);
            }
        }
        // Market orders the cap would have cut, now that the minute's other orders have
        // filled.
        for (ord, open) in deferred {
            let filled = self.execute(&ord, open, Liquidity::Taker, None);
            if filled.abs() < ord.size.abs() {
                self.cancels.push(Cancel {
                    ts,
                    order_id: ord.id,
                    reason: CancelReason::MarginLimit,
                });
            }
        }

        // Funding settled in this bucket. Longs pay a positive rate.
        for (m, row) in rows.iter().enumerate() {
            let Some(row) = row else { continue };
            if let (Some(rate), Some(px)) = (row.funding_settled, row.mark_price.or(row.close)) {
                let payment = self.books[m].position * px * rate;
                self.cash -= payment;
                self.funding_paid += payment;
                self.books[m].funding += payment;
            }
        }

        // Liquidation at each bar's adverse extreme, checked when a market with a position
        // has a bar this minute.
        let adverse = |b: &Book, row: Option<&Row>| -> Option<f64> {
            row.and_then(|r| r.high.zip(r.low))
                .map(|(h, l)| if b.position > 0.0 { l } else { h })
        };
        let any = self
            .books
            .iter()
            .zip(rows)
            .any(|(b, r)| b.position != 0.0 && adverse(b, *r).is_some());
        if any {
            let mut equity = self.cash;
            let mut maintenance = 0.0;
            let mut prices = vec![None; self.books.len()];
            for (m, b) in self.books.iter().enumerate() {
                if b.position == 0.0 {
                    continue;
                }
                let Some(px) = adverse(b, rows[m]).or(b.last_price) else {
                    continue;
                };
                equity += b.position * px;
                maintenance += b.spec.maintenance_margin * b.position.abs() * px;
                prices[m] = Some(px);
            }
            if equity < maintenance {
                self.liquidate(&prices);
            }
        }

        for (m, row) in rows.iter().enumerate() {
            let Some(row) = row else { continue };
            let b = &mut self.books[m];
            if row.close.is_some() {
                b.last_price = row.close;
            }
            if let (Some(b10), Some(a10), Some(b50), Some(a50), Some(sp)) = (
                row.depth_bid_10bps,
                row.depth_ask_10bps,
                row.depth_bid_50bps,
                row.depth_ask_50bps,
                row.spread_bps,
            ) {
                b.prev_depth = Some(Depth {
                    half_spread_bps: sp / 2.0,
                    bid_10: b10,
                    ask_10: a10,
                    bid_50: b50,
                    ask_50: a50,
                });
            } else {
                b.prev_depth = None;
            }
        }
        self.equity_curve.push(self.equity());
    }

    /// Fills market `m`'s resting orders against its bar. In a portfolio, a market order
    /// the leverage cap would cut goes to `deferred` with its bar's open instead.
    fn fill_market(&mut self, m: MarketId, row: &Row, deferred: &mut Vec<(Order, f64)>) {
        if let (Some(o), Some(h), Some(l)) = (row.open, row.high, row.low) {
            for pass in 0..3 {
                let mut i = 0;
                while i < self.orders.len() {
                    let ord = self.orders[i];
                    if ord.market != m {
                        i += 1;
                        continue;
                    }
                    let buy = ord.size > 0.0;
                    // (base price, liquidity, cap) if this order fills on this bar.
                    let hit = match (pass, ord.kind) {
                        (0, OrderType::Market) => Some((o, Liquidity::Taker, None)),
                        (1, OrderType::Stop { trigger: t }) => {
                            if buy && h >= t {
                                Some((o.max(t), Liquidity::Taker, None))
                            } else if !buy && l <= t {
                                Some((o.min(t), Liquidity::Taker, None))
                            } else {
                                None
                            }
                        }
                        (2, OrderType::Limit { price: p }) => {
                            let marketable = if buy { o <= p } else { o >= p };
                            if ord.fresh && marketable {
                                Some((o, Liquidity::Taker, Some(p)))
                            } else if (buy && l < p) || (!buy && h > p) {
                                Some((p, Liquidity::Maker, None))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    if pass == 0 && hit.is_some() && self.books.len() > 1 && self.would_cap(&ord, o)
                    {
                        self.orders.remove(i);
                        deferred.push((ord, o));
                        continue;
                    }
                    match hit {
                        Some((base, liq, cap)) => {
                            let filled = self.execute(&ord, base, liq, cap);
                            self.orders.remove(i);
                            if filled.abs() < ord.size.abs() {
                                let reason = if ord.reduce_only && filled == 0.0 {
                                    CancelReason::NothingToReduce
                                } else {
                                    CancelReason::MarginLimit
                                };
                                self.cancels.push(Cancel {
                                    ts: row.ts,
                                    order_id: ord.id,
                                    reason,
                                });
                            }
                        }
                        None => i += 1,
                    }
                }
            }
            for ord in &mut self.orders {
                if ord.market == m {
                    ord.fresh = false;
                }
            }
        }
    }

    /// Closes every position with a price in `prices`, then cancels every order.
    fn liquidate(&mut self, prices: &[Option<f64>]) {
        for (m, px) in prices.iter().enumerate() {
            let Some(px) = *px else { continue };
            let size = -self.books[m].position;
            let buy = size > 0.0;
            let slip = self.slippage_bps(m, size.abs() * px, buy) / 10_000.0;
            let price = if buy {
                px * (1.0 + slip)
            } else {
                px * (1.0 - slip)
            };
            let fee = size.abs() * price * self.cfg.taker_fee_bps / 10_000.0;
            self.cash -= size * price + fee;
            self.books[m].position = 0.0;
            self.fills.push(Fill {
                ts: self.ts,
                market: m,
                order_id: None,
                size,
                price,
                fee,
                liquidity: Liquidity::Liquidation,
            });
            self.liquidations += 1;
        }
        if self.cash < 0.0 {
            self.bad_debt += -self.cash;
            self.cash = 0.0;
        }
        for o in std::mem::take(&mut self.orders) {
            self.cancels.push(Cancel {
                ts: self.ts,
                order_id: o.id,
                reason: CancelReason::Liquidated,
            });
        }
    }

    pub fn finish(self) -> Report {
        let final_equity = self
            .books
            .iter()
            .any(|b| b.last_price.is_some())
            .then(|| self.equity());
        // A one-market engine hashes exactly what it did before portfolios existed.
        let portfolio = self.books.len() > 1;
        let mut h = Fnv::new();
        for f in &self.fills {
            h.i64(f.ts);
            if portfolio {
                h.i64(f.market as i64);
            }
            h.f64(f.size);
            h.f64(f.price);
            h.f64(f.fee);
        }
        for c in &self.cancels {
            h.i64(c.ts);
            h.i64(c.order_id as i64);
        }
        h.f64(self.funding_paid);
        h.f64(self.bad_debt);
        h.f64(self.cash);
        for b in &self.books {
            h.f64(b.position);
        }
        Report {
            result_hash: h.0,
            markets: self.books.iter().map(|b| b.spec.clone()).collect(),
            final_positions: self.books.iter().map(|b| b.position).collect(),
            final_prices: self.books.iter().map(|b| b.last_price).collect(),
            funding_by_market: self.books.iter().map(|b| b.funding).collect(),
            fills: self.fills,
            cancels: self.cancels,
            funding_paid: self.funding_paid,
            liquidations: self.liquidations,
            bad_debt: self.bad_debt,
            final_cash: self.cash,
            final_equity,
            equity_curve: self.equity_curve,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub markets: Vec<MarketSpec>,
    pub fills: Vec<Fill>,
    pub cancels: Vec<Cancel>,
    pub funding_paid: f64,
    pub liquidations: u32,
    /// Losses past zero equity, absorbed by the venue rather than the account.
    pub bad_debt: f64,
    pub final_cash: f64,
    /// One per market, in market order.
    pub final_positions: Vec<f64>,
    /// Each market's last close, in market order.
    pub final_prices: Vec<Option<f64>>,
    /// Funding each market paid, in market order.
    pub funding_by_market: Vec<f64>,
    /// Equity at the last closes, or None if no row had a close.
    pub final_equity: Option<f64>,
    /// Equity at each step's closes, one value per step.
    pub equity_curve: Vec<f64>,
    /// FNV-1a over every fill, cancel and the final state, as raw float bits. A portfolio
    /// also hashes each fill's market.
    pub result_hash: u64,
}

/// What a strategy sees and may do on each bar.
pub struct Ctx<'a> {
    engine: &'a mut Engine,
}

impl Ctx<'_> {
    pub fn position(&self) -> f64 {
        self.engine.position()
    }

    pub fn position_of(&self, market: MarketId) -> f64 {
        self.engine.position_of(market)
    }

    pub fn cash(&self) -> f64 {
        self.engine.cash()
    }

    pub fn equity(&self) -> f64 {
        self.engine.equity()
    }

    pub fn submit_to(&mut self, market: MarketId, req: OrderRequest) -> Result<OrderId, Reject> {
        self.engine.submit_to(market, req)
    }

    pub fn open_orders(&self) -> &[Order] {
        self.engine.open_orders()
    }

    pub fn submit(&mut self, req: OrderRequest) -> Result<OrderId, Reject> {
        self.engine.submit(req)
    }

    pub fn market(&mut self, size: f64) -> Result<OrderId, Reject> {
        self.submit(OrderRequest {
            size,
            kind: OrderType::Market,
            reduce_only: false,
        })
    }

    pub fn limit(&mut self, size: f64, price: f64) -> Result<OrderId, Reject> {
        self.submit(OrderRequest {
            size,
            kind: OrderType::Limit { price },
            reduce_only: false,
        })
    }

    pub fn stop(&mut self, size: f64, trigger: f64) -> Result<OrderId, Reject> {
        self.submit(OrderRequest {
            size,
            kind: OrderType::Stop { trigger },
            reduce_only: false,
        })
    }

    pub fn cancel(&mut self, id: OrderId) -> Result<(), Reject> {
        self.engine.cancel(id)
    }

    pub fn cancel_all(&mut self) {
        self.engine.cancel_all()
    }
}

pub trait Strategy {
    fn on_bar(&mut self, ctx: &mut Ctx, row: &Row);
}

/// A strategy over several markets: `rows[m]` is market `m`'s row this minute, if any.
pub trait PortfolioStrategy {
    fn on_bars(&mut self, ctx: &mut Ctx, rows: &[Option<&Row>]);
}

/// The Rust host: step each row, then let the strategy act on it.
pub fn run(rows: &[Row], strategy: &mut impl Strategy, cfg: Config) -> Report {
    let mut engine = Engine::new(cfg);
    for row in rows {
        engine.step(row);
        strategy.on_bar(
            &mut Ctx {
                engine: &mut engine,
            },
            row,
        );
    }
    engine.finish()
}

/// The Rust host for a portfolio: `data[m]` is market `m`'s rows, oldest first.
pub fn run_portfolio(
    data: &[&[Row]],
    markets: Vec<MarketSpec>,
    strategy: &mut impl PortfolioStrategy,
    cfg: Config,
) -> Report {
    let mut engine = Engine::with_markets(cfg, markets);
    for rows in align(data) {
        engine.step_markets(&rows);
        strategy.on_bars(
            &mut Ctx {
                engine: &mut engine,
            },
            &rows,
        );
    }
    engine.finish()
}

/// Walks several markets' rows minute by minute: each item has one slot per market, the
/// row with the earliest `ts` not yet given and every other market's row with that `ts`.
/// Each market's rows must be in increasing `ts` order.
pub fn align<'a>(data: &'a [&'a [Row]]) -> impl Iterator<Item = Vec<Option<&'a Row>>> + 'a {
    let mut next = vec![0usize; data.len()];
    std::iter::from_fn(move || {
        let ts = data
            .iter()
            .zip(&next)
            .filter_map(|(rows, &i)| rows.get(i).map(|r| r.ts))
            .min()?;
        Some(
            data.iter()
                .zip(next.iter_mut())
                .map(|(rows, i)| match rows.get(*i) {
                    Some(r) if r.ts == ts => {
                        *i += 1;
                        Some(r)
                    }
                    _ => None,
                })
                .collect(),
        )
    })
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for x in b {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn i64(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    fn f64(&mut self, v: f64) {
        self.bytes(&v.to_bits().to_le_bytes());
    }
}

#[cfg(test)]
mod tests;
