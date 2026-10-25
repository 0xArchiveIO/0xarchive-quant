//! The Python wheel's compiled half, `oxarchive_backtest._native` (`pip install
//! 0xarchive-backtest`): the engine, the run loop that drives a strategy (`Run`, behind
//! `_backtest` and `Session`), the history a strategy reads (`History`) and point-in-time
//! universes. The Python half, python/oxarchive_backtest/, holds the strategy model
//! (`Strategy`, `Series`, the pieces in `x`), data sources and the `0xbt` command;
//! references/strategy-api.md in the skill describes the whole.
//!
//! Python is called only at the strategy's clock ticks and for fills. The minute loop, the
//! bars and every rolling read stay here, so a weekly strategy over a year and a half of
//! minutes calls into Python about eighty times.

mod math;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use oxs_engine::bars::{self, BarBuilder, Clock, Days, Edge, FIELDS, History, MINUTE_MS, Op, Ref};
use oxs_engine::calendar::{self, Session as Exchange, Zone};
use oxs_engine::exits::{DrawdownStop, Exit, Exits, Reason};
use oxs_engine::receipt::{DatasetRef, Receipt, ResultRecord, market_records};
use oxs_engine::stats::{Attribution, Stats, attribution, daily_returns, stats, versus};
use oxs_engine::universe::{By, HOUR_MS, Hourly, Pick, TopN, Universe, schedule};
use oxs_engine::{
    Config, Engine, Fill, Liquidity, MarketSpec, OrderRequest, OrderType, Reject, Report,
};
use oxs_schema::Row;
use pyo3::exceptions::{PyIOError, PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{IntoPyDict, PyDict, PyString};

fn reject(r: Reject) -> PyErr {
    PyValueError::new_err(format!("order rejected: {r:?}"))
}

/// A row from a dict of column values (the live stream's JSON), refusing unknown keys so a
/// misspelled column can't silently arrive as null.
fn row_from_dict(d: &Bound<'_, PyDict>) -> PyResult<Row> {
    let ts: i64 = d
        .get_item("ts")?
        .ok_or_else(|| PyKeyError::new_err("a row needs ts (bucket open, Unix ms)"))?
        .extract()?;
    let mut r = Row {
        ts,
        ..Row::default()
    };
    for (k, v) in d.iter() {
        let k: String = k.extract()?;
        if k == "ts" || k == "market" {
            continue;
        }
        let f = bars::field_id(&k)
            .ok_or_else(|| PyKeyError::new_err(format!("unknown row column {k:?}")))?;
        bars::set(&mut r, f, v.extract()?);
    }
    Ok(r)
}

/// One market's rows, oldest first: in memory, or read from its files as a run reaches them.
#[pyclass(frozen, name = "Dataset", module = "oxarchive_backtest")]
pub struct Dataset {
    parts: Vec<Part>,
    /// The files the rows came from, for the receipt.
    files: Vec<DatasetRef>,
    /// The columns a streamed file is read with (None: all).
    columns: Option<Vec<String>>,
}

/// A month of a dataset.
enum Part {
    Rows(Vec<Row>),
    /// Decoded when a run reaches it and dropped when the run moves on, so a run holds one
    /// month per market however long it is.
    File {
        path: std::path::PathBuf,
        rows: usize,
    },
}

impl Dataset {
    fn cursor(&self) -> Cursor<'_> {
        Cursor {
            ds: self,
            part: 0,
            chunk: Chunk::Borrowed(&[]),
            i: 0,
        }
    }

    fn market(&self) -> Option<String> {
        self.files.first().map(|d| d.market.clone())
    }
}

enum Chunk<'a> {
    Borrowed(&'a [Row]),
    Owned(Vec<Row>),
}

impl Chunk<'_> {
    fn rows(&self) -> &[Row] {
        match self {
            Chunk::Borrowed(r) => r,
            Chunk::Owned(r) => r,
        }
    }
}

/// Walks a dataset's rows in order, reading each streamed file when it is reached.
struct Cursor<'a> {
    ds: &'a Dataset,
    part: usize,
    chunk: Chunk<'a>,
    i: usize,
}

impl Cursor<'_> {
    /// The next row's `ts`, reading the next part when this one is used up.
    fn peek_ts(&mut self) -> PyResult<Option<i64>> {
        while self.i >= self.chunk.rows().len() {
            let Some(part) = self.ds.parts.get(self.part) else {
                return Ok(None);
            };
            self.part += 1;
            self.i = 0;
            self.chunk = Chunk::Borrowed(&[]); // free the last month before reading the next
            self.chunk = match part {
                Part::Rows(r) => Chunk::Borrowed(r),
                Part::File { path, .. } => {
                    let io = |e: String| PyIOError::new_err(format!("{}: {e}", path.display()));
                    let cols: Option<Vec<&str>> = self
                        .ds
                        .columns
                        .as_ref()
                        .map(|c| c.iter().map(String::as_str).collect());
                    let (_, rows) = oxs_dataset::read_path(path, cols.as_deref())
                        .map_err(|e| io(e.to_string()))?;
                    Chunk::Owned(rows)
                }
            };
        }
        Ok(Some(self.chunk.rows()[self.i].ts))
    }

    fn current(&self) -> Option<&Row> {
        self.chunk.rows().get(self.i)
    }
}

/// Steps `f` over several datasets minute by minute, over [from, until): each step has one
/// slot per market, the rows with the earliest `ts` not yet given. Streamed files are read
/// a month at a time, so they never all sit in memory at once.
fn walk(
    cursors: &mut [Cursor<'_>],
    from: i64,
    until: i64,
    mut f: impl FnMut(&[Option<&Row>], i64) -> PyResult<()>,
) -> PyResult<usize> {
    let mut n = 0;
    loop {
        let mut ts: Option<i64> = None;
        for c in cursors.iter_mut() {
            while let Some(t) = c.peek_ts()? {
                if t >= from {
                    ts = Some(ts.map_or(t, |m| m.min(t)));
                    break;
                }
                c.i += 1;
            }
        }
        let Some(ts) = ts.filter(|&t| t < until) else {
            return Ok(n);
        };
        let hit: Vec<bool> = {
            let step: Vec<Option<&Row>> = cursors
                .iter()
                .map(|c| c.current().filter(|r| r.ts == ts))
                .collect();
            f(&step, ts)?;
            step.iter().map(Option::is_some).collect()
        };
        for (c, h) in cursors.iter_mut().zip(hit) {
            if h {
                c.i += 1;
            }
        }
        n += 1;
    }
}

#[pymethods]
impl Dataset {
    fn __len__(&self) -> usize {
        self.parts
            .iter()
            .map(|p| match p {
                Part::Rows(r) => r.len(),
                Part::File { rows, .. } => *rows,
            })
            .sum()
    }

    /// The files the rows came from, as (market, month, dataset_version).
    #[getter]
    fn files(&self) -> Vec<(String, String, String)> {
        self.files
            .iter()
            .map(|d| (d.market.clone(), d.month.clone(), d.dataset_version.clone()))
            .collect()
    }

    /// Every row as a dict of its known columns (and `ts`), the live stream's shape: for
    /// feeding a `Session` stored rows one at a time.
    fn rows<'py>(&self, py: Python<'py>) -> PyResult<Vec<Bound<'py, PyDict>>> {
        let mut out = Vec::new();
        walk(&mut [self.cursor()], i64::MIN, i64::MAX, |step, _| {
            let r = step[0].expect("one dataset has a row every step");
            let d = PyDict::new(py);
            d.set_item("ts", r.ts)?;
            for (f, (name, _)) in FIELDS.iter().enumerate() {
                if let Some(v) = bars::get(r, f) {
                    d.set_item(name, v)?;
                }
            }
            out.push(d);
            Ok(())
        })?;
        Ok(out)
    }

    /// The dataset's market (`hyperliquid:BTC`), or None if it has no files.
    #[getter(market)]
    fn py_market(&self) -> Option<String> {
        self.market()
    }
}

/// Reads the `.parquet` files in `dir` (months `first` to `last` of their names, inclusive,
/// when given), in name order. `columns` limits the read; unread columns arrive as None.
/// With `stream=True` only the footers are read now, and each file is read when a run
/// reaches it and dropped after.
#[pyfunction]
#[pyo3(signature = (dir, columns=None, stream=false, months=None))]
fn load(
    dir: &str,
    columns: Option<Vec<String>>,
    stream: bool,
    months: Option<(String, String)>,
) -> PyResult<Dataset> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| PyIOError::new_err(format!("{dir}: {e}")))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .filter(|p| {
            let Some((a, b)) = &months else { return true };
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            stem >= a.as_str() && stem <= b.as_str()
        })
        .collect();
    paths.sort();
    let cols: Option<Vec<&str>> = columns
        .as_ref()
        .map(|c| c.iter().map(String::as_str).collect());
    let mut parts = Vec::new();
    let mut refs = Vec::new();
    for f in paths {
        let io = |e: String| PyIOError::new_err(format!("{}: {e}", f.display()));
        let kv = if stream {
            let (kv, rows) = oxs_dataset::read_meta_path(&f).map_err(|e| io(e.to_string()))?;
            parts.push(Part::File {
                path: f.clone(),
                rows,
            });
            kv
        } else {
            let (kv, rows) =
                oxs_dataset::read_path(&f, cols.as_deref()).map_err(|e| io(e.to_string()))?;
            parts.push(Part::Rows(rows));
            kv
        };
        let meta = oxs_dataset::FileMeta::from_kv(&kv).ok_or_else(|| {
            PyIOError::new_err(format!("{} has no 0xArchive dataset metadata", f.display()))
        })?;
        if refs
            .first()
            .is_some_and(|d: &DatasetRef| d.market != meta.market)
        {
            return Err(PyIOError::new_err(format!(
                "{dir} mixes markets ({} and {}); load each market's directory separately",
                refs[0].market, meta.market
            )));
        }
        refs.push(DatasetRef {
            market: meta.market,
            month: meta.month,
            dataset_version: meta.dataset_version,
        });
    }
    Ok(Dataset {
        parts,
        files: refs,
        columns: if stream { columns } else { None },
    })
}

#[pyclass(from_py_object, name = "Config", module = "oxarchive_backtest")]
#[derive(Clone, Copy)]
pub struct PyConfig(Config);

