#!/usr/bin/env python3
"""Reverse engineering a trader: pull one wallet's positions, profile its style, and score how
much of its book a candidate strategy reproduces.

    trader.py fetch 0xADDRESS --start 2025-06-01 --end 2026-07-01 [--venue hyperliquid]
              [--data DIR] [--out trader.json]
        Every fill that changed the wallet's positions (GET .../positions/changes), paged
        and split into 30-day requests, written as one file the mirror strategy reads.
        Needs $OXARCHIVE_API_KEY. Billed like trades: one credit per 1,000 rows.
        --data marks which markets the backtest dataset has (in_dataset).

    trader.py profile trader.json
        The style, from the fills alone: markets, round trips, holding times, long and
        short time, size, maker share, when it opens (UTC hour and weekday), realized PnL.

        Includes "scaling": orders per round trip, where each add sits against the first
        fill and its size against what was held, and the exit against the average entry.

    trader.py entries trader.json [--data DIR] [--lengths 6,8,12,14,16,20,24,48,72]
        Which reference level the opening fills track: each one measured against hourly
        EMAs (closed and live), SMAs and the session VWAP, ranked by how tightly the
        offsets cluster. Reads the backtest dataset; needs the 0xarchive-backtest wheel.

    trader.py grid trader.json [--join-minutes 15] [--min-orders 20]
        Whether a market is traded as a grid: fills joined into orders (same side and price,
        close in time), then the most common order size, the most common price step from one
        order to the next, the share of orders and steps that match them, the share of fills
        on that price grid, and the range the position moved in. A book that holds a position
        and trades a fixed size at fixed price steps around it shows near 1.0 on all three.

    trader.py replicate trader.json CANDIDATE.py --start DATE --end DATE [--data DIR]
              [-p NAME=VALUE ...] [-c NAME=VALUE ...] [--clock 1h]
        Backtests the trader's book (templates/mirror.py) and the candidate over the same
        window, then compares them hour by hour: how many of the trader's held hours the
        candidate is on the same side, on the other side, or flat, and how much of the
        trader's daily returns (in the engine, after costs) it tracks. Needs the 0xarchive-backtest
        wheel. Both runs go into an active study's ledger as trials.

fetch, profile and grid use only the standard library; entries and replicate need the engine.
"""

import argparse
import datetime as dt
import json
import os
import pathlib
import statistics
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.0xarchive.io"
VERSION = "2026-10-01"
HERE = pathlib.Path(__file__).resolve().parent
# The mirror strategy: beside this script when both were copied into a study, else the
# skill's templates/.
MIRROR = next((p for p in (HERE / "mirror.py", HERE.parent / "templates" / "mirror.py")
               if p.is_file()), HERE.parent / "templates" / "mirror.py")
DAY = 86_400_000
HOUR = 3_600_000
PREFIX = {
    "hyperliquid": "/v1/hyperliquid/wallets/{a}/positions/changes",
    "hip3": "/v1/hyperliquid/hip3/wallets/{a}/positions/changes",
    "lighter": "/v1/lighter/accounts/{a}/positions/changes",
}


def ms(text):
    """Unix ms from ms digits, a date or an RFC 3339 time (UTC)."""
    if isinstance(text, (int, float)):
        return int(text)
    if text.isdigit():
        return int(text)
    t = dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
    if t.tzinfo is None:
        t = t.replace(tzinfo=dt.timezone.utc)
    return int(t.timestamp() * 1000)


def iso(t):
    return dt.datetime.fromtimestamp(t / 1000, dt.timezone.utc).strftime("%Y-%m-%d %H:%M")


def num(v):
    return None if v is None else float(v)


def get(path, params, key):
    url = API + path + "?" + urllib.parse.urlencode(params)
    req = urllib.request.Request(url, headers={"x-api-key": key, "0xArchive-Version": VERSION})
    for attempt in range(5):
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r)
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", "replace")
            try:
                err = json.loads(body)
            except json.JSONDecodeError:
                err = {"error": body[:300]}
            code = err.get("error_code")
            if e.code in (429, 500, 503) and code != "insufficient_credits" and attempt < 4:
                time.sleep(float(e.headers.get("Retry-After") or 2 ** attempt))
                continue
            raise SystemExit(f"{e.code} {code}: {err.get('error')} (request_id {err.get('request_id')})")
    raise SystemExit("gave up after retries")


