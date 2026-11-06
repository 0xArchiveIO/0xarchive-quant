# A scale-in ladder as a strategy: the shape `trader.py profile` reports under "scaling"
# when a book adds to losing positions at fixed steps and closes the whole position at a
# fixed distance from its average entry (DCA, grid and martingale books).
#
#     0xbt backtest ladder.py -p markets=BTC,ETH,SOL --start 2026-08-01 --end 2026-11-01
#     python3 trader.py replicate trader.json ladder.py -p markets=BTC,ETH,SOL --clock 1m ...
#
# Flat market: rest an entry order where price would sit `entry` away from the reference,
# a live hourly EMA (closed hours plus the forming one, which is what `trader.py entries`
# calls emaNh_live). First fill opens a trip. Then rest one add at each of `steps` from the
# first fill, sized `mult` x what is held, and one reduce-only order for the whole position
# at the average entry + `exit`, re-placed after every add. `stop` (0 = none) closes the
# trip at the average entry - stop. `side` is 1 for longs (buys dips), -1 for shorts.
#
# Fill in the numbers from the profile ("adds", "exit_vs_average_entry") and from
# `trader.py entries` (the reference and its median offset). The defaults are placeholders.
import oxarchive_backtest as oxb


def strategy(markets="BTC,ETH,SOL", side=1, ema_hours=12, entry=-0.01,
             steps="-0.02,-0.04,-0.06,-0.08", mult="1.0,1.0,1.0,1.0",
             exit=0.005, base=0.005, stop=0.0):
    universe = [m.strip() for m in markets.split(",")] if isinstance(markets, str) else list(markets)
    steps = [float(v) for v in steps.split(",")] if isinstance(steps, str) else list(steps)
    mult = [float(v) for v in mult.split(",")] if isinstance(mult, str) else list(mult)
    a = 2 / (ema_hours + 1)

    def cancel(ctx, oid):
        # An order can fill in the same minute it would be replaced, and cancelling a
        # filled order raises, so only cancel what is still open.
        if oid is not None and any(o.id == oid for o in ctx.open_orders):
            ctx.cancel(oid)

    def entry_px(e):
        # p / (e (1 - a) + p a) = 1 + entry, solved for p
        k = 1 + entry
        return k * e * (1 - a) / (1 - k * a)

    def rest(ctx, m, t):
        for key in ("next", "out", "stop"):
            cancel(ctx, t.get(key))
            t[key] = None
        held = abs(t["qty"])
        if held <= 0:
            return
        avg = t["cost"] / t["qty"]
        t["out"] = ctx.limit(m, -side * held, avg * (1 + side * exit), reduce_only=True)
        if stop > 0:
            t["stop"] = ctx.stop(m, -side * held, avg * (1 - side * stop), reduce_only=True)
        i = t["level"]                       # orders filled so far, 1 = the entry
        if i <= len(steps):
            t["next"] = ctx.limit(m, side * mult[i - 1] * held, t["first"] * (1 + side * steps[i - 1]))

    def step(ctx, h):
        st = ctx.state
        ema, trips, ent = st.setdefault("ema", {}), st.setdefault("trips", {}), st.setdefault("entry", {})
        hour = ctx.time // 3_600_000
        new_hour = st.get("hour") != hour
        if new_hour:
            st["hour"] = hour
            closes = h.close.last()
            for m in universe:
                c = closes[m]
                if c is not None:
                    ema[m] = c if m not in ema else ema[m] * (1 - a) + c * a
        for m in universe:
            if m in trips or m not in ema or ctx.price(m) is None:
                continue
            if new_hour or m not in ent:     # the level only moves when an hour closes
                cancel(ctx, ent.get(m))
                px = entry_px(ema[m])
                ent[m] = ctx.limit(m, side * base * ctx.equity / px, px)

    def on_fill(ctx, f):
        st = ctx.state
        m, trips, ent = f.market, st.setdefault("trips", {}), st.setdefault("entry", {})
        t = trips.get(m)
        if f.size * side > 0:                # adds to the position
            if t is None:
                t = trips[m] = {"first": f.price, "qty": 0.0, "cost": 0.0, "level": 0}
                ent.pop(m, None)
            if f.order_id != t.get("last_add"):
                t["level"] += 1
                t["last_add"] = f.order_id
            t["qty"] += f.size
            t["cost"] += f.size * f.price
            rest(ctx, m, t)
        elif t is not None:                  # reduces it
            t["cost"] *= (t["qty"] + f.size) / t["qty"]
            t["qty"] += f.size
            if abs(ctx.positions[m]) <= 1e-12:
                for key in ("next", "out", "stop"):
                    cancel(ctx, t.get(key))
                del trips[m]

    return oxb.Strategy(universe=universe, clock="1m", step=step, on_fill=on_fill)