#[pymethods]
impl PyConfig {
    #[new]
    #[pyo3(signature = (
        starting_cash=None, taker_fee_bps=None, maker_fee_bps=None, builder_fee_bps=None,
        fallback_slippage_bps=None, max_leverage=None, maintenance_margin=None
    ))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        starting_cash: Option<f64>,
        taker_fee_bps: Option<f64>,
        maker_fee_bps: Option<f64>,
        builder_fee_bps: Option<f64>,
        fallback_slippage_bps: Option<f64>,
        max_leverage: Option<f64>,
        maintenance_margin: Option<f64>,
    ) -> Self {
        let d = Config::default();
        PyConfig(Config {
            starting_cash: starting_cash.unwrap_or(d.starting_cash),
            taker_fee_bps: taker_fee_bps.unwrap_or(d.taker_fee_bps),
            maker_fee_bps: maker_fee_bps.unwrap_or(d.maker_fee_bps),
            builder_fee_bps: builder_fee_bps.unwrap_or(d.builder_fee_bps),
            fallback_slippage_bps: fallback_slippage_bps.unwrap_or(d.fallback_slippage_bps),
            max_leverage: max_leverage.unwrap_or(d.max_leverage),
            maintenance_margin: maintenance_margin.unwrap_or(d.maintenance_margin),
        })
    }

    #[getter]
    fn starting_cash(&self) -> f64 {
        self.0.starting_cash
    }
}

/// A fill, passed to `on_fill` and listed in the report.
#[pyclass(
    skip_from_py_object,
    frozen,
    get_all,
    name = "Fill",
    module = "oxarchive_backtest"
)]
#[derive(Clone)]
pub struct PyFill {
    ts: i64,
    market: String,
    order_id: Option<u64>,
    size: f64,
    price: f64,
    fee: f64,
    /// "maker", "taker" or "liquidation".
    liquidity: &'static str,
}

fn py_fill(f: &Fill, names: &[String]) -> PyFill {
    PyFill {
        ts: f.ts,
        market: names[f.market].clone(),
        order_id: f.order_id,
        size: f.size,
        price: f.price,
        fee: f.fee,
        liquidity: match f.liquidity {
            Liquidity::Maker => "maker",
            Liquidity::Taker => "taker",
            Liquidity::Liquidation => "liquidation",
        },
    }
}

/// An open order.
#[pyclass(frozen, get_all, name = "Order", module = "oxarchive_backtest")]
pub struct PyOrder {
    id: u64,
    market: String,
    size: f64,
    /// "market", "limit" or "stop".
    kind: &'static str,
    /// The limit price or the stop's trigger; None for a market order.
    price: Option<f64>,
    reduce_only: bool,
}

/// What a strategy sees and may do. Orders name their market and fill from the next
/// minute; order methods return the order id.
#[pyclass(name = "Ctx", module = "oxarchive_backtest")]
pub struct Ctx {
    engine: Engine,
    names: Vec<String>,
    /// Now: the close of the latest minute, in ms.
    time: i64,
    members: Vec<String>,
    /// Kept between ticks for the strategy's own use.
    #[pyo3(get)]
    state: Py<PyDict>,
    /// Kept between ticks for the run's pieces (pending and held weights).
    #[pyo3(get, name = "_internal")]
    internal: Py<PyDict>,
}

impl Ctx {
    fn id(&self, market: &str) -> PyResult<usize> {
        self.names.iter().position(|n| n == market).ok_or_else(|| {
            PyValueError::new_err(format!(
                "unknown market {market:?}; markets: {:?}",
                self.names
            ))
        })
    }

    fn submit(&mut self, m: usize, size: f64, kind: OrderType, reduce_only: bool) -> PyResult<u64> {
        self.engine
            .submit_to(
                m,
                OrderRequest {
                    size,
                    kind,
                    reduce_only,
                },
            )
            .map_err(reject)
    }
}

#[pymethods]
impl Ctx {
    /// Now: the close of the latest minute, in ms.
    #[getter]
    fn time(&self) -> i64 {
        self.time
    }

    /// Every market's position, by name.
    #[getter]
    fn positions(&self) -> BTreeMap<String, f64> {
        self.names
            .iter()
            .enumerate()
            .map(|(m, n)| (n.clone(), self.engine.position_of(m)))
            .collect()
    }

    /// A market's last close, or None before its first.
    fn price(&self, market: &str) -> PyResult<Option<f64>> {
        Ok(self.engine.price_of(self.id(market)?))
    }

    /// Every market the run can trade.
    #[getter]
    fn markets(&self) -> Vec<String> {
        self.names.clone()
    }

    /// The universe's members now, in rank order.
    #[getter]
    fn universe(&self) -> Vec<String> {
        self.members.clone()
    }

    #[getter]
    fn cash(&self) -> f64 {
        self.engine.cash()
    }

    #[getter]
    fn equity(&self) -> f64 {
        self.engine.equity()
    }

    #[getter]
    fn open_orders(&self) -> Vec<PyOrder> {
        self.engine
            .open_orders()
            .iter()
            .map(|o| {
                let (kind, price) = match o.kind {
                    OrderType::Market => ("market", None),
                    OrderType::Limit { price } => ("limit", Some(price)),
                    OrderType::Stop { trigger } => ("stop", Some(trigger)),
                };
                PyOrder {
                    id: o.id,
                    market: self.names[o.market].clone(),
                    size: o.size,
                    kind,
                    price,
                    reduce_only: o.reduce_only,
                }
            })
            .collect()
    }

    #[pyo3(signature = (market, size, reduce_only=false))]
    fn market(&mut self, market: &str, size: f64, reduce_only: bool) -> PyResult<u64> {
        let m = self.id(market)?;
        self.submit(m, size, OrderType::Market, reduce_only)
    }

    #[pyo3(signature = (market, size, price, reduce_only=false))]
    fn limit(&mut self, market: &str, size: f64, price: f64, reduce_only: bool) -> PyResult<u64> {
        let m = self.id(market)?;
        self.submit(m, size, OrderType::Limit { price }, reduce_only)
    }

    #[pyo3(signature = (market, size, trigger, reduce_only=false))]
    fn stop(&mut self, market: &str, size: f64, trigger: f64, reduce_only: bool) -> PyResult<u64> {
        let m = self.id(market)?;
        self.submit(m, size, OrderType::Stop { trigger }, reduce_only)
    }

    fn cancel(&mut self, order_id: u64) -> PyResult<()> {
        self.engine.cancel(order_id).map_err(reject)
    }

    fn cancel_all(&mut self) {
        self.engine.cancel_all()
    }

    /// Trades every market to its target weight (a fraction of equity, signed; a market not
    /// named, or None, is 0) with market orders. A market already flat with weight 0, or
    /// one with no price yet, is left alone, and so is one whose weight is within `band` of
    /// its target (its position's value over equity against `exposure` times the weight).
    /// Returns the order ids. In a portfolio a buy the leverage cap would cut waits for the
    /// minute's sells (fill model bar-v2), so the order of markets doesn't matter.
    #[pyo3(signature = (weights, exposure=1.0, band=1e-6))]
    fn rebalance(
        &mut self,
        weights: &Bound<'_, PyAny>,
        exposure: f64,
        band: f64,
    ) -> PyResult<Vec<u64>> {
        let mut ids = Vec::new();
        for (m, delta) in self.deltas(weights, exposure, band)? {
            ids.push(self.submit(m, delta, OrderType::Market, false)?);
        }
        Ok(ids)
    }

    /// What trading to `weights` now would cost, in quote currency: taker and builder fees
    /// and slippage against the last prices, from each market's previous-minute depth (the
    /// fill model's own estimate). Arguments as in `rebalance`.
    #[pyo3(signature = (weights, exposure=1.0, band=1e-6))]
    fn cost(&self, weights: &Bound<'_, PyAny>, exposure: f64, band: f64) -> PyResult<f64> {
        let mut total = 0.0;
        for (m, delta) in self.deltas(weights, exposure, band)? {
            total += self.engine.estimate_cost(m, delta).unwrap_or(0.0);
        }
        Ok(total)
    }
}

impl Ctx {
    /// (market, size) of the trades that take the book to `weights`.
    fn deltas(
        &self,
        weights: &Bound<'_, PyAny>,
        exposure: f64,
        band: f64,
    ) -> PyResult<Vec<(usize, f64)>> {
        let mut w = vec![0.0; self.names.len()];
        for item in weights.call_method0("items")?.try_iter()? {
            let (k, v): (String, Option<f64>) = item?.extract()?;
            w[self.id(&k)?] = v.unwrap_or(0.0);
        }
        let eq = self.engine.equity();
        let mut out = Vec::new();
        for (m, &wm) in w.iter().enumerate() {
            let pos = self.engine.position_of(m);
            let Some(px) = self.engine.price_of(m) else {
                continue;
            };
            if wm == 0.0 && pos == 0.0 {
                continue;
            }
            let delta = exposure * eq * wm / px - pos;
            if delta.abs() * px > band * eq {
                out.push((m, delta));
            }
        }
        Ok(out)
    }
}

/// The bars a strategy reads: the last bars of every market, closed on their boundary. The
/// Python `History` (strategy.py) wraps this with fields and Series.
#[pyclass(name = "History", module = "oxarchive_backtest")]
pub struct PyHistory {
    /// One history per bar size the strategy reads; the first is its `bars`.
    hs: Vec<History>,
    names: Vec<String>,
    /// The markets a read covers: the universe's members at this tick.
    members: Vec<usize>,
    /// Reads made since the last bar closed or the members changed: a strategy that runs
    /// every minute on hourly bars, or reads the same thing twice in a tick, reads once.
    cache: std::sync::Mutex<HashMap<String, Vec<Option<f64>>>>,
}

fn parse_op(op: &str, params: Option<&Bound<'_, PyDict>>, names: &[String]) -> PyResult<Op> {
    let get = |k: &str| -> PyResult<Option<Bound<'_, PyAny>>> {
        match params {
            Some(p) => p.get_item(k),
            None => Ok(None),
        }
    };
    let clip: Option<f64> = match get("clip")? {
        Some(v) => v.extract()?,
        None => None,
    };
    let int = |k: &str| -> PyResult<usize> {
        get(k)?
            .ok_or_else(|| PyKeyError::new_err(format!("{op} needs {k}")))?
            .extract()
    };
    Ok(match op {
        "last" => Op::Last,
        "ago" => Op::Ago,
        "ret" => Op::Ret,
        "vol" => Op::Vol { clip },
        "mean" => Op::Mean,
        "sum" => Op::Sum,
        "std" => Op::Std,
        "min" => Op::Min,
        "max" => Op::Max,
        "logret" => Op::LogRet,
        "ewma" => {
            let halflife: f64 = get("halflife")?
                .ok_or_else(|| PyKeyError::new_err("ewma needs halflife"))?
                .extract()?;
            if halflife.is_nan() || halflife <= 0.0 {
                return Err(PyValueError::new_err("ewma: the half-life is above 0 bars"));
            }
            Op::Ewma { halflife }
        }
        "beta" | "corr" | "resid" => {
            let to: String = get("to")?
                .ok_or_else(|| PyKeyError::new_err(format!("{op} needs to")))?
                .extract()?;
            let to = if to == "universe" {
                Ref::Basket
            } else {
                Ref::Market(names.iter().position(|n| *n == to).ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "{op} against {to:?}, which is not a market of this run"
                    ))
                })?)
            };
            match op {
                "beta" => Op::Beta { to, clip },
                "corr" => Op::Corr { to, clip },
                _ => Op::Resid {
                    to,
                    fit: int("fit")?,
                    clip,
                },
            }
        }
        "seasonal" => {
            let (period, ahead) = (int("period")?, int("ahead")?);
            if period == 0 || ahead >= period {
                return Err(PyValueError::new_err(
                    "seasonal: the period is at least one bar, and ahead is less than it",
                ));
            }
            Op::Seasonal {
                period,
                ahead,
                clip,
            }
        }
        _ => return Err(PyValueError::new_err(format!("unknown read {op:?}"))),
    })
}