def cmd_fetch(args):
    key = os.environ.get("OXARCHIVE_API_KEY")
    if not key:
        raise SystemExit("set OXARCHIVE_API_KEY (https://www.0xarchive.io/signup)")
    path = PREFIX[args.venue].format(a=args.address)
    t0, t1 = ms(args.start), ms(args.end)
    rows, notices = [], set()
    # One request window per 30 days: the Free plan's per-request span.
    for w0 in range(t0, t1, 30 * DAY):
        w1 = min(w0 + 30 * DAY, t1)
        cursor = None
        while True:
            params = {"start": w0, "end": w1, "limit": 5000}
            if cursor:
                params["cursor"] = cursor
            page = get(path, params, key)
            rows.extend(page["data"])
            meta = page.get("meta", {})
            if meta.get("notice"):
                notices.add(meta["notice"])
            if not meta.get("has_more"):
                break
            cursor = meta["next_cursor"]
        print(f"{iso(w0)[:10]} to {iso(w1)[:10]}: {len(rows)} fills so far", file=sys.stderr)
    changes, gross, held, mismatched = [], 0.0, {}, 0
    for r in rows:
        end_pos = num(r.get("end_position"))
        if end_pos is None:
            continue
        d = r.get("direction") or ""
        # end_position is signed (short negative); the venue's direction text says the same.
        if end_pos > 0 and d.endswith("Short") or end_pos < 0 and d.endswith("Long"):
            mismatched += 1
        c = {"ts": ms(r["timestamp"]), "symbol": r["symbol"], "end_position": end_pos,
             "start_position": num(r.get("start_position")), "price": num(r.get("price")),
             "size": num(r.get("size")), "side": r.get("side"), "direction": d or None,
             "closed_pnl": num(r.get("closed_pnl")), "fee": num(r.get("fee")),
             "maker": (not r["crossed"]) if r.get("crossed") is not None else r.get("is_maker")}
        changes.append(c)
        if c["price"]:
            held[c["symbol"]] = abs(end_pos) * c["price"]
            g = 0.0
            for v in held.values():
                g += v
            gross = max(gross, g)
    changes.sort(key=lambda c: (c["ts"], c["symbol"]))
    book = {"address": args.address, "venue": args.venue, "start": t0, "end": t1,
            "fetched": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "max_gross_usd": gross, "changes": changes, "notices": sorted(notices)}
    if args.data:
        have = {p.name for p in pathlib.Path(args.data).iterdir() if p.is_dir()}
        book["in_dataset"] = sorted({c["symbol"] for c in changes} & have)
    pathlib.Path(args.out).write_text(json.dumps(book) + "\n", encoding="utf-8")
    print(f"{len(changes)} position changes in {len({c['symbol'] for c in changes})} markets "
          f"written to {args.out}; largest gross position ${gross:,.0f}")
    for n in sorted(notices):
        print(f"notice: {n}")
    if mismatched:
        print(f"warning: {mismatched} row(s) whose sign disagrees with their direction text")
    return 0


def _q(xs, f):
    xs = sorted(xs)
    return xs[int(f * (len(xs) - 1))]


