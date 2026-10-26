//! Builds the 1-minute dataset from the public 0xArchive API, with the user's own key.
//!
//! Files are tagged dataset version `api-v0`. Differences from the schema:
//! - Book and liquidation-level columns are null: the API has no 1-minute form of them.
//! - Context columns (OI, mark, funding...) are the API's 1-minute averages, not the
//!   last snapshot.
//! - A minute missing from candles is null, not zero: the API can't tell a quiet
//!   minute from a collection gap.
//! - `funding_settled` on each hour's first row is the previous minute's funding estimate.
//!
//! The key is read from `OXARCHIVE_API_KEY` (or `OXA_API_KEY`) and never printed.
//!
//!   0xbt-build-api build --coin BTC --from 2025-10 --to 2026-09 --out data
//!   (months with no candles, such as before a market listed, get no file;
//!   --skip-existing leaves months that already have a file; OXS_BUILD_THREADS sets the
//!   requests in flight, default 16)
//!   0xbt-build-api build --coin BTC --from 2026-10 --to 2026-10 --until 1791300000000 --out recent
//!   (a month to date: rows stop before --until, ms; keep it out of the published dataset,
//!   since candles are rewritten for up to 24 h)
//!   0xbt-build-api build --coin xyz:CL --from 2026-01 --to 2026-09 --out data
//!   (a HIP-3 market, named dex:coin as the API names it: read from /v1/hyperliquid/hip3
//!   and written to data/hyperliquid/xyz/CL/, since a colon can't be in a Windows path)
//!   0xbt-build-api check --dir data/hyperliquid/BTC
//!   0xbt-build-api manifest --dir data        (writes data/manifest.json; see manifest.rs)

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use oxs_dataset::FileMeta;
use oxs_schema::{BUCKET_MS, Row, Source};
use serde_json::Value;

const BASE: &str = "https://api.0xarchive.io/v1/hyperliquid";
const DATASET_VERSION: &str = "api-v0";
const HOUR_MS: i64 = 3_600_000;

type Res<T> = Result<T, String>;

/// Runs one command as the `0xbt-build-api` binary would: `args[0]` is the program name.
/// Returns the exit code, printing any error to stderr.
pub fn run(args: Vec<String>) -> i32 {
    match dispatch(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

fn dispatch(args: &[String]) -> Res<()> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let need = |name: &str, what: &str| flag(name).ok_or_else(|| format!("{name} {what} is required"));
    match args.get(1).map(String::as_str) {
        Some("build") => build(
            &flag("--coin").unwrap_or_else(|| "BTC".into()),
            &need("--from", "YYYY-MM")?,
            &need("--to", "YYYY-MM")?,
            Path::new(&flag("--out").unwrap_or_else(|| "data".into())),
            flag("--until")
                .map(|u| u.parse::<i64>().map_err(|_| format!("--until {u}: not a ms timestamp")))
                .transpose(),
            args.iter().any(|a| a == "--skip-existing"),
        ),
        Some("check") => check(Path::new(&need("--dir", "DIR")?)),
        Some("manifest") => manifest(
            Path::new(&flag("--dir").unwrap_or_else(|| "data".into())),
            flag("--out").map(PathBuf::from),
        ),
        _ => Err("usage: 0xbt-build-api build --coin BTC --from YYYY-MM --to YYYY-MM [--out data] [--until MS] [--skip-existing] | check --dir DIR | manifest [--dir data] [--out FILE]".into()),
    }
}

// ---------- dates, no external crate ----------

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn date_ms(s: &str) -> Res<i64> {
    let p: Vec<i64> = s
        .split('-')
        .map(|x| x.parse::<i64>().map_err(|_| format!("bad date {s}")))
        .collect::<Res<_>>()?;
    let d = if p.len() == 3 { p[2] } else { 1 };
    Ok(days_from_civil(p[0], p[1], d) * 86_400_000)
}

/// Parses `2026-10-05T14:00:00Z` or `2026-10-05T14:00:00.000Z`, or a number of ms.
fn ts_ms(v: &Value) -> Res<i64> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let s = v
        .as_str()
        .ok_or("timestamp is neither a number nor a string")?;
    let (date, time) = s
        .split_once('T')
        .ok_or_else(|| format!("bad timestamp {s}"))?;
    let time = time.trim_end_matches('Z');
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let h: Vec<i64> = hms
        .split(':')
        .map(|x| x.parse().map_err(|_| format!("bad timestamp {s}")))
        .collect::<Res<_>>()?;
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)])
        .parse()
        .map_err(|_| format!("bad timestamp {s}"))?;
    Ok(date_ms(date)? + ((h[0] * 60 + h[1]) * 60 + h[2]) * 1000 + ms)
}