/// A clock, as the Python pieces describe it (x.py, `_spec`).
fn clock_from_py(d: &Bound<'_, PyDict>) -> PyResult<Clock> {
    let get = |k: &str| -> PyResult<Bound<'_, PyAny>> {
        d.get_item(k)?
            .ok_or_else(|| PyKeyError::new_err(format!("a clock needs {k}")))
    };
    let kind: String = get("kind")?.extract()?;
    Ok(match kind.as_str() {
        "every" => {
            let every_ms: i64 = get("every_ms")?.extract()?;
            if every_ms < MINUTE_MS || every_ms % MINUTE_MS != 0 {
                return Err(PyValueError::new_err(
                    "a clock ticks every whole number of minutes",
                ));
            }
            Clock::every(every_ms, get("anchor_ms")?.extract()?)
        }
        "calendar" => {
            let day: i32 = get("day")?.extract()?;
            let days = match get("days")?.extract::<String>()?.as_str() {
                "daily" => Days::Daily,
                "weekdays" => Days::Weekdays,
                "weekly" if (0..7).contains(&day) => Days::Weekly(day as u32),
                "monthly" if (1..=28).contains(&day) || (-28..=-1).contains(&day) => {
                    Days::Monthly(day)
                }
                other => {
                    return Err(PyValueError::new_err(format!(
                        "a calendar clock's days: daily, weekdays, weekly (day 0-6) or monthly \
                         (day 1 to 28, or -1 for the last); got {other} {day}"
                    )));
                }
            };
            let at_ms: i64 = get("at_ms")?.extract()?;
            if !(0..calendar::DAY_MS).contains(&at_ms) || at_ms % MINUTE_MS != 0 {
                return Err(PyValueError::new_err("at is a whole minute of the day"));
            }
            Clock::Calendar {
                days,
                at_ms,
                zone: zone_of(&get("zone")?.extract::<String>()?)?,
            }
        }
        "session" => {
            let edge = match get("edge")?.extract::<String>()?.as_str() {
                "open" => Edge::Open,
                "close" => Edge::Close,
                e => return Err(PyValueError::new_err(format!("no session edge {e:?}"))),
            };
            let offset_ms: i64 = get("offset_ms")?.extract()?;
            if offset_ms % MINUTE_MS != 0 {
                return Err(PyValueError::new_err("a session offset is whole minutes"));
            }
            Clock::Session {
                session: session_of(&get("session")?.extract::<String>()?)?,
                edge,
                offset_ms,
            }
        }
        _ => return Err(PyValueError::new_err(format!("unknown clock {kind:?}"))),
    })
}

fn zone_of(name: &str) -> PyResult<Zone> {
    Zone::from_name(name).ok_or_else(|| {
        PyValueError::new_err(format!(
            "no zone {name:?}; zones: UTC, America/New_York, America/Chicago, Europe/London, Asia/Tokyo"
        ))
    })
}

fn session_of(name: &str) -> PyResult<Exchange> {
    Exchange::from_name(name).ok_or_else(|| {
        PyValueError::new_err(format!("no session {name:?}; sessions: us, europe, asia"))
    })
}

/// The latest tick of `clock` (a clock spec) at or before `t`.
#[pyfunction]
fn _last_tick(clock: &Bound<'_, PyDict>, t: i64) -> PyResult<Option<i64>> {
    Ok(clock_from_py(clock)?.last_tick(t))
}

/// Whether `t` is a tick of `clock`.
#[pyfunction]
fn _is_tick(clock: &Bound<'_, PyDict>, t: i64) -> PyResult<bool> {
    Ok(clock_from_py(clock)?.is_tick(t))
}

/// The wall clock at `t` in `zone`: (year, month, day, weekday (0 = Monday), ms after
/// midnight, days since 1970-01-01).
#[pyfunction]
fn _local(t: i64, zone: &str) -> PyResult<(i64, u32, u32, u32, i64, i64)> {
    let l = calendar::Local::at(t, zone_of(zone)?);
    Ok((l.year, l.month, l.day, l.weekday, l.ms, l.days))
}

/// (the open session's (open, close) or None, the next open, the last close, why the
/// exchange is shut today if a holiday).
type SessionState = (
    Option<(i64, i64)>,
    Option<i64>,
    Option<i64>,
    Option<&'static str>,
);

/// One exchange session at `t`.
#[pyfunction]
fn _session(name: &str, t: i64) -> PyResult<SessionState> {
    let s = session_of(name)?;
    let day = calendar::Local::at(t, s.zone()).days;
    let holiday = match s {
        Exchange::Us if calendar::weekday(day) < 5 => calendar::nyse_holiday(day),
        _ => None,
    };
    Ok((s.at(t), s.next_open(t), s.last_close(t), holiday))
}

impl PyHistory {
    fn view(&self, bar_ms: Option<i64>) -> PyResult<&History> {
        match bar_ms {
            None => Ok(&self.hs[0]),
            Some(b) => self.hs.iter().find(|h| h.bar_ms == b).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "the strategy reads {}-minute bars, which its check before the run \
                     didn't see",
                    b / MINUTE_MS
                ))
            }),
        }
    }

    fn column(h: &History, field: &str) -> PyResult<usize> {
        let f = bars::field_id(field)
            .ok_or_else(|| PyKeyError::new_err(format!("no column {field:?}")))?;
        h.fields.iter().position(|&x| x == f).ok_or_else(|| {
            PyValueError::new_err(format!(
                "the strategy reads {field}, which its check before the run didn't see; \
                 add it with fields=[{field:?}] on the Strategy"
            ))
        })
    }

    fn too_short(e: bars::TooShort) -> PyErr {
        PyValueError::new_err(format!(
            "the strategy reads {} bars back, but the history keeps {}; its check before the \
             run saw a shorter read. Declare it with history= on the Strategy",
            e.need, e.kept
        ))
    }

    /// One minute of rows into every history.
    fn step(&mut self, ts: i64, rows: &[Option<&Row>]) {
        let before: usize = self.hs.iter().map(|h| h.closed).sum();
        for h in &mut self.hs {
            h.step(ts, rows);
        }
        if self.hs.iter().map(|h| h.closed).sum::<usize>() != before {
            self.cache.get_mut().expect("the read cache").clear();
        }
    }

    fn set_members(&mut self, members: Vec<usize>) {
        if members != self.members {
            self.cache.get_mut().expect("the read cache").clear();
            self.members = members;
        }
    }
}

#[pymethods]
impl PyHistory {
    /// The markets reads cover, in universe order.
    #[getter]
    fn markets(&self) -> Vec<String> {
        self.members
            .iter()
            .map(|&m| self.names[m].clone())
            .collect()
    }

    /// The close of the latest bar, in ms; None before the first.
    #[getter]
    fn time(&self) -> Option<i64> {
        self.hs[0].now()
    }

    /// The close of the latest bar of size `bar_ms`.
    fn time_of(&self, bar_ms: i64) -> PyResult<Option<i64>> {
        Ok(self.view(Some(bar_ms))?.now())
    }

    #[getter]
    fn bar_ms(&self) -> i64 {
        self.hs[0].bar_ms
    }

    /// `op` ("last", "ago", "ret", "logret", "vol", "mean", "sum", "std", "min", "max",
    /// "ewma", "seasonal", "beta", "corr", "resid") over the last `n` bars of `field`,
    /// `shift` bars back, one value per market, on bars of `bar_ms` (default: the
    /// strategy's). `params` are the op's own (clip; period and ahead; halflife; to, a
    /// market or "universe"; fit).
    #[pyo3(signature = (op, field, n, shift=0, bar_ms=None, **params))]
    fn read(
        &self,
        op: &str,
        field: &str,
        n: usize,
        shift: usize,
        bar_ms: Option<i64>,
        params: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Vec<Option<f64>>> {
        let h = self.view(bar_ms)?;
        let k = Self::column(h, field)?;
        let op = parse_op(op, params, &self.names)?;
        let key = format!("{}|{op:?}|{k}|{n}|{shift}", h.bar_ms);
        if let Some(v) = self.cache.lock().expect("the read cache").get(&key) {
            return Ok(v.clone());
        }
        let v = h
            .read(op, k, n, shift, &self.members)
            .map_err(Self::too_short)?;
        self.cache
            .lock()
            .expect("the read cache")
            .insert(key, v.clone());
        Ok(v)
    }

    /// The last `n` bars of `field`, oldest first, `shift` bars back, one list per market.
    #[pyo3(signature = (field, n, shift=0, bar_ms=None))]
    fn window(
        &self,
        field: &str,
        n: usize,
        shift: usize,
        bar_ms: Option<i64>,
    ) -> PyResult<Vec<Vec<Option<f64>>>> {
        let h = self.view(bar_ms)?;
        let k = Self::column(h, field)?;
        h.window(k, n, shift, &self.members)
            .map_err(Self::too_short)
    }
}

/// A universe rule, as the Python pieces describe it.
enum Rule {
    TopN(TopN),
    Custom {
        select: Py<PyAny>,
        every_ms: i64,
        anchor_ms: i64,
    },
}

impl Rule {
    fn from_py(d: &Bound<'_, PyDict>) -> PyResult<Self> {
        let get = |k: &str| -> PyResult<Bound<'_, PyAny>> {
            d.get_item(k)?
                .ok_or_else(|| PyKeyError::new_err(format!("a universe rule needs {k}")))
        };
        let kind: String = get("kind")?.extract()?;
        Ok(match kind.as_str() {
            "top_n" => {
                let opt = |k: &str| d.get_item(k);
                Rule::TopN(TopN {
                    n: get("n")?.extract()?,
                    window_hours: get("window_hours")?.extract()?,
                    min_history_hours: get("min_history_hours")?.extract()?,
                    every_hours: get("every_hours")?.extract()?,
                    anchor_ms: get("anchor_ms")?.extract()?,
                    by: match opt("by")?
                        .map(|v| v.extract::<String>())
                        .transpose()?
                        .as_deref()
                    {
                        None | Some("volume") => By::Volume,
                        Some("open_interest") => By::OpenInterest,
                        Some(b) => {
                            return Err(PyValueError::new_err(format!(
                                "top_n ranks by volume or open_interest, not {b:?}"
                            )));
                        }
                    },
                    min_volume: opt("min_volume")?.map_or(Ok(0.0), |v| v.extract())?,
                    exclude: opt("exclude")?.map_or(Ok(vec![]), |v| v.extract())?,
                    sectors: opt("sectors")?.map_or(Ok(BTreeMap::new()), |v| v.extract())?,
                    per_sector: opt("per_sector")?.map_or(Ok(0), |v| v.extract())?,
                    ..TopN::default()
                })
            }
            "custom" => Rule::Custom {
                select: get("select")?.unbind(),
                every_ms: get("every_ms")?.extract()?,
                anchor_ms: get("anchor_ms")?.extract()?,
            },
            _ => return Err(PyValueError::new_err(format!("unknown rule {kind:?}"))),
        })
    }