def scaling(book, join_bp=5.0, join_minutes=10):
    """How a book scales into and out of each round trip: entries per trip, where each
    add sits against the first fill, its size against what was held, and where the
    position closes against its average entry.

    Fills that add to a position at one price (within join_bp) and close in time (within
    join_minutes of the order's first fill) count as one order, since a resting limit
    often fills in pieces. Moves are signed by the trip's side, so for a long and a
    short alike a negative move is against the position: an add at -2% bought 2% lower
    (long) or sold 2% higher (short). Trips open at the window's start are skipped."""
    by = {}
    for c in book["changes"]:
        by.setdefault(c["symbol"], []).append(c)
    trips = []
    for cs in by.values():
        trip, prev = None, 0.0
        for c in cs:
            pos, start = c["end_position"], c["start_position"]
            start = prev if start is None else start
            px, size = c["price"], abs(pos - start)
            if trip is None and start == 0 and pos != 0:
                trip = {"side": 1 if pos > 0 else -1, "orders": [], "exit_qty": 0.0,
                        "exit_cost": 0.0, "pnl": 0.0}
            if trip is not None and px:
                side = trip["side"]
                if abs(pos) > abs(start) and pos * side > 0:          # adds to the position
                    o = trip["orders"][-1] if trip["orders"] else None
                    if (o and abs(px / o["px"] - 1) * 1e4 <= join_bp
                            and c["ts"] - o["ts"] <= join_minutes * 60_000):
                        o["px"] = (o["px"] * o["qty"] + px * size) / (o["qty"] + size)
                        o["qty"] += size
                    else:
                        trip["orders"].append({"ts": c["ts"], "px": px, "qty": size,
                                               "held": abs(start)})
                else:                                                 # reduces it
                    cut = min(size, abs(start))
                    trip["exit_qty"] += cut
                    trip["exit_cost"] += cut * px
                trip["pnl"] += c["closed_pnl"] or 0.0
                if pos == 0 or pos * side < 0:
                    trip["closed"] = True
                    trips.append(trip)
                    trip = None
                    if pos != 0:                                      # flipped: a new trip
                        trip = {"side": 1 if pos > 0 else -1, "exit_qty": 0.0,
                                "exit_cost": 0.0, "pnl": 0.0,
                                "orders": [{"ts": c["ts"], "px": px, "qty": abs(pos), "held": 0.0}]}
            prev = pos
        if trip is not None and trip["orders"]:
            trip["closed"] = False
            trips.append(trip)
    trips = [t for t in trips if t["orders"]]
    if not trips:
        return None
    counts = {}
    for t in trips:
        counts[len(t["orders"])] = counts.get(len(t["orders"]), 0) + 1
    adds = {}
    for t in trips:
        o, side = t["orders"], t["side"]
        for i in range(1, len(o)):
            held = sum(x["qty"] for x in o[:i])
            a = adds.setdefault(i, ([], []))
            a[0].append(side * (o[i]["px"] / o[0]["px"] - 1))
            a[1].append(o[i]["qty"] / held)
    exits = []
    for t in trips:
        if t["closed"] and t["exit_qty"] > 0:
            q = sum(x["qty"] for x in t["orders"])
            avg = sum(x["px"] * x["qty"] for x in t["orders"]) / q
            exits.append(t["side"] * (t["exit_cost"] / t["exit_qty"] / avg - 1))
    closed = [t for t in trips if t["closed"]]
    return {
        "round_trips": len(trips),
        "orders_per_trip": {n: round(k / len(trips), 4) for n, k in sorted(counts.items())},
        "adds": [{"add": i, "trips": len(m), "move_from_first_median": round(statistics.median(m), 5),
                  "move_p10": round(_q(m, 0.1), 5), "move_p90": round(_q(m, 0.9), 5),
                  "size_vs_held_median": round(statistics.median(z), 3)}
                 for i, (m, z) in sorted(adds.items()) if len(m) >= 5],
        "exit_vs_average_entry": ({"median": round(statistics.median(exits), 5),
                                   "p10": round(_q(exits, 0.1), 5), "p90": round(_q(exits, 0.9), 5)}
                                  if exits else None),
        "closed_trips": len(closed),
        "losing_trips": sum(1 for t in closed if t["pnl"] < 0),
    }