fn month_bounds(ym: &str) -> Res<(i64, i64)> {
    let (y, m) = ym
        .split_once('-')
        .ok_or_else(|| format!("bad month {ym}"))?;
    let (y, m): (i64, i64) = (
        y.parse().map_err(|_| "bad year")?,
        m.parse().map_err(|_| "bad month")?,
    );
    let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    Ok((
        days_from_civil(y, m, 1) * 86_400_000,
        days_from_civil(ny, nm, 1) * 86_400_000,
    ))
}

fn months(from: &str, to: &str) -> Res<Vec<String>> {
    let parse = |s: &str| -> Res<(i64, i64)> {
        let (y, m) = s.split_once('-').ok_or_else(|| format!("bad month {s}"))?;
        Ok((
            y.parse().map_err(|_| "bad year")?,
            m.parse().map_err(|_| "bad month")?,
        ))
    };
    let (mut y, mut m) = parse(from)?;
    let end = parse(to)?;
    let mut out = Vec::new();
    while (y, m) <= end {
        out.push(format!("{y:04}-{m:02}"));
        (y, m) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    }
    Ok(out)
}

// ---------- API ----------

struct Api {
    agent: ureq::Agent,
    key: String,
    requests: AtomicU64,
    threads: usize,
}

impl Api {
    fn new() -> Res<Self> {
        let key = std::env::var("OXARCHIVE_API_KEY")
            .or_else(|_| std::env::var("OXA_API_KEY"))
            .map_err(|_| "OXARCHIVE_API_KEY is not set")?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(90)))
            .build()
            .into();
        // Requests in flight at once. The API served 120 at once without a 429 (2026-10-07);
        // 16 a market keeps a three-market backfill near 50.
        let threads = std::env::var("OXS_BUILD_THREADS")
            .ok()
            .and_then(|t| t.parse().ok())
            .unwrap_or(16);
        Ok(Api {
            agent,
            key,
            requests: AtomicU64::new(0),
            threads,
        })
    }

    fn get(&self, url: &str) -> Res<Value> {
        let mut wait = 2;
        for _ in 0..8 {
            self.requests.fetch_add(1, Ordering::Relaxed);
            match self.agent.get(url).header("X-API-Key", &self.key).call() {
                Ok(mut resp) => {
                    let status = resp.status().as_u16();
                    let body = resp
                        .body_mut()
                        .read_to_string()
                        .map_err(|e| e.to_string())?;
                    if status == 200 {
                        return serde_json::from_str(&body)
                            .map_err(|e| format!("bad JSON from {url}: {e}"));
                    }
                    if status != 429 && status < 500 {
                        return Err(format!(
                            "{status} from {url}: {}",
                            &body[..body.len().min(300)]
                        ));
                    }
                }
                Err(e) => eprintln!("  retrying after transport error: {e}"),
            }
            std::thread::sleep(Duration::from_secs(wait));
            wait = (wait * 2).min(60);
        }
        Err(format!("gave up on {url}"))
    }

    /// Every 1-minute bucket in [start, end), paging by moving `start` past the last
    /// bucket returned. That works the same on every endpoint, whatever its cursor rules.
    fn window(&self, path: &str, start: i64, end: i64, limit: i64) -> Res<Vec<(i64, Value)>> {
        let mut out = Vec::new();
        let mut s = start;
        while s < end {
            let url = format!(
                "{BASE}/{path}{}interval=1m&start={s}&end={end}&limit={limit}",
                if path.contains('?') { '&' } else { '?' }
            );
            let mut page = self.get(&url)?;
            let data = page["data"]
                .as_array_mut()
                .ok_or_else(|| format!("no data array from {path}"))?;
            if data.is_empty() {
                break;
            }
            let mut last = s;
            for item in data.drain(..) {
                let t = ts_ms(&item["timestamp"])?;
                if t >= start && t < end {
                    out.push((t, item));
                }
                last = last.max(t);
            }
            if last < s {
                break;
            }
            s = last + BUCKET_MS;
        }
        Ok(out)
    }

    /// Every bucket of each (path, start, end), fetched in parallel: each range is cut into
    /// windows of one full page (the page caps differ by endpoint), and `threads` workers
    /// take windows from a shared list. A window that comes back short still pages.
    fn buckets(&self, ranges: &[(String, i64, i64)]) -> Res<Vec<HashMap<i64, Value>>> {
        let mut jobs = Vec::new();
        for (k, (path, start, end)) in ranges.iter().enumerate() {
            let limit = page_cap(path);
            let mut s = *start;
            while s < *end {
                let e = (s + limit * BUCKET_MS).min(*end);
                jobs.push((k, s, e, limit));
                s = e;
            }
        }
        let next = AtomicUsize::new(0);
        let out: Mutex<Vec<HashMap<i64, Value>>> =
            Mutex::new(ranges.iter().map(|_| HashMap::new()).collect());
        let failed: Mutex<Option<String>> = Mutex::new(None);
        std::thread::scope(|scope| {
            for _ in 0..self.threads.min(jobs.len()).max(1) {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(k, s, e, limit)) = jobs.get(i) else {
                            break;
                        };
                        if failed.lock().unwrap().is_some() {
                            break;
                        }
                        match self.window(&ranges[k].0, s, e, limit) {
                            Ok(items) => out.lock().unwrap()[k].extend(items),
                            Err(err) => {
                                failed.lock().unwrap().get_or_insert(err);
                            }
                        }
                    }
                });
            }
        });
        if let Some(err) = failed.into_inner().unwrap() {
            return Err(err);
        }
        Ok(out.into_inner().unwrap())
    }
}