    /// (every, anchor) in ms.
    fn schedule(&self) -> (i64, i64) {
        match self {
            Rule::TopN(t) => (t.every_hours * HOUR_MS, t.anchor_ms),
            Rule::Custom {
                every_ms,
                anchor_ms,
                ..
            } => (*every_ms, *anchor_ms),
        }
    }

    fn pick(
        &self,
        py: Python<'_>,
        markets: &Arc<Vec<(String, Hourly)>>,
        at: i64,
    ) -> PyResult<Option<Pick>> {
        match self {
            Rule::TopN(t) => Ok(t.pick_at(markets, at)),
            Rule::Custom { select, .. } => {
                let view = Py::new(
                    py,
                    PickView {
                        data: markets.clone(),
                        at,
                    },
                )?;
                let got = select.bind(py).call1((view,))?;
                if got.is_none() {
                    return Ok(None);
                }
                let members: Vec<String> = got.extract().map_err(|_| {
                    PyValueError::new_err("a universe rule returns a list of market names, or None")
                })?;
                if let Some(m) = members
                    .iter()
                    .find(|m| !markets.iter().any(|(n, _)| n == *m))
                {
                    return Err(PyValueError::new_err(format!(
                        "the universe rule picked {m:?}, which is not a market of the run"
                    )));
                }
                Ok(Some(Pick { at, members }))
            }
        }
    }
}

/// Which markets the strategy sees at each tick.
enum Uni {
    /// The same markets throughout (indices, in the order given).
    Fixed(Vec<usize>),
    /// Picks made ahead of the run (a backtest).
    Picks(Arc<Universe>),
    /// Picks made as the run reaches each pick time, from the hours it has seen (live).
    Online {
        rule: Rule,
        hourly: Arc<Vec<(String, Hourly)>>,
        next: Option<i64>,
        current: Vec<String>,
        picks: Vec<Pick>,
    },
}

/// One history a run keeps: a bar size, the columns read on it, how many bars to keep and
/// how far prices carry.
struct HistSpec {
    bar_ms: i64,
    carry_bars: usize,
    capacity: usize,
    fields: Vec<usize>,
}

/// The spec of a run that the Python side works out (strategy.py, `Strategy._spec`).
struct Spec {
    /// The strategy's own bar size first.
    histories: Vec<HistSpec>,
    /// Bad-print repair's ratio, if on.
    repair: Option<f64>,
    exits: Vec<Exit>,
    drawdown: Option<DrawdownStop>,
    clock: Clock,
    /// Re-trade to the last target weights at these ticks between the clock's.
    hold: Option<Clock>,
    /// Trade this long after each tick of the clock instead of at it.
    delay_ms: i64,
    /// The first minute close at which the strategy runs; earlier minutes only warm up.
    start: i64,
}

impl Spec {
    fn from_py(d: &Bound<'_, PyDict>) -> PyResult<Self> {
        let get = |k: &str| -> PyResult<Bound<'_, PyAny>> {
            d.get_item(k)?
                .ok_or_else(|| PyKeyError::new_err(format!("a run spec needs {k}")))
        };
        let mut histories = Vec::new();
        for item in get("histories")?.try_iter()? {
            let item = item?;
            let h = item.cast::<PyDict>()?;
            let hget = |k: &str| -> PyResult<Bound<'_, PyAny>> {
                h.get_item(k)?
                    .ok_or_else(|| PyKeyError::new_err(format!("a history spec needs {k}")))
            };
            let names: Vec<String> = hget("fields")?.extract()?;
            let mut fields = Vec::new();
            for n in names {
                fields.push(
                    bars::field_id(&n)
                        .ok_or_else(|| PyKeyError::new_err(format!("no column {n:?}")))?,
                );
            }
            histories.push(HistSpec {
                bar_ms: hget("bar_ms")?.extract()?,
                carry_bars: hget("carry_bars")?.extract()?,
                capacity: hget("capacity")?.extract()?,
                fields,
            });
        }
        if histories.is_empty() {
            return Err(PyValueError::new_err(
                "a run spec needs at least one history",
            ));
        }
        Ok(Spec {
            histories,
            repair: match d.get_item("repair")? {
                Some(v) => v.extract()?,
                None => None,
            },
            exits: match d.get_item("exits")? {
                Some(v) => exits_from_py(&v)?,
                None => Vec::new(),
            },
            drawdown: match d.get_item("drawdown")? {
                Some(v) if !v.is_none() => {
                    let (max, cooldown_ms): (f64, Option<i64>) = v.extract()?;
                    if !(0.0..1.0).contains(&max) || max == 0.0 {
                        return Err(PyValueError::new_err("a drawdown stop is between 0 and 1"));
                    }
                    Some(DrawdownStop { max, cooldown_ms })
                }
                _ => None,
            },
            clock: clock_from_py(get("clock")?.cast()?)?,
            hold: match d.get_item("hold")? {
                Some(h) if !h.is_none() => Some(clock_from_py(h.cast()?)?),
                _ => None,
            },
            delay_ms: match d.get_item("delay_ms")? {
                Some(v) => v.extract()?,
                None => 0,
            },
            start: get("start")?.extract()?,
        })
    }
}

/// Exits as the Python pieces describe them: [(kind, value)].
fn exits_from_py(v: &Bound<'_, PyAny>) -> PyResult<Vec<Exit>> {
    let mut out = Vec::new();
    for item in v.try_iter()? {
        let (kind, x): (String, f64) = item?.extract()?;
        if x.is_nan() || x <= 0.0 {
            return Err(PyValueError::new_err(format!("{kind}: above 0, got {x}")));
        }
        out.push(match kind.as_str() {
            "stop_loss" => Exit::StopLoss(x),
            "take_profit" => Exit::TakeProfit(x),
            "trailing_stop" => Exit::Trailing(x),
            "time_stop" => Exit::Time(x as i64),
            _ => return Err(PyValueError::new_err(format!("unknown exit {kind:?}"))),
        });
    }
    Ok(out)
}

/// The markets of a run, and their limits. `markets` pairs each name with its dataset
/// market (`hyperliquid:BTC`), which picks the default leverage (oxs_engine::markets);
/// `max_leverage` overrides it by name.
fn specs(
    markets: &[(String, Option<String>)],
    max_leverage: Option<BTreeMap<String, f64>>,
    cfg: &Config,
) -> PyResult<Vec<MarketSpec>> {
    let lev = max_leverage.unwrap_or_default();
    for k in lev.keys() {
        if !markets.iter().any(|(n, _)| n == k) {
            return Err(PyValueError::new_err(format!(
                "max_leverage names {k:?}, which is not a market of this run"
            )));
        }
    }
    Ok(markets
        .iter()
        .map(|(name, ds)| {
            oxs_engine::markets::spec(
                name,
                ds.as_deref().unwrap_or(""),
                lev.get(name).copied(),
                cfg,
            )
        })
        .collect())
}

/// Runs a strategy one minute at a time: the one loop behind `_backtest` and `Session`, so
/// a paper session fed the dataset's rows gives the backtest's result.
struct Run {
    strategy: Py<PyAny>,
    ctx: Py<Ctx>,
    hist: Py<PyHistory>,
    has_fill: bool,
    /// The strategy's `on_universe`, when it has one: (members at the last minute).
    watch_members: Option<Vec<usize>>,
    exits: Exits,
    /// Sums over the window's minutes of gross and net exposure (position value over
    /// equity), the largest gross, and the minutes counted.
    exposure: (f64, f64, f64, usize),
    seen: usize,
    clock: Clock,
    hold: Option<Clock>,
    delay_ms: i64,
    start: i64,
    uni: Uni,
    index: HashMap<String, usize>,
    curve: Vec<(i64, f64)>,
    cfg: Config,
    last_ts: Option<i64>,
}

impl Run {
    fn new(
        py: Python<'_>,
        strategy: Bound<'_, PyAny>,
        markets: Vec<(String, Option<String>)>,
        cfg: Config,
        max_leverage: Option<BTreeMap<String, f64>>,
        spec: Spec,
        uni: Uni,
    ) -> PyResult<Self> {
        if markets.is_empty() {
            return Err(PyValueError::new_err("a run needs at least one market"));
        }
        let specs = specs(&markets, max_leverage, &cfg)?;
        let names: Vec<String> = markets.into_iter().map(|(n, _)| n).collect();
        let names_len = names.len();
        let index: HashMap<String, usize> = names
            .iter()
            .enumerate()
            .map(|(i, n)| (n.clone(), i))
            .collect();
        if let Uni::Picks(u) = &uni {
            for p in &u.picks {
                if let Some(m) = p.members.iter().find(|m| !index.contains_key(*m)) {
                    return Err(PyValueError::new_err(format!(
                        "the universe holds {m:?}, which is not a market of this run"
                    )));
                }
            }
        }
        let mut hs = Vec::new();
        for hspec in &spec.histories {
            let mut h = History::new(
                hspec.bar_ms,
                hspec.fields.clone(),
                names.len(),
                hspec.capacity,
                hspec.carry_bars,
            );
            if let Some(ratio) = spec.repair {
                h.set_repair(ratio).map_err(PyValueError::new_err)?;
            }
            hs.push(h);
        }
        let hist = PyHistory {
            hs,
            names: names.clone(),
            members: Vec::new(),
            cache: Default::default(),
        };
        let ctx = Ctx {
            engine: Engine::with_markets(cfg, specs),
            names,
            time: i64::MIN,
            members: Vec::new(),
            state: PyDict::new(py).unbind(),
            internal: PyDict::new(py).unbind(),
        };
        Ok(Run {
            has_fill: strategy.getattr("_has_fill")?.extract()?,
            watch_members: strategy
                .getattr("_has_universe")?
                .extract::<bool>()?
                .then(Vec::new),
            exits: Exits::new(spec.exits.clone(), spec.drawdown, names_len),
            exposure: (0.0, 0.0, 0.0, 0),
            strategy: strategy.unbind(),
            ctx: Py::new(py, ctx)?,
            hist: Py::new(py, hist)?,
            seen: 0,
            clock: spec.clock,
            hold: spec.hold,
            delay_ms: spec.delay_ms,
            start: spec.start,
            uni,
            index,
            curve: Vec::new(),
            cfg,
            last_ts: None,
        })
    }

    fn names(&self, py: Python<'_>) -> Vec<String> {
        self.ctx.borrow(py).names.clone()
    }