def profile(book):
    """The style of a book, from its position changes."""
    by = {}
    for c in book["changes"]:
        by.setdefault(c["symbol"], []).append(c)
    hours, weekdays = [0] * 24, [0] * 7
    markets, holds, long_ms, short_ms = {}, [], 0, 0
    pnl = fees = 0.0
    maker = known = 0
    for m, cs in by.items():
        trips, opened, prev_pos, prev_ts = 0, None, 0.0, None
        top = 0.0
        for c in cs:
            if prev_ts is not None:
                if prev_pos > 0:
                    long_ms += c["ts"] - prev_ts
                elif prev_pos < 0:
                    short_ms += c["ts"] - prev_ts
            pos = c["end_position"]
            if prev_pos == 0 and pos != 0 or prev_pos * pos < 0:
                when = dt.datetime.fromtimestamp(c["ts"] / 1000, dt.timezone.utc)
                hours[when.hour] += 1
                weekdays[when.weekday()] += 1
                if prev_pos * pos < 0 and opened is not None:
                    holds.append(c["ts"] - opened)
                    trips += 1
                opened = c["ts"]
            elif prev_pos != 0 and pos == 0 and opened is not None:
                holds.append(c["ts"] - opened)
                trips += 1
                opened = None
            if c["price"]:
                top = max(top, abs(pos) * c["price"])
            pnl += c["closed_pnl"] or 0.0
            fees += c["fee"] or 0.0
            if c["maker"] is not None:
                known += 1
                maker += 1 if c["maker"] else 0
            prev_pos, prev_ts = pos, c["ts"]
        markets[m] = {"fills": len(cs), "round_trips": trips, "max_position_usd": round(top, 2),
                      "open_at_end": prev_pos != 0}
    held = long_ms + short_ms
    return {
        "address": book["address"],
        "window": [iso(book["start"]), iso(book["end"])],
        "fills": len(book["changes"]),
        "markets": dict(sorted(markets.items(), key=lambda kv: -kv[1]["fills"])),
        "not_in_dataset": sorted(set(markets) - set(book.get("in_dataset", markets))),
        "round_trips": len(holds),
        "hold_hours_median": round(statistics.median(holds) / HOUR, 2) if holds else None,
        "hold_hours_p90": (round(sorted(holds)[int(0.9 * (len(holds) - 1))] / HOUR, 2)
                           if holds else None),
        "long_share_of_held_time": round(long_ms / held, 3) if held else None,
        "max_gross_usd": round(book["max_gross_usd"], 2),
        "maker_share": round(maker / known, 3) if known else None,
        "opens_by_utc_hour": {h: n for h, n in enumerate(hours) if n},
        "opens_by_weekday": {d: n for d, n in zip(("mon", "tue", "wed", "thu", "fri", "sat", "sun"),
                                                  weekdays) if n},
        "realized_pnl": round(pnl, 2),
        "fees": round(fees, 2),
        "scaling": scaling(book),
    }


def cmd_profile(args):
    book = json.loads(pathlib.Path(args.book).read_text(encoding="utf-8"))
    print(json.dumps(profile(book), indent=1))
    return 0


def _mode(values, rel=0.01):
    """The most common value, and how many values sit within rel of it."""
    counts = {}
    for v in values:
        k = float(f"{v:.6g}")
        counts[k] = counts.get(k, 0) + 1
    if not counts:
        return None, 0
    top = max(counts, key=lambda k: (counts[k], -k))
    return top, sum(1 for v in values if abs(v - top) <= rel * abs(top))