/// The most rows one request returns, by endpoint (measured 2026-10-07; larger `limit`s
/// are cut to these).
fn page_cap(path: &str) -> i64 {
    let path = path.strip_prefix("hip3/").unwrap_or(path);
    if path.starts_with("candles/") || path.ends_with("/flow") {
        10_000
    } else if path.starts_with("cvd/") {
        1_440
    } else {
        1_000
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn int(v: &Value) -> Option<i32> {
    num(v).map(|x| x as i32)
}

// ---------- build ----------

fn build(
    coin: &str,
    from: &str,
    to: &str,
    out: &Path,
    until: Res<Option<i64>>,
    skip_existing: bool,
) -> Res<()> {
    let until = until?;
    let api = Api::new()?;
    let liq_start = date_ms(Source::Liquidations.hyperliquid_perp_start())?;
    let orders_start = date_ms(Source::Orders.hyperliquid_perp_start())?;
    // The candles route refuses a range that ends before 2025-03-22 12:00 UTC and clamps
    // one that straddles it, so each page-sized window starts there at the earliest.
    let candles_start = date_ms(Source::Fills.hyperliquid_perp_start())? + 12 * HOUR_MS;
    // A HIP-3 market (dex:coin) reads the same routes under hip3/ and lives one folder
    // deeper, hyperliquid/<dex>/<coin>/.
    let (route, dir) = match coin.split_once(':') {
        Some((dex, name)) => ("hip3/", out.join("hyperliquid").join(dex).join(name)),
        None => ("", out.join("hyperliquid").join(coin)),
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    for ym in months(from, to)? {
        let (start, end) = month_bounds(&ym)?;
        // Whole minutes only, so a month to date ends on its last closed minute.
        let end = until.map_or(end, |u| end.min(u - u.rem_euclid(BUCKET_MS)));
        if end <= start {
            continue;
        }
        let t = std::time::Instant::now();
        if skip_existing && dir.join(format!("{ym}.parquet")).exists() {
            println!("{ym}: exists, skipped");
            continue;
        }
        let [candles, cvd, flow, oi, funding, liqs]: [HashMap<i64, Value>; 6] = api
            .buckets(&[
                (format!("{route}candles/{coin}"), start.max(candles_start), end),
                (format!("{route}cvd/{coin}"), start, end),
                (format!("{route}orders/{coin}/flow"), start.max(orders_start), end),
                (format!("{route}openinterest/{coin}"), start, end),
                // One extra minute before the month, for the first hour's settlement.
                (format!("{route}funding/{coin}"), start - BUCKET_MS, end),
                // The API refuses ranges that end before a source's first date.
                (
                    format!("{route}liquidations/{coin}/volume"),
                    start.max(liq_start),
                    end,
                ),
            ])?
            .try_into()
            .map_err(|_| "six ranges in, six out")?;
        // A month before the market listed (or before fills exist) has no prices, so a
        // backtest can't use it: no file, rather than a month of nulls.
        if candles.is_empty() {
            println!("{ym}: no candles, skipped");
            continue;
        }

        let mut rows = Vec::with_capacity(((end - start) / BUCKET_MS) as usize);
        let mut ts = start;
        while ts < end {
            let mut r = Row {
                ts,
                ..Row::default()
            };
            if let Some(c) = candles.get(&ts) {
                r.open = num(&c["open"]);
                r.high = num(&c["high"]);
                r.low = num(&c["low"]);
                r.close = num(&c["close"]);
                r.volume = num(&c["volume"]);
                r.quote_volume = num(&c["quote_volume"]);
                r.trade_count = int(&c["trade_count"]);
                r.vwap = match (r.quote_volume, r.volume) {
                    (Some(q), Some(v)) if v > 0.0 => Some(q / v),
                    _ => None,
                };
            }
            if let Some(c) = cvd.get(&ts) {
                r.taker_buy_quote = num(&c["buy_volume"]);
                r.taker_sell_quote = num(&c["sell_volume"]);
                if let (Some(b), Some(s)) = (r.taker_buy_quote, r.taker_sell_quote) {
                    r.cvd_delta = Some(b - s);
                }
            }
            if ts >= orders_start
                && let Some(f) = flow.get(&ts)
            {
                r.limit_orders_placed = int(&f["limit_orders_placed"]);
                r.orders_canceled = int(&f["orders_canceled"]);
                r.orders_filled = int(&f["orders_filled"]);
                r.trigger_orders_placed = int(&f["trigger_orders_placed"]);
                r.tpsl_orders_placed = int(&f["tpsl_orders_placed"]);
                r.orders_triggered = int(&f["orders_triggered"]);
                r.orders_force_canceled = int(&f["orders_force_canceled"]);
            }
            if let Some(o) = oi.get(&ts) {
                r.open_interest = num(&o["open_interest"]);
                r.mark_price = num(&o["mark_price"]);
                r.oracle_price = num(&o["oracle_price"]);
                r.mid_price = num(&o["mid_price"]);
                r.impact_bid = num(&o["impact_bid_price"]);
                r.impact_ask = num(&o["impact_ask_price"]);
            }
            if let Some(f) = funding.get(&ts) {
                r.funding_rate = num(&f["funding_rate"]);
                r.premium = num(&f["premium"]);
            }
            if ts.rem_euclid(HOUR_MS) == 0 {
                r.funding_settled = funding
                    .get(&(ts - BUCKET_MS))
                    .and_then(|f| num(&f["funding_rate"]));
            }
            if ts >= liq_start {
                // The endpoint omits minutes with no liquidations: those are zeros.
                let l = liqs.get(&ts);
                let f = |k: &str| Some(l.and_then(|l| num(&l[k])).unwrap_or(0.0));
                let i = |k: &str| Some(l.and_then(|l| int(&l[k])).unwrap_or(0));
                r.liq_long_usd = f("long_usd");
                r.liq_short_usd = f("short_usd");
                r.liq_long_count = i("long_count");
                r.liq_short_count = i("short_count");
            }
            rows.push(r);
            ts += BUCKET_MS;
        }

        // Written beside and renamed into place, so an interrupted build never leaves a
        // truncated file that --skip-existing would keep.
        let path = dir.join(format!("{ym}.parquet"));
        let part = dir.join(format!("{ym}.parquet.part"));
        let file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let meta = FileMeta {
            market: format!("hyperliquid:{coin}"),
            month: ym.clone(),
            dataset_version: DATASET_VERSION.into(),
        };
        oxs_dataset::write(file, &meta, &rows).map_err(|e| e.to_string())?;
        std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        println!(
            "{ym}: {} rows, {} with candles, {:.1} MB, {:.0}s, {} requests so far",
            rows.len(),
            candles.len(),
            size as f64 / 1e6,
            t.elapsed().as_secs_f64(),
            api.requests.load(Ordering::Relaxed)
        );
    }
    Ok(())
}

// ---------- manifest ----------

/// Every `VENUE/MARKET/*.parquet` under `dir`, as one manifest. Fails if the files are of
/// more than one dataset or schema version.
fn manifest(dir: &Path, out: Option<PathBuf>) -> Res<()> {
    use oxs_dataset::manifest::{
        FileEntry, MANIFEST_VERSION, Manifest, MarketEntry, first_values, utc_date,
    };
    use sha2::{Digest, Sha256};
    use std::collections::BTreeMap;

    let list = |d: &Path| -> Res<Vec<PathBuf>> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .map_err(|e| format!("{}: {e}", d.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        v.sort();
        Ok(v)
    };
    let mut markets: BTreeMap<String, MarketEntry> = BTreeMap::new();
    let mut versions = std::collections::BTreeSet::new();
    for venue in list(dir)?.into_iter().filter(|p| p.is_dir()) {
        for market_dir in list(&venue)?.into_iter().filter(|p| p.is_dir()) {
            for f in list(&market_dir)?
                .into_iter()
                .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
            {
                let raw = std::fs::read(&f).map_err(|e| e.to_string())?;
                let sha: String = Sha256::digest(&raw)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                let bytes = bytes::Bytes::from(raw);
                let (kv, rows) =
                    oxs_dataset::read(bytes.clone(), Some(&[])).map_err(|e| e.to_string())?;
                let meta = FileMeta::from_kv(&kv)
                    .ok_or_else(|| format!("{} has no dataset metadata", f.display()))?;
                versions.insert((
                    meta.dataset_version.clone(),
                    kv.get("oxs.schema_version").cloned().unwrap_or_default(),
                ));
                let firsts = first_values(bytes).map_err(|e| e.to_string())?;
                let path = f
                    .strip_prefix(dir)
                    .map_err(|e| e.to_string())?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let m = markets
                    .entry(meta.market.clone())
                    .or_insert_with(|| MarketEntry {
                        market: meta.market.clone(),
                        rows: 0,
                        files: Vec::new(),
                        column_starts: BTreeMap::new(),
                    });
                m.rows += rows.len() as u64;
                m.files.push(FileEntry {
                    month: meta.month,
                    path,
                    bytes: bytes_len(&f)?,
                    rows: rows.len() as u64,
                    sha256: sha,
                });
                // Files are read oldest first, so the first value seen is the earliest.
                for (col, ts) in firsts {
                    let slot = m.column_starts.entry(col).or_insert(None);
                    if slot.is_none() {
                        *slot = ts.map(utc_date);
                    }
                }
            }
        }
    }
    if versions.len() != 1 {
        return Err(format!(
            "expected one dataset and schema version under {}, found {versions:?}",
            dir.display()
        ));
    }
    let (dataset_version, schema_version) = versions.into_iter().next().unwrap();
    let manifest = Manifest {
        manifest_version: MANIFEST_VERSION,
        schema_version,
        dataset_version,
        markets: markets.into_values().collect(),
    };
    let out = out.unwrap_or_else(|| dir.join("manifest.json"));
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(&out, json + "\n").map_err(|e| e.to_string())?;
    let files: usize = manifest.markets.iter().map(|m| m.files.len()).sum();
    println!(
        "{}: {} markets, {files} files, dataset {}",
        out.display(),
        manifest.markets.len(),
        manifest.dataset_version
    );
    Ok(())
}

fn bytes_len(p: &Path) -> Res<u64> {
    std::fs::metadata(p)
        .map(|m| m.len())
        .map_err(|e| e.to_string())
}

// ---------- check ----------

fn check(dir: &Path) -> Res<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "parquet"))
        .collect();
    files.sort();
    let mut rows = Vec::new();
    for f in &files {
        let (_, r) = oxs_dataset::read(std::fs::File::open(f).map_err(|e| e.to_string())?, None)
            .map_err(|e| e.to_string())?;
        rows.extend(r);
    }
    let mut problems: Vec<String> = Vec::new();
    let mut flag = |msg: String| {
        if problems.len() < 20 {
            problems.push(msg);
        }
    };
    let liq_start = date_ms(Source::Liquidations.hyperliquid_perp_start())?;
    let orders_start = date_ms(Source::Orders.hyperliquid_perp_start())?;
    let fills_start = date_ms(Source::Fills.hyperliquid_perp_start())?;
    let eps = 1e-9;

    for (i, r) in rows.iter().enumerate() {
        // Contiguous grid.
        if i > 0 && r.ts != rows[i - 1].ts + BUCKET_MS {
            flag(format!(
                "grid break at {}: previous {}",
                r.ts,
                rows[i - 1].ts
            ));
        }
        // Prices are all present or all absent, and consistent.
        let ohlc = [r.open, r.high, r.low, r.close];
        if ohlc.iter().any(Option::is_some) && ohlc.iter().any(Option::is_none) {
            flag(format!("{}: partial OHLC", r.ts));
        }
        if let [Some(o), Some(h), Some(l), Some(c)] = ohlc {
            if h + eps < o.max(c) || l - eps > o.min(c) {
                flag(format!("{}: high/low don't contain open/close", r.ts));
            }
            if let Some(v) = r.vwap
                && (v > h * (1.0 + 1e-9) || v < l * (1.0 - 1e-9))
            {
                flag(format!("{}: vwap {v} outside [{l}, {h}]", r.ts));
            }
        }
        if r.volume.is_some_and(|v| v < 0.0) || r.trade_count.is_some_and(|c| c < 0) {
            flag(format!("{}: negative volume or count", r.ts));
        }
        if let (Some(b), Some(s), Some(d)) = (r.taker_buy_quote, r.taker_sell_quote, r.cvd_delta)
            && (b - s - d).abs() > eps
        {
            flag(format!("{}: cvd_delta != buy - sell", r.ts));
        }
        // No lookahead in the one column this builder derives across rows: the settled
        // rate sits only on an hour's first row and comes from the row before it.
        if let Some(fs) = r.funding_settled {
            if r.ts.rem_euclid(HOUR_MS) != 0 {
                flag(format!("{}: funding_settled off the hour", r.ts));
            }
            if i > 0 && rows[i - 1].funding_rate != Some(fs) {
                flag(format!(
                    "{}: funding_settled doesn't match the previous minute's estimate",
                    r.ts
                ));
            }
        }
        // Nothing from a source before it exists.
        if r.ts < fills_start && (r.close.is_some() || r.volume.is_some()) {
            flag(format!("{}: price data before fills start", r.ts));
        }
        if r.ts < orders_start && r.limit_orders_placed.is_some() {
            flag(format!("{}: order counts before orders start", r.ts));
        }
        if r.ts < liq_start && r.liq_long_usd.is_some() {
            flag(format!("{}: liquidations before liquidations start", r.ts));
        }
        // Columns the API build never fills.
        if r.best_bid.is_some() || r.liq_levels_long_1pct_usd.is_some() {
            flag(format!(
                "{}: book or liq-level column filled in an api-v0 build",
                r.ts
            ));
        }
    }

    let n = rows.len() as f64;
    let pct = |f: &dyn Fn(&Row) -> bool| 100.0 * rows.iter().filter(|r| f(r)).count() as f64 / n;
    println!(
        "{} files, {} rows, {} to {}",
        files.len(),
        rows.len(),
        rows.first().map_or(0, |r| r.ts),
        rows.last().map_or(0, |r| r.ts)
    );
    println!("  with candles:       {:6.2}%", pct(&|r| r.close.is_some()));
    println!(
        "  with taker flow:    {:6.2}%",
        pct(&|r| r.cvd_delta.is_some())
    );
    println!(
        "  with order counts:  {:6.2}%",
        pct(&|r| r.limit_orders_placed.is_some())
    );
    println!(
        "  with OI and mark:   {:6.2}%",
        pct(&|r| r.mark_price.is_some())
    );
    println!(
        "  with funding:       {:6.2}%",
        pct(&|r| r.funding_rate.is_some())
    );
    println!(
        "  with liquidations:  {:6.2}%",
        pct(&|r| r.liq_long_usd.is_some())
    );
    let hours = rows
        .iter()
        .filter(|r| r.ts.rem_euclid(HOUR_MS) == 0)
        .count();
    let settled = rows.iter().filter(|r| r.funding_settled.is_some()).count();
    println!("  hours with a settled funding rate: {settled} of {hours}");
    if problems.is_empty() {
        println!("OK: no problems found");
        Ok(())
    } else {
        for p in &problems {
            println!("  problem: {p}");
        }
        Err(format!("{} or more problems", problems.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_ms("1970-01-01").unwrap(), 0);
        assert_eq!(
            ts_ms(&Value::from("2026-10-05T14:00:00Z")).unwrap(),
            1_791_208_800_000
        );
        assert_eq!(
            ts_ms(&Value::from("2026-10-05T14:00:00.500Z")).unwrap(),
            1_791_208_800_500
        );
        assert_eq!(
            ts_ms(&Value::from(1_791_208_800_000i64)).unwrap(),
            1_791_208_800_000
        );
        assert_eq!(
            month_bounds("2024-02").unwrap().1 - month_bounds("2024-02").unwrap().0,
            29 * 86_400_000
        );
        assert_eq!(
            months("2025-11", "2026-02").unwrap(),
            ["2025-11", "2025-12", "2026-01", "2026-02"]
        );
    }
}