    /// The universe's members at minute close `t`, as indices.
    fn members(&self, t: i64) -> Vec<usize> {
        let names: &[String] = match &self.uni {
            Uni::Fixed(v) => return v.clone(),
            Uni::Picks(u) => u.members_at(t),
            Uni::Online { current, .. } => current,
        };
        names.iter().map(|n| self.index[n]).collect()
    }

    /// An online universe sees the minute's rows, then makes any pick now due.
    fn observe(&mut self, py: Python<'_>, t: i64, rows: &[Option<&Row>]) -> PyResult<()> {
        let Uni::Online {
            rule,
            hourly,
            next,
            current,
            picks,
        } = &mut self.uni
        else {
            return Ok(());
        };
        let h = Arc::get_mut(hourly).expect("only a pick holds the hourly series, briefly");
        for (m, r) in rows.iter().enumerate() {
            if let Some(r) = r {
                h[m].1.push(r);
            }
        }
        let (every, anchor) = rule.schedule();
        let mut at = next.unwrap_or_else(|| {
            if t < anchor {
                anchor
            } else {
                t - (t - anchor).rem_euclid(every)
            }
        });
        while at <= t {
            if let Some(p) = rule.pick(py, hourly, at)? {
                *current = p.members.clone();
                picks.push(p);
            }
            at += every;
        }
        *next = Some(at);
        Ok(())
    }

    fn step(&mut self, py: Python<'_>, ts: i64, rows: &[Option<&Row>]) -> PyResult<()> {
        let t = ts + MINUTE_MS;
        let fills = {
            let mut c = self.ctx.borrow_mut(py);
            c.time = t;
            c.engine.step_markets(rows);
            let fills = c.engine.fills();
            let out: Vec<PyFill> = if self.has_fill {
                fills[self.seen..]
                    .iter()
                    .map(|f| py_fill(f, &c.names))
                    .collect()
            } else {
                Vec::new()
            };
            self.seen = fills.len();
            out
        };
        for f in fills {
            self.strategy
                .bind(py)
                .call_method1("_fill", (self.ctx.clone_ref(py), f))?;
        }
        self.hist.borrow_mut(py).step(ts, rows);
        self.observe(py, t, rows)?;
        self.last_ts = Some(ts);
        if t < self.start {
            return Ok(());
        }
        if !self.exits.is_empty() {
            self.exits
                .check(&mut self.ctx.borrow_mut(py).engine, rows, t);
        }
        if let Some(last) = &self.watch_members {
            let now = self.members(t);
            if now != *last {
                let names = self.names(py);
                let joined: Vec<&String> = now
                    .iter()
                    .filter(|m| !last.contains(m))
                    .map(|&m| &names[m])
                    .collect();
                let left: Vec<&String> = last
                    .iter()
                    .filter(|m| !now.contains(m))
                    .map(|&m| &names[m])
                    .collect();
                {
                    let mut c = self.ctx.borrow_mut(py);
                    c.members = now.iter().map(|&m| c.names[m].clone()).collect();
                }
                self.strategy
                    .bind(py)
                    .call_method1("_universe", (self.ctx.clone_ref(py), joined, left))?;
                self.watch_members = Some(now);
            }
        }
        let halted = self.exits.halted(t);
        let tick = !halted && self.clock.is_tick(t);
        let act = !halted && self.delay_ms > 0 && self.clock.is_tick(t - self.delay_ms);
        let hold = !halted && !tick && !act && self.hold.is_some_and(|c| c.is_tick(t));
        if tick || act || hold {
            let members = self.members(t);
            {
                let mut c = self.ctx.borrow_mut(py);
                c.members = members.iter().map(|&m| c.names[m].clone()).collect();
            }
            self.hist.borrow_mut(py).set_members(members);
            self.strategy.bind(py).call_method1(
                "_tick",
                (
                    self.ctx.clone_ref(py),
                    self.hist.clone_ref(py),
                    tick,
                    act,
                    hold,
                ),
            )?;
        }
        let c = self.ctx.borrow(py);
        let eq = c.engine.equity();
        self.curve.push((t, eq));
        if eq > 0.0 {
            let (mut gross, mut net) = (0.0, 0.0);
            for m in 0..c.names.len() {
                let pos = c.engine.position_of(m);
                if pos != 0.0
                    && let Some(px) = c.engine.price_of(m)
                {
                    gross += (pos * px).abs();
                    net += pos * px;
                }
            }
            let e = &mut self.exposure;
            e.0 += gross / eq;
            e.1 += net / eq;
            e.2 = e.2.max(gross / eq);
            e.3 += 1;
        }
        Ok(())
    }

    fn report(
        &self,
        py: Python<'_>,
        datasets: Vec<DatasetRef>,
        window: Option<(i64, i64)>,
    ) -> PyReport {
        let c = self.ctx.borrow(py);
        let r = c.engine.clone().finish();
        let picks = match &self.uni {
            Uni::Picks(u) => Some(u.clone()),
            Uni::Online { picks, .. } => Some(Arc::new(Universe {
                rule: None,
                picks: picks.clone(),
            })),
            Uni::Fixed(_) => None,
        };
        let mut rep = PyReport::new(
            r,
            self.cfg,
            &c.names,
            datasets,
            self.curve.clone(),
            window,
            picks,
        );
        let (g, n, mx, k) = self.exposure;
        rep.exposure = (k > 0).then(|| (g / k as f64, n / k as f64, mx));
        rep.exits = self
            .exits
            .log
            .iter()
            .map(|&(t, m, why)| {
                let why = match why {
                    Reason::StopLoss => "stop_loss",
                    Reason::TakeProfit => "take_profit",
                    Reason::Trailing => "trailing_stop",
                    Reason::Time => "time_stop",
                    Reason::Drawdown => "drawdown_stop",
                };
                (t, m.map(|m| c.names[m].clone()), why)
            })
            .collect();
        rep
    }
}