def grid(book, join_minutes=15, min_orders=20):
    """Per market: is it traded as a fixed-size, fixed-step grid?

    Fills at one price on one side within join_minutes of the last are one order (a resting
    limit often fills in pieces). Size and step matches allow 1%; a fill is on the grid when
    its distance from the most traded price is a whole number of steps (within 1% of a step).
    Markets with fewer than min_orders orders are left out."""
    by = {}
    for c in book["changes"]:
        if c["price"] and c["size"]:
            by.setdefault(c["symbol"], []).append(c)
    out = {}
    for m, cs in by.items():
        orders = []
        for c in cs:
            o = orders[-1] if orders else None
            if (o and o["side"] == c["side"] and o["px"] == c["price"]
                    and c["ts"] - o["ts"] <= join_minutes * 60_000):
                o["qty"] += c["size"]
                o["ts"] = c["ts"]
                o["pnl"] += c["closed_pnl"] or 0.0
                o["reduces"] = o["reduces"] or abs(c["end_position"]) < abs(c["start_position"] or 0.0)
            else:
                orders.append({"side": c["side"], "px": c["price"], "qty": c["size"], "ts": c["ts"],
                               "pnl": c["closed_pnl"] or 0.0,
                               "reduces": abs(c["end_position"]) < abs(c["start_position"] or 0.0)})
        if len(orders) < min_orders:
            continue
        size, at_size = _mode([o["qty"] for o in orders])
        steps = [abs(b["px"] - a["px"]) for a, b in zip(orders, orders[1:]) if b["px"] != a["px"]]
        step, at_step = _mode(steps)
        on = 0
        if step:
            anchor = statistics.mode(c["price"] for c in cs)
            for c in cs:
                k = (c["price"] - anchor) / step
                on += abs(k - round(k)) <= 0.01
        pos = [c["end_position"] for c in cs]
        px = sorted(c["price"] for c in cs)
        reducing = [o for o in orders if o["reduces"]]
        out[m] = {
            "fills": len(cs), "orders": len(orders),
            "order_size": size, "orders_at_that_size": at_size,
            "price_step": step,
            "price_step_pct": round(step / px[len(px) // 2] * 100, 3) if step else None,
            "steps": len(steps), "steps_at_that_step": at_step,
            "fills_on_grid": on, "share_on_grid": round(on / len(cs), 3),
            "position_min": min(pos), "position_max": max(pos),
            "buy_orders": sum(1 for o in orders if o["side"] == "B"),
            "sell_orders": sum(1 for o in orders if o["side"] == "A"),
            "reducing_orders_in_profit": (round(sum(1 for o in reducing if o["pnl"] > 0) / len(reducing), 3)
                                          if reducing else None),
            "realized_pnl": round(sum(o["pnl"] for o in orders), 2),
        }
    return {"address": book["address"], "window": [iso(book["start"]), iso(book["end"])],
            "join_minutes": join_minutes,
            "markets": dict(sorted(out.items(), key=lambda kv: -kv[1]["orders"]))}


def cmd_grid(args):
    book = json.loads(pathlib.Path(args.book).read_text(encoding="utf-8"))
    print(json.dumps(grid(book, args.join_minutes, args.min_orders), indent=1))
    return 0


def _opens(book):
    """{market: [(ts, price, side)]}: each trip's first fill (from flat, or a flip)."""
    out, prev = {}, {}
    for c in book["changes"]:
        m, pos = c["symbol"], c["end_position"]
        start = c["start_position"] if c["start_position"] is not None else prev.get(m, 0.0)
        if pos != 0 and (start == 0 or start * pos < 0) and c["price"]:
            out.setdefault(m, []).append((c["ts"], c["price"], 1 if pos > 0 else -1))
        prev[m] = pos
    return out


def cmd_entries(args):
    """Where opening fills sit against common reference levels, and which level they track.

    For each opening fill, reads the minute before it and measures the fill price against
    hourly EMAs (on closed hours, and live: closed hours plus the forming bar at the current
    price), hourly SMAs and the session VWAP (from 00:00 UTC). A rule such as "buy 1%
    under the 12-hour EMA" shows as the reference whose offsets cluster tightest around
    their median (lowest dispersion: interquartile range over the median's size), with most
    entries at that level and few ordinary minutes there (lift)."""
    import oxarchive_backtest as oxb

    book = json.loads(pathlib.Path(args.book).read_text(encoding="utf-8"))
    src = oxb.Source(args.data)
    lengths = [int(n) for n in args.lengths.split(",")]
    have = set(src.markets())
    opens = {m: v for m, v in _opens(book).items() if m in have}
    warm = (max(lengths) + 2) * HOUR
    refs = ([f"ema{n}h" for n in lengths] + [f"ema{n}h_live" for n in lengths]
            + [f"sma{n}h" for n in lengths] + ["vwap_session"])
    entry = {r: [] for r in refs}
    base = {r: [] for r in refs}
    sides = []
    for m, ops in sorted(opens.items()):
        t0, t1 = ops[0][0] - warm, ops[-1][0] + 60_000
        t0 -= t0 % HOUR
        rows = src.rows(m, t0, t1, columns=["close", "volume", "quote_volume"])
        ops = sorted(ops)
        j, k = 0, 0
        ema = {n: None for n in lengths}
        closes = []
        day, qv, vol = None, 0.0, 0.0
        side = 1 if sum(o[2] for o in ops) >= 0 else -1
        for i, r in enumerate(rows):
            px = r.get("close")
            if px is None:
                continue
            ts = r["ts"]
            d = ts // DAY
            if d != day:
                day, qv, vol = d, 0.0, 0.0
            vals = {}
            for n in lengths:
                a = 2 / (n + 1)
                if ema[n] is not None:
                    vals[f"ema{n}h"] = ema[n]
                    vals[f"ema{n}h_live"] = ema[n] * (1 - a) + px * a
                if len(closes) >= n:
                    vals[f"sma{n}h"] = sum(closes[-n:]) / n
            if vol > 0:
                vals["vwap_session"] = qv / vol
            # opening fills in the next minute see this row as their latest
            while j < len(ops) and ops[j][0] < ts + 120_000:
                if ops[j][0] >= ts + 60_000:
                    o_ts, o_px, o_side = ops[j]
                    for ref, v in vals.items():
                        entry[ref].append(o_side * (o_px / v - 1))
                    sides.append(o_side)
                j += 1
            if ts >= ops[0][0] and k % 5 == 0:                       # ordinary minutes, sampled
                for ref, v in vals.items():
                    base[ref].append(side * (px / v - 1))
            k += 1
            qv += r.get("quote_volume") or 0.0
            vol += r.get("volume") or 0.0
            if (ts // 60_000 + 1) % 60 == 0:                          # an hour closes
                closes.append(px)
                for n in lengths:
                    a = 2 / (n + 1)
                    ema[n] = px if ema[n] is None else ema[n] * (1 - a) + px * a
    out = []
    for ref in refs:
        e, b = entry[ref], base[ref]
        if len(e) < 20 or not b:
            continue
        med = statistics.median(e)
        if abs(med) < 0.0005:
            continue
        cut = 0.7 * med
        hit = (lambda x: x <= cut) if med < 0 else (lambda x: x >= cut)
        at, bs = sum(map(hit, e)) / len(e), sum(map(hit, b)) / len(b)
        iqr = _q(e, 0.75) - _q(e, 0.25)
        out.append({"reference": ref, "entries": len(e), "offset_median": round(med, 5),
                    "offset_iqr": round(iqr, 5), "dispersion": round(iqr / abs(med), 3),
                    "entries_at_level": round(at, 3), "minutes_at_level": round(bs, 4),
                    "lift": round(at / bs, 2) if bs else None})
    out.sort(key=lambda r: r["dispersion"])
    print(json.dumps({"opening_fills": sum(len(v) for v in opens.values()),
                      "markets": sorted(opens), "references": out[: args.top]}, indent=1))
    return 0


def _positions(fills):
    """{market: ([ts], [position after])} from a report's fills, oldest first."""
    out = {}
    for f in sorted(fills, key=lambda f: f.ts):
        ts, pos = out.setdefault(f.market, ([], []))
        ts.append(f.ts)
        pos.append((pos[-1] if pos else 0.0) + f.size)
    return out


def _at(path, t):
    import bisect

    ts, pos = path
    i = bisect.bisect_right(ts, t) - 1
    return pos[i] if i >= 0 else 0.0


def _corr(xs, ys):
    n = len(xs)
    mx = my = 0.0
    for a, b in zip(xs, ys):
        mx += a
        my += b
    mx, my = mx / n, my / n
    sxy = sxx = syy = 0.0
    for a, b in zip(xs, ys):
        sxy += (a - mx) * (b - my)
        sxx += (a - mx) * (a - mx)
        syy += (b - my) * (b - my)
    return sxy / (sxx * syy) ** 0.5 if sxx > 0 and syy > 0 else None


def compare(mirror, candidate, t0, t1):
    """Hour by hour over [t0, t1): the candidate against the trader's book, both as the
    engine traded them."""
    a, b = _positions(mirror.fills), _positions(candidate.fills)
    eps = 1e-12
    held = same = other = flat = extra = 0
    for m in sorted(set(a) | set(b)):
        for t in range(t0 - t0 % HOUR + HOUR, t1, HOUR):
            pa = _at(a[m], t) if m in a else 0.0
            pb = _at(b[m], t) if m in b else 0.0
            if abs(pa) > eps:
                held += 1
                if abs(pb) <= eps:
                    flat += 1
                elif (pa > 0) == (pb > 0):
                    same += 1
                else:
                    other += 1
            elif abs(pb) > eps:
                extra += 1
    share = lambda n, d: round(n / d, 3) if d else None
    ra, rb = mirror.daily_returns, candidate.daily_returns
    k = min(len(ra), len(rb))
    corr = _corr(ra[:k], rb[:k]) if k > 2 else None
    pa = {m: v["pnl"] for m, v in (mirror.attribution or {}).items()}
    pb = {m: v["pnl"] for m, v in (candidate.attribution or {}).items()}
    total_a = total_b = 0.0
    for v in pa.values():
        total_a += v
    for v in pb.values():
        total_b += v
    return {
        "trader_held_market_hours": held,
        "same_side": share(same, held),
        "other_side": share(other, held),
        "flat_while_trader_held": share(flat, held),
        "candidate_only_market_hours": extra,
        "trader_pnl": round(total_a, 2),
        "candidate_pnl": round(total_b, 2),
        # The two books run at different sizes, so compare their returns, not their PnL.
        "daily_return_correlation": None if corr is None else round(corr, 3),
        "markets_both_profitable": share(sum(1 for m in pa if pa[m] > 0 and pb.get(m, 0.0) > 0),
                                         sum(1 for m in pa if pa[m] > 0)),
        "trader_sharpe": (mirror.stats or {}).get("sharpe"),
        "candidate_sharpe": (candidate.stats or {}).get("sharpe"),
        "per_market_pnl": {m: {"trader": round(pa.get(m, 0.0), 2), "candidate": round(pb.get(m, 0.0), 2)}
                           for m in sorted(set(pa) | set(pb))},
    }


def _pairs(items):
    out = {}
    for item in items or ():
        name, _, value = item.partition("=")
        try:
            out[name] = json.loads(value)
        except json.JSONDecodeError:
            out[name] = value
    return out


def cmd_replicate(args):
    try:
        import oxarchive_backtest as oxb
    except ImportError:
        raise SystemExit('replicate needs the engine: pip install "0xarchive-backtest[curated]"') from None
    from oxarchive_backtest import study

    config = oxb.Config(**_pairs(args.config))
    t0, t1 = ms(args.start), ms(args.end)

    def run(path, params):
        text = pathlib.Path(path).read_text(encoding="utf-8")
        module = oxb.load_strategy(path, name=f"oxs_{pathlib.Path(path).stem}")
        with study.running(path, text):
            return oxb.backtest(oxb.strategy_of(module, params), t0, t1, args.data, config)

    try:
        mirror = run(args.mirror or MIRROR, {"path": str(pathlib.Path(args.book).resolve()),
                                             "clock": args.clock})
        candidate = run(args.candidate, _pairs(args.param))
    except (ValueError, oxb.Sealed, oxb.MissingData) as e:
        raise SystemExit(str(e)) from None
    out = compare(mirror, candidate, t0, t1)
    out["trader_result_hash"], out["candidate_result_hash"] = mirror.result_hash, candidate.result_hash
    print(json.dumps(out, indent=1))
    return 0


def main(argv=None):
    p = argparse.ArgumentParser(prog="trader.py", description=__doc__.split("\n\n")[0])
    sub = p.add_subparsers(dest="command", required=True)
    f = sub.add_parser("fetch", help="pull a wallet's position changes")
    f.add_argument("address")
    f.add_argument("--start", required=True)
    f.add_argument("--end", required=True)
    f.add_argument("--venue", default="hyperliquid", choices=sorted(PREFIX))
    f.add_argument("--data", help="the backtest dataset, to mark which markets it has")
    f.add_argument("--out", default="trader.json")
    f.set_defaults(run=cmd_fetch)
    f = sub.add_parser("profile", help="the trader's style from the fills")
    f.add_argument("book")
    f.set_defaults(run=cmd_profile)
    f = sub.add_parser("grid", help="whether a market is traded as a fixed-size, fixed-step grid")
    f.add_argument("book")
    f.add_argument("--join-minutes", type=int, default=15)
    f.add_argument("--min-orders", type=int, default=20)
    f.set_defaults(run=cmd_grid)
    f = sub.add_parser("entries", help="which reference level the opening fills track")
    f.add_argument("book")
    f.add_argument("--data", default="data/hyperliquid")
    f.add_argument("--lengths", default="6,8,12,14,16,20,24,48,72", help="hours, comma-separated")
    f.add_argument("--top", type=int, default=10)
    f.set_defaults(run=cmd_entries)
    f = sub.add_parser("replicate", help="score a candidate against the trader's book")
    f.add_argument("book")
    f.add_argument("candidate")
    f.add_argument("--start", required=True)
    f.add_argument("--end", required=True)
    f.add_argument("--data", default="data/hyperliquid")
    f.add_argument("--clock", default="1h", help="the mirror's clock (default 1h)")
    f.add_argument("--mirror", help="the mirror strategy file (default: mirror.py beside this script)")
    f.add_argument("-p", "--param", action="append", metavar="NAME=VALUE")
    f.add_argument("-c", "--config", action="append", metavar="NAME=VALUE")
    f.set_defaults(run=cmd_replicate)
    args = p.parse_args(argv)
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main())