fn data_list<'py>(data: &Bound<'py, PyAny>) -> PyResult<Vec<(String, PyRef<'py, Dataset>)>> {
    let mut out = Vec::new();
    for item in data.try_iter()? {
        let (name, ds): (String, Bound<'py, Dataset>) = item?.extract()?;
        out.push((name, ds.borrow()));
    }
    Ok(out)
}

fn universe_of(
    members: Option<Vec<String>>,
    picks: Option<PyRef<'_, PyUniverse>>,
    rule: Option<&Bound<'_, PyDict>>,
    names: &[String],
) -> PyResult<Uni> {
    match (members, picks, rule) {
        (Some(m), None, None) => {
            let mut idx = Vec::new();
            for n in &m {
                idx.push(names.iter().position(|x| x == n).ok_or_else(|| {
                    PyValueError::new_err(format!("{n:?} is not a market of this run"))
                })?);
            }
            Ok(Uni::Fixed(idx))
        }
        (None, Some(u), None) => Ok(Uni::Picks(u.0.clone())),
        (None, None, Some(r)) => Ok(Uni::Online {
            rule: Rule::from_py(r)?,
            hourly: Arc::new(
                names
                    .iter()
                    .map(|n| (n.clone(), Hourly::default()))
                    .collect(),
            ),
            next: None,
            current: Vec::new(),
            picks: Vec::new(),
        }),
        _ => Err(PyValueError::new_err(
            "give one of members=, universe= or rule=",
        )),
    }
}

/// Runs `strategy` (a Python `Strategy`: `_tick(ctx, history)`, and `_fill(ctx, fill)` when
/// `_has_fill`) over `data`, a list of (name, Dataset), from `from_ms` to `until` (ms,
/// exclusive). The strategy trades from `spec["start"]`; earlier minutes warm the history.
/// The universe is `members` (fixed) or `universe` (picks made ahead).
#[pyfunction]
#[pyo3(signature = (strategy, data, spec, from_ms, until, config=None, max_leverage=None, members=None, universe=None, extra_files=None, benchmark=None))]
#[allow(clippy::too_many_arguments)]
fn _backtest(
    py: Python<'_>,
    strategy: Bound<'_, PyAny>,
    data: &Bound<'_, PyAny>,
    spec: &Bound<'_, PyDict>,
    from_ms: i64,
    until: i64,
    config: Option<PyConfig>,
    max_leverage: Option<BTreeMap<String, f64>>,
    members: Option<Vec<String>>,
    universe: Option<PyRef<'_, PyUniverse>>,
    extra_files: Option<Vec<(String, String, String)>>,
    benchmark: Option<Py<PyReport>>,
) -> PyResult<PyReport> {
    let cfg = config.map_or_else(Config::default, |c| c.0);
    let sets = data_list(data)?;
    let markets: Vec<(String, Option<String>)> = sets
        .iter()
        .map(|(n, ds)| (n.clone(), ds.market()))
        .collect();
    let names: Vec<String> = markets.iter().map(|(n, _)| n.clone()).collect();
    let spec = Spec::from_py(spec)?;
    let start = spec.start;
    let mut uni = universe_of(members, universe, None, &names)?;
    if let Uni::Picks(u) = &uni {
        // Only the picks in force while the strategy runs: the one at `start`, then those
        // before `until`. Earlier picks may hold markets the run doesn't load.
        let first = u.picks.partition_point(|p| p.at <= start).saturating_sub(1);
        let picks = u.picks[first..]
            .iter()
            .take_while(|p| p.at < until)
            .cloned()
            .collect();
        uni = Uni::Picks(Arc::new(Universe {
            rule: u.rule.clone(),
            picks,
        }));
    }
    let mut run = Run::new(py, strategy, markets, cfg, max_leverage, spec, uni)?;
    let mut cursors: Vec<Cursor<'_>> = sets.iter().map(|(_, ds)| ds.cursor()).collect();
    walk(&mut cursors, from_ms, until, |step, ts| {
        run.step(py, ts, step)
    })?;
    let mut datasets: Vec<DatasetRef> = sets.iter().flat_map(|(_, ds)| ds.files.clone()).collect();
    for (market, month, dataset_version) in extra_files.unwrap_or_default() {
        datasets.push(DatasetRef {
            market,
            month,
            dataset_version,
        });
    }
    let mut rep = run.report(py, datasets, Some((start, until)));
    rep.benchmark = benchmark;
    Ok(rep)
}

/// Drives a strategy with rows as they arrive, one minute at a time: paper trading on the
/// live stream, with the backtest's loop and fill model. The Python `Session`
/// (python/oxarchive_backtest/run.py) builds it.
#[pyclass(name = "_Session", module = "oxarchive_backtest")]
pub struct Session {
    run: Run,
    names: Vec<String>,
}

#[pymethods]
impl Session {
    #[new]
    #[pyo3(signature = (strategy, markets, spec, config=None, max_leverage=None, members=None, rule=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        py: Python<'_>,
        strategy: Bound<'_, PyAny>,
        markets: Vec<(String, String)>,
        spec: &Bound<'_, PyDict>,
        config: Option<PyConfig>,
        max_leverage: Option<BTreeMap<String, f64>>,
        members: Option<Vec<String>>,
        rule: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let cfg = config.map_or_else(Config::default, |c| c.0);
        let names: Vec<String> = markets.iter().map(|(n, _)| n.clone()).collect();
        let uni = universe_of(members, None, rule, &names)?;
        let markets = markets.into_iter().map(|(n, d)| (n, Some(d))).collect();
        Ok(Session {
            run: Run::new(
                py,
                strategy,
                markets,
                cfg,
                max_leverage,
                Spec::from_py(spec)?,
                uni,
            )?,
            names,
        })
    }

    /// Feeds one closed minute: a dict of market name -> row dict, leaving out a market
    /// with no row that minute. Rows must arrive in increasing `ts` order.
    fn step(&mut self, py: Python<'_>, rows: &Bound<'_, PyDict>) -> PyResult<()> {
        for k in rows.keys() {
            let k: String = k.extract()?;
            if !self.names.contains(&k) {
                return Err(PyKeyError::new_err(format!(
                    "unknown market {k:?}; markets: {:?}",
                    self.names
                )));
            }
        }
        let parsed: Vec<Option<Row>> = self
            .names
            .iter()
            .map(|m| match rows.get_item(m)? {
                Some(r) => Ok(Some(row_from_dict(r.cast::<PyDict>()?)?)),
                None => Ok(None),
            })
            .collect::<PyResult<_>>()?;
        let mut ts = None;
        for r in parsed.iter().flatten() {
            if ts.is_some_and(|t| t != r.ts) {
                return Err(PyValueError::new_err("the rows of one step must share ts"));
            }
            ts = Some(r.ts);
        }
        let ts = ts.ok_or_else(|| PyValueError::new_err("a step needs at least one row"))?;
        if let Some(last) = self.run.last_ts
            && ts <= last
        {
            return Err(PyValueError::new_err(format!(
                "rows must arrive in increasing ts order: {ts} after {last}"
            )));
        }
        let refs: Vec<Option<&Row>> = parsed.iter().map(Option::as_ref).collect();
        self.run.step(py, ts, &refs)
    }

    /// Steps stored rows through the session: every minute of `data` (a list of (name,
    /// Dataset) for some or all of the markets) that is after the session's last row and
    /// before `until`. Returns the number of minutes stepped.
    #[pyo3(signature = (data, until=None))]
    fn replay(
        &mut self,
        py: Python<'_>,
        data: &Bound<'_, PyAny>,
        until: Option<i64>,
    ) -> PyResult<usize> {
        let sets = data_list(data)?;
        let mut held: Vec<Option<&Dataset>> = vec![None; self.names.len()];
        for (k, ds) in &sets {
            let m = self.names.iter().position(|n| n == k).ok_or_else(|| {
                PyKeyError::new_err(format!("unknown market {k:?}; markets: {:?}", self.names))
            })?;
            held[m] = Some(&**ds);
        }
        let from = self.run.last_ts.map_or(i64::MIN, |t| t + 1);
        let empty = Dataset {
            parts: Vec::new(),
            files: Vec::new(),
            columns: None,
        };
        let mut cursors: Vec<Cursor<'_>> =
            held.iter().map(|d| d.unwrap_or(&empty).cursor()).collect();
        let run = &mut self.run;
        walk(&mut cursors, from, until.unwrap_or(i64::MAX), |step, ts| {
            run.step(py, ts, step)
        })
    }

    /// Feeds an online universe the hours before the first minute replayed: `data` is a
    /// list of (name, Dataset) of hourly bars (or minute rows); rows from `until` (ms) on
    /// are left for the replay.
    fn preload_hourly(&mut self, data: &Bound<'_, PyAny>, until: i64) -> PyResult<()> {
        if self.run.last_ts.is_some() {
            return Err(PyValueError::new_err("preload before the first step"));
        }
        let Uni::Online { hourly, .. } = &mut self.run.uni else {
            return Ok(());
        };
        let h = Arc::get_mut(hourly).expect("not shared before the first step");
        for (k, ds) in data_list(data)? {
            let Some(m) = h.iter().position(|(n, _)| *n == k) else {
                continue;
            };
            walk(&mut [ds.cursor()], i64::MIN, until, |step, _| {
                if let Some(r) = step[0] {
                    h[m].1.push(r);
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    /// The `ts` of the last minute stepped or replayed, or None before the first.
    #[getter]
    fn last_ts(&self) -> Option<i64> {
        self.run.last_ts
    }

    /// The strategy's context: positions, cash, equity and open orders so far.
    #[getter]
    fn ctx(&self, py: Python<'_>) -> Py<Ctx> {
        self.run.ctx.clone_ref(py)
    }

    /// The run so far, as a Report. Its receipt names each market with month "live".
    fn report(&self, py: Python<'_>) -> PyReport {
        let datasets = self
            .run
            .ctx
            .borrow(py)
            .engine
            .markets()
            .iter()
            .map(|m| DatasetRef {
                market: m.name.clone(),
                month: "live".into(),
                dataset_version: oxs_schema::SCHEMA_VERSION.into(),
            })
            .collect();
        self.run.report(py, datasets, None)
    }

    #[getter]
    fn markets(&self, py: Python<'_>) -> Vec<String> {
        self.run.names(py)
    }
}

#[pyclass(frozen, name = "Report", module = "oxarchive_backtest")]
pub struct PyReport {
    #[pyo3(get)]
    fills: Vec<PyFill>,
    #[pyo3(get)]
    liquidations: u32,
    #[pyo3(get)]
    bad_debt: f64,
    #[pyo3(get)]
    funding_paid: f64,
    #[pyo3(get)]
    final_cash: f64,
    /// Every market's final position, by name.
    #[pyo3(get)]
    final_positions: BTreeMap<String, f64>,
    #[pyo3(get)]
    final_equity: Option<f64>,
    /// (minute close, equity) from the window's start.
    #[pyo3(get)]
    equity: Vec<(i64, f64)>,
    /// 16 hex digits.
    #[pyo3(get)]
    result_hash: String,
    /// (minute close, market or None for the whole book, why) of every exit the run sent.
    #[pyo3(get)]
    exits: Vec<(i64, Option<String>, &'static str)>,
    /// The benchmark run over the same window, if one was asked for.
    #[pyo3(get)]
    benchmark: Option<Py<PyReport>>,
    /// Mean gross and net exposure over the window's minutes, and the largest gross.
    exposure: Option<(f64, f64, f64)>,
    attribution: Vec<Attribution>,
    names: Vec<String>,
    stats: Option<Stats>,
    picks: Option<Arc<Universe>>,
    window: Option<(i64, i64)>,
    cfg: Config,
    markets: Vec<MarketSpec>,
    datasets: Vec<DatasetRef>,
    result: ResultRecord,
}

impl PyReport {
    fn new(
        r: Report,
        cfg: Config,
        names: &[String],
        datasets: Vec<DatasetRef>,
        curve: Vec<(i64, f64)>,
        window: Option<(i64, i64)>,
        picks: Option<Arc<Universe>>,
    ) -> Self {
        PyReport {
            fills: r.fills.iter().map(|f| py_fill(f, names)).collect(),
            liquidations: r.liquidations,
            bad_debt: r.bad_debt,
            funding_paid: r.funding_paid,
            final_cash: r.final_cash,
            final_positions: names
                .iter()
                .cloned()
                .zip(r.final_positions.clone())
                .collect(),
            final_equity: r.final_equity,
            result_hash: format!("{:016x}", r.result_hash),
            exits: Vec::new(),
            benchmark: None,
            exposure: None,
            attribution: attribution(
                &r.fills,
                names.len(),
                &r.final_positions,
                &r.final_prices,
                &r.funding_by_market,
            ),
            names: names.to_vec(),
            result: ResultRecord::of(&r),
            stats: stats(&curve),
            equity: curve,
            picks,
            window,
            markets: r.markets,
            cfg,
            datasets,
        }
    }
}

/// Where this wheel runs, for the receipt.
const PLATFORM: &str = if cfg!(target_os = "emscripten") {
    "pyodide"
} else {
    "python"
};

#[pymethods]
impl PyReport {
    /// The headline numbers over the window: start, end, start_equity, end_equity, return,
    /// sharpe (annualised, daily), max_drawdown, and fills, fees, funding_paid, liquidations.
    /// None for a run with no minutes in its window.
    #[getter]
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        let Some(s) = &self.stats else {
            return Ok(None);
        };
        let mut fees = 0.0;
        let mut volume = 0.0;
        for f in &self.fills {
            fees += f.fee;
            volume += (f.size * f.price).abs();
        }
        let (mut trades, mut wins) = (0, 0);
        for a in &self.attribution {
            trades += a.trades;
            wins += a.wins;
        }
        let mut mean_eq = 0.0;
        for &(_, e) in &self.equity {
            mean_eq += e;
        }
        mean_eq /= self.equity.len().max(1) as f64;
        let days = (s.end - s.start) as f64 / oxs_engine::stats::DAY_MS as f64;
        let d = PyDict::new(py);
        d.set_item("start", s.start)?;
        d.set_item("end", s.end)?;
        d.set_item("start_equity", s.start_equity)?;
        d.set_item("end_equity", s.end_equity)?;
        d.set_item("return", s.ret)?;
        d.set_item("annual_return", s.annual_return)?;
        d.set_item("volatility", s.volatility)?;
        d.set_item("sharpe", s.sharpe)?;
        d.set_item("sortino", s.sortino)?;
        d.set_item("max_drawdown", s.max_drawdown)?;
        d.set_item("calmar", s.calmar)?;
        d.set_item("hit_rate", s.hit_rate)?;
        d.set_item("trades", trades)?;
        d.set_item(
            "win_rate",
            (trades > 0).then(|| wins as f64 / trades as f64),
        )?;
        // Notional traded over mean equity, per year.
        d.set_item(
            "turnover",
            (days > 0.0 && mean_eq > 0.0).then(|| volume / mean_eq * 365.0 / days),
        )?;
        if let Some((g, n, mx)) = self.exposure {
            d.set_item("exposure", g)?;
            d.set_item("net_exposure", n)?;
            d.set_item("max_exposure", mx)?;
        }
        d.set_item("fills", self.fills.len())?;
        d.set_item("fees", fees)?;
        d.set_item("funding_paid", self.funding_paid)?;
        d.set_item("liquidations", self.liquidations)?;
        d.set_item("exits", self.exits.len())?;
        Ok(Some(d))
    }

    /// Each market's share: {market: {pnl, fees, funding, volume, trades, wins}}, PnL after
    /// its fees and funding, marked at its last price. The PnLs sum to the change in equity
    /// (less any bad debt).
    #[getter]
    fn attribution<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let out = PyDict::new(py);
        for (name, a) in self.names.iter().zip(&self.attribution) {
            let d = PyDict::new(py);
            d.set_item("pnl", a.pnl)?;
            d.set_item("fees", a.fees)?;
            d.set_item("funding", a.funding)?;
            d.set_item("volume", a.volume)?;
            d.set_item("trades", a.trades)?;
            d.set_item("wins", a.wins)?;
            out.set_item(name, d)?;
        }
        Ok(out)
    }

    /// Daily returns from the first day the equity moved: what the stats are taken over.
    #[getter]
    fn daily_returns(&self) -> Vec<f64> {
        daily_returns(&self.equity)
    }

    /// Against the benchmark, from daily returns: excess_return, beta, alpha (annualised),
    /// correlation, tracking_error and information_ratio. None without a benchmark.
    #[getter]
    fn vs<'py>(&self, py: Python<'py>) -> PyResult<Option<Bound<'py, PyDict>>> {
        let Some(b) = &self.benchmark else {
            return Ok(None);
        };
        let Some(v) = versus(&self.equity, &b.get().equity) else {
            return Ok(None);
        };
        let d = PyDict::new(py);
        d.set_item("excess_return", v.excess_return)?;
        d.set_item("beta", v.beta)?;
        d.set_item("alpha", v.alpha)?;
        d.set_item("correlation", v.correlation)?;
        d.set_item("tracking_error", v.tracking_error)?;
        d.set_item("information_ratio", v.information_ratio)?;
        Ok(Some(d))
    }

    /// The universe's picks the run used, or None for fixed markets.
    #[getter]
    fn universe(&self) -> Option<PyUniverse> {
        self.picks.clone().map(PyUniverse)
    }

    /// The run's receipt: engine, fill model and dataset versions, the window,
    /// the SHA-256 of `source` (the strategy's source, never stored), `params` (a JSON-able
    /// dict) and the result. Two runs with the same receipt_id are the same run with the
    /// same result.
    #[pyo3(signature = (source, params=None))]
    fn receipt(
        &self,
        py: Python<'_>,
        source: &Bound<'_, PyAny>,
        params: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyReceipt> {
        let source: Vec<u8> = match source.cast::<PyString>() {
            Ok(s) => s.to_str()?.as_bytes().to_vec(),
            Err(_) => source.extract()?,
        };
        let params: BTreeMap<String, serde_json::Value> = match params {
            None => BTreeMap::new(),
            Some(p) => {
                let json: String = py
                    .import("json")?
                    .call_method(
                        "dumps",
                        (p,),
                        Some(&[("allow_nan", false)].into_py_dict(py)?),
                    )?
                    .extract()?;
                serde_json::from_str(&json)
                    .map_err(|e| PyValueError::new_err(format!("params must be a dict: {e}")))?
            }
        };
        let mut r = Receipt::with_result(
            PLATFORM,
            self.datasets.clone(),
            &source,
            params,
            self.cfg,
            market_records(&self.markets, &self.cfg),
            self.result.clone(),
        );
        r.window = self.window;
        Ok(PyReceipt(r))
    }
}

/// A backtest receipt. `json` is its canonical form, the body of `POST /receipts`.
#[pyclass(frozen, name = "Receipt", module = "oxarchive_backtest")]
pub struct PyReceipt(Receipt);

#[pymethods]
impl PyReceipt {
    /// SHA-256 of the canonical JSON, hex.
    #[getter]
    fn receipt_id(&self) -> String {
        self.0.receipt_id()
    }

    #[getter]
    fn json(&self) -> String {
        self.0.to_canonical_json()
    }

    #[getter]
    fn result_hash(&self) -> String {
        self.0.result.result_hash.clone()
    }

    #[getter]
    fn strategy_source_sha256(&self) -> String {
        self.0.strategy_source_sha256.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "Receipt(id={}, result_hash={})",
            &self.0.receipt_id()[..16],
            self.0.result.result_hash
        )
    }
}

/// A point-in-time universe: which markets a strategy may trade, from each pick to the next.
#[pyclass(frozen, name = "Universe", module = "oxarchive_backtest")]
pub struct PyUniverse(Arc<Universe>);

#[pymethods]
impl PyUniverse {
    /// (pick time in ms, members in rank order), oldest first.
    #[getter]
    fn picks(&self) -> Vec<(i64, Vec<String>)> {
        self.0
            .picks
            .iter()
            .map(|p| (p.at, p.members.clone()))
            .collect()
    }

    /// The members at `t` (ms): the latest pick at or before it.
    fn members(&self, t: i64) -> Vec<String> {
        self.0.members_at(t).to_vec()
    }

    /// Every market that is a member at some time in [start, end), sorted.
    #[pyo3(signature = (start=i64::MIN, end=i64::MAX))]
    fn markets_between(&self, start: i64, end: i64) -> Vec<String> {
        // The pick in force at `start`, then every pick before `end`.
        let first = self
            .0
            .picks
            .partition_point(|p| p.at <= start)
            .saturating_sub(1);
        let mut v: Vec<String> = self.0.picks[first..]
            .iter()
            .take_while(|p| p.at < end)
            .flat_map(|p| p.members.iter().cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// The universe as JSON, to store or send to a browser and read back with `from_json`.
    #[getter]
    fn json(&self) -> PyResult<String> {
        serde_json::to_string(&*self.0).map_err(|e| PyValueError::new_err(e.to_string()))
    }

    #[staticmethod]
    fn from_json(s: &str) -> PyResult<Self> {
        let u: Universe = serde_json::from_str(s)
            .map_err(|e| PyValueError::new_err(format!("not a universe: {e}")))?;
        if u.picks.windows(2).any(|w| w[0].at >= w[1].at) {
            return Err(PyValueError::new_err(
                "a universe's picks must be in time order",
            ));
        }
        Ok(PyUniverse(Arc::new(u)))
    }

    fn __len__(&self) -> usize {
        self.0.picks.len()
    }

    fn __repr__(&self) -> String {
        format!(
            "Universe({} picks, {} markets)",
            self.0.picks.len(),
            self.markets_between(i64::MIN, i64::MAX).len()
        )
    }
}

/// Each market's hourly series, read from its rows (minute rows or hourly bars).
fn hourly_of(data: &Bound<'_, PyAny>) -> PyResult<Vec<(String, Hourly)>> {
    let mut out = Vec::new();
    for (name, ds) in data_list(data)? {
        let mut h = Hourly::default();
        walk(&mut [ds.cursor()], i64::MIN, i64::MAX, |step, _| {
            if let Some(r) = step[0] {
                h.push(r);
            }
            Ok(())
        })?;
        out.push((name, h));
    }
    Ok(out)
}

/// The picks of `rule` (a dict, as the Python pieces describe it) over `data`, a list of
/// (name, Dataset) of every market that could be picked, made before `until` (ms).
#[pyfunction]
fn _picks(
    py: Python<'_>,
    data: &Bound<'_, PyAny>,
    rule: &Bound<'_, PyDict>,
    until: i64,
) -> PyResult<PyUniverse> {
    let rule = Rule::from_py(rule)?;
    let hourly = Arc::new(hourly_of(data)?);
    let (every, anchor) = rule.schedule();
    let mut picks = Vec::new();
    for at in schedule(anchor, every / HOUR_MS, &hourly) {
        if at >= until {
            break;
        }
        if let Some(p) = rule.pick(py, &hourly, at)? {
            picks.push(p);
        }
    }
    Ok(PyUniverse(Arc::new(Universe {
        rule: match rule {
            Rule::TopN(t) => Some(t),
            Rule::Custom { .. } => None,
        },
        picks,
    })))
}

/// What a universe rule sees at one pick: every market's hourly series up to `at`, never
/// after it.
#[pyclass(frozen, name = "PickView", module = "oxarchive_backtest")]
pub struct PickView {
    data: Arc<Vec<(String, Hourly)>>,
    at: i64,
}

impl PickView {
    fn series(&self, market: &str) -> PyResult<&Hourly> {
        self.data
            .iter()
            .find(|(n, _)| n == market)
            .map(|(_, h)| h)
            .ok_or_else(|| PyKeyError::new_err(format!("no market {market:?}")))
    }
}

#[pymethods]
impl PickView {
    /// The pick's time (ms). Only hours that ended by then are visible.
    #[getter]
    fn at(&self) -> i64 {
        self.at
    }

    /// The markets with a price before the pick.
    #[getter]
    fn markets(&self) -> Vec<String> {
        self.data
            .iter()
            .filter(|(_, h)| h.first_price().is_some_and(|f| f < self.at))
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// The last `hours` hourly closes before the pick, oldest first, None for an hour with
    /// no trade (or before the market listed).
    fn closes(&self, market: &str, hours: i64) -> PyResult<Vec<Option<f64>>> {
        let mut out = vec![None; hours.max(0) as usize];
        let start = self.at - hours * HOUR_MS;
        for h in self.series(market)?.before(self.at, hours) {
            out[((h.start - start) / HOUR_MS) as usize] = h.close;
        }
        Ok(out)
    }

    /// Dollar volume over the last `hours` hours before the pick.
    fn volume(&self, market: &str, hours: i64) -> PyResult<f64> {
        let mut v = 0.0;
        for h in self.series(market)?.before(self.at, hours) {
            v += h.volume;
        }
        Ok(v)
    }

    /// Open interest in dollars at the pick: the latest open interest times the latest
    /// close. None without either (the rule's data must hold open_interest).
    fn open_interest(&self, market: &str) -> PyResult<Option<f64>> {
        Ok(self.series(market)?.open_interest_usd(self.at))
    }

    /// The mean hourly funding rate over the last `hours` hours, of the hours that have one.
    fn funding(&self, market: &str, hours: i64) -> PyResult<Option<f64>> {
        let (mut s, mut n) = (0.0, 0);
        for h in self.series(market)?.before(self.at, hours) {
            if let Some(f) = h.funding {
                s += f;
                n += 1;
            }
        }
        Ok((n > 0).then(|| s / n as f64))
    }

    /// The sample standard deviation of hourly returns over the last `hours` hours, between
    /// consecutive hours that both traded.
    fn volatility(&self, market: &str, hours: i64) -> PyResult<Option<f64>> {
        let hs = self.series(market)?.before(self.at, hours);
        let mut rets = Vec::new();
        for w in hs.windows(2) {
            if w[1].start - w[0].start == HOUR_MS
                && let (Some(a), Some(b)) = (w[0].close, w[1].close)
            {
                rets.push(b / a - 1.0);
            }
        }
        Ok(bars::stdev(&rets))
    }

    /// When the market first traded (ms), or None if not before the pick.
    fn listed(&self, market: &str) -> PyResult<Option<i64>> {
        Ok(self.series(market)?.first_price().filter(|&f| f < self.at))
    }

    /// Hours from the market's first price to the pick; 0 if it has none yet.
    fn history_hours(&self, market: &str) -> PyResult<i64> {
        Ok(self
            .series(market)?
            .first_price()
            .map_or(0, |f| ((self.at - f) / HOUR_MS).max(0)))
    }
}

/// Writes hourly bars of the minute files in `minute_dir` to `out_dir`, one file per month
/// with the same name, every column aggregated as `History` does (oxs_engine::bars). A
/// universe rule reads these instead of the minute files: the same picks, from a sixtieth of
/// the rows. Skips a month whose hourly file is newer than its minute file. Returns the
/// number of files written.
#[pyfunction]
fn _write_hourly(minute_dir: &str, out_dir: &str) -> PyResult<usize> {
    let io = |e: std::io::Error| PyIOError::new_err(format!("{out_dir}: {e}"));
    std::fs::create_dir_all(out_dir).map_err(io)?;
    let ds = load(minute_dir, None, true, None)?;
    let mut written = 0;
    for (part, meta) in ds.parts.iter().zip(&ds.files) {
        let Part::File { path, .. } = part else {
            continue;
        };
        let out = std::path::Path::new(out_dir).join(path.file_name().expect("a file"));
        let newer = |p: &std::path::Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if let (Some(o), Some(i)) = (newer(&out), newer(path))
            && o >= i
        {
            continue;
        }
        let (_, rows) =
            oxs_dataset::read_path(path, None).map_err(|e| PyIOError::new_err(e.to_string()))?;
        let mut b = BarBuilder::new((0..FIELDS.len()).collect(), 1);
        let mut hours = Vec::new();
        let mut open: Option<i64> = None;
        for r in &rows {
            let h = r.ts - r.ts.rem_euclid(HOUR_MS);
            if let Some(o) = open
                && o != h
            {
                let bar = b.finish();
                hours.push(b.to_row(&bar, 0, o));
            }
            open = Some(h);
            b.push(0, r);
        }
        if let Some(o) = open
            && b.any()
        {
            let bar = b.finish();
            hours.push(b.to_row(&bar, 0, o));
        }
        let tmp = out.with_extension("parquet.part");
        let file = std::fs::File::create(&tmp).map_err(io)?;
        let fm = oxs_dataset::FileMeta {
            market: meta.market.clone(),
            month: meta.month.clone(),
            dataset_version: format!("{}+1h", meta.dataset_version),
        };
        oxs_dataset::write(file, &fm, &hours).map_err(|e| PyIOError::new_err(e.to_string()))?;
        std::fs::rename(&tmp, &out).map_err(io)?;
        written += 1;
    }
    Ok(written)
}

/// Writes the reference rows as one Parquet file in `dir`, so tests can run strategies
/// without real data. `market` names the file's market; `seed` gives a different market's
/// rows; `quote_volume` sets every minute's dollar volume (the reference rows have none).
#[pyfunction]
#[pyo3(signature = (dir, n, market="synthetic:reference", scale=1.0, quote_volume=None, seed=None, month="2026-10"))]
fn _write_reference(
    dir: &str,
    n: usize,
    market: &str,
    scale: f64,
    quote_volume: Option<f64>,
    seed: Option<u64>,
    month: &str,
) -> PyResult<()> {
    let io = |e: std::io::Error| PyIOError::new_err(e.to_string());
    std::fs::create_dir_all(dir).map_err(io)?;
    let file = std::fs::File::create(std::path::Path::new(dir).join(format!("{month}.parquet")))
        .map_err(io)?;
    let (mut rows, version) = match seed {
        None => (oxs_engine::reference::rows(n), format!("lcg-{n}")),
        Some(s) => (
            oxs_engine::reference::rows_seeded(n, s),
            format!("lcg-{n}-{s}"),
        ),
    };
    let meta = oxs_dataset::FileMeta {
        market: market.into(),
        month: month.into(),
        dataset_version: version,
    };
    if scale != 1.0 {
        for r in &mut rows {
            for p in [
                &mut r.open,
                &mut r.high,
                &mut r.low,
                &mut r.close,
                &mut r.mark_price,
            ] {
                *p = p.map(|x| x * scale);
            }
        }
    }
    if let Some(q) = quote_volume {
        for r in &mut rows {
            r.quote_volume = Some(q);
        }
    }
    oxs_dataset::write(file, &meta, &rows).map_err(|e| PyIOError::new_err(e.to_string()))
}

/// Writes `rows` (row dicts, in ts order, all in `month`) to `dir`/`month`.parquet as
/// `market` ("hyperliquid:BTC") at `dataset_version`: rows made elsewhere, such as hourly
/// candles for markets the dataset doesn't hold, read like any dataset file.
#[pyfunction]
fn _write_rows(
    dir: &str,
    market: &str,
    month: &str,
    dataset_version: &str,
    rows: Vec<Bound<'_, PyDict>>,
) -> PyResult<()> {
    let io = |e: std::io::Error| PyIOError::new_err(format!("{dir}: {e}"));
    std::fs::create_dir_all(dir).map_err(io)?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(row_from_dict(r)?);
    }
    if out.windows(2).any(|w| w[1].ts <= w[0].ts) {
        return Err(PyValueError::new_err("rows must be in increasing ts order"));
    }
    let meta = oxs_dataset::FileMeta {
        market: market.into(),
        month: month.into(),
        dataset_version: dataset_version.into(),
    };
    let tmp = std::path::Path::new(dir).join(format!("{month}.parquet.part"));
    let file = std::fs::File::create(&tmp).map_err(io)?;
    oxs_dataset::write(file, &meta, &out).map_err(|e| PyIOError::new_err(e.to_string()))?;
    std::fs::rename(
        &tmp,
        std::path::Path::new(dir).join(format!("{month}.parquet")),
    )
    .map_err(io)
}

#[pyfunction]
fn engine_version() -> &'static str {
    oxs_engine::ENGINE_VERSION
}

/// Runs the Rust reference backtest and returns (fills, result hash as hex).
#[pyfunction]
fn reference() -> (usize, String) {
    let r = oxs_engine::reference::run_reference();
    (r.fills.len(), format!("{:016x}", r.result_hash))
}

/// The byte ranges [start, end) a read of `columns` needs from a Parquet file of `size`
/// bytes whose last bytes are `tail`: the footer and those columns' chunks, gaps under
/// `merge` bytes joined. An int instead when the footer needs a longer tail (that many
/// bytes). A browser fetches these with HTTP range requests instead of the whole file.
#[pyfunction]
#[pyo3(signature = (tail, size, columns, merge=16_384))]
fn _column_ranges<'py>(
    py: Python<'py>,
    tail: &[u8],
    size: u64,
    columns: Vec<String>,
    merge: u64,
) -> PyResult<Bound<'py, PyAny>> {
    let cols: Vec<&str> = columns.iter().map(String::as_str).collect();
    match oxs_dataset::column_ranges(tail, size, &cols, merge) {
        Ok(r) => Ok(r.into_pyobject(py)?.into_any()),
        Err(Ok(need)) => Ok(need.into_pyobject(py)?.into_any()),
        Err(Err(e)) => Err(PyIOError::new_err(e.to_string())),
    }
}

/// The stats of an equity curve, [(ms, equity)], as `Report.stats` gives its own (the
/// curve's part: return, annual_return, volatility, sharpe, sortino, max_drawdown, calmar,
/// hit_rate). None for an empty curve.
#[pyfunction]
fn _stats<'py>(py: Python<'py>, curve: Vec<(i64, f64)>) -> PyResult<Option<Bound<'py, PyDict>>> {
    let Some(s) = stats(&curve) else {
        return Ok(None);
    };
    let d = PyDict::new(py);
    d.set_item("start", s.start)?;
    d.set_item("end", s.end)?;
    d.set_item("return", s.ret)?;
    d.set_item("annual_return", s.annual_return)?;
    d.set_item("volatility", s.volatility)?;
    d.set_item("sharpe", s.sharpe)?;
    d.set_item("sortino", s.sortino)?;
    d.set_item("max_drawdown", s.max_drawdown)?;
    d.set_item("calmar", s.calmar)?;
    d.set_item("hit_rate", s.hit_rate)?;
    Ok(Some(d))
}

/// Every column a history can read, and the columns the fill model reads.
#[pyfunction]
fn _columns() -> (Vec<&'static str>, Vec<&'static str>) {
    (
        FIELDS.iter().map(|(n, _)| *n).collect(),
        vec![
            "open",
            "high",
            "low",
            "close",
            "mark_price",
            "funding_settled",
            "spread_bps",
            "depth_bid_10bps",
            "depth_ask_10bps",
            "depth_bid_50bps",
            "depth_ask_50bps",
        ],
    )
}

/// The minute-row columns a history of `fields` needs.
#[pyfunction]
fn _columns_for(fields: Vec<String>) -> PyResult<Vec<&'static str>> {
    let mut ids = Vec::new();
    for f in fields {
        ids.push(
            bars::field_id(&f).ok_or_else(|| PyKeyError::new_err(format!("no column {f:?}")))?,
        );
    }
    Ok(bars::columns_for(&ids))
}

/// The compiled half of the package; python/oxarchive_backtest/__init__.py re-exports it.
/// Runs the API dataset builder (`0xbt data pull`): the same arguments as the
/// `0xbt-build-api` binary, program name first. Returns its exit code.
#[pyfunction]
fn _build_api(py: Python<'_>, args: Vec<String>) -> i32 {
    py.detach(|| oxs_build_api::run(args))
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_submodule(&math::module(m.py())?)?;
    m.add_class::<Dataset>()?;
    m.add_class::<PyConfig>()?;
    m.add_class::<PyFill>()?;
    m.add_class::<PyOrder>()?;
    m.add_class::<Ctx>()?;
    m.add_class::<PyHistory>()?;
    m.add_class::<PyReport>()?;
    m.add_class::<PyReceipt>()?;
    m.add_class::<Session>()?;
    m.add_class::<PyUniverse>()?;
    m.add_class::<PickView>()?;
    m.add_function(wrap_pyfunction!(load, m)?)?;
    m.add_function(wrap_pyfunction!(_build_api, m)?)?;
    m.add_function(wrap_pyfunction!(_backtest, m)?)?;
    m.add_function(wrap_pyfunction!(_picks, m)?)?;
    m.add_function(wrap_pyfunction!(_write_hourly, m)?)?;
    m.add_function(wrap_pyfunction!(_write_reference, m)?)?;
    m.add_function(wrap_pyfunction!(_columns, m)?)?;
    m.add_function(wrap_pyfunction!(_stats, m)?)?;
    m.add_function(wrap_pyfunction!(_column_ranges, m)?)?;
    m.add_function(wrap_pyfunction!(_write_rows, m)?)?;
    m.add_function(wrap_pyfunction!(_columns_for, m)?)?;
    m.add_function(wrap_pyfunction!(_last_tick, m)?)?;
    m.add_function(wrap_pyfunction!(_is_tick, m)?)?;
    m.add_function(wrap_pyfunction!(_local, m)?)?;
    m.add_function(wrap_pyfunction!(_session, m)?)?;
    m.add_function(wrap_pyfunction!(engine_version, m)?)?;
    m.add_function(wrap_pyfunction!(reference, m)?)?;
    m.add("FILL_MODEL", oxs_engine::FILL_MODEL)?;
    Ok(())
}
