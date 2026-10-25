use super::*;

const T0: i64 = 1_791_208_800_000;

fn cfg() -> Config {
    Config {
        starting_cash: 10_000.0,
        taker_fee_bps: 4.5,
        maker_fee_bps: 1.5,
        builder_fee_bps: 1.0,
        fallback_slippage_bps: 2.0,
        max_leverage: 40.0,
        maintenance_margin: 0.0125,
    }
}

fn bar(i: i64, o: f64, h: f64, l: f64, c: f64) -> Row {
    Row {
        ts: T0 + i * 60_000,
        open: Some(o),
        high: Some(h),
        low: Some(l),
        close: Some(c),
        mark_price: Some(c),
        ..Row::default()
    }
}

fn flat(i: i64, px: f64) -> Row {
    bar(i, px, px, px, px)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9 * a.abs().max(1.0)
}

#[test]
fn market_fills_at_next_open_with_fallback_slippage_and_fees() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 2.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&bar(1, 101.0, 102.0, 100.0, 101.5));
    let f = e.fills[0];
    assert_eq!(f.ts, T0 + 60_000);
    assert!(close(f.price, 101.0 * 1.0002));
    assert!(
        close(f.fee, 2.0 * f.price * 5.5 / 10_000.0),
        "taker 4.5 + builder 1.0"
    );
    assert_eq!(f.liquidity, Liquidity::Taker);
    assert_eq!(e.position(), 2.0);
}

#[test]
fn resting_limit_fills_only_when_traded_through() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Limit { price: 99.0 },
        reduce_only: false,
    })
    .unwrap();
    // Low touches 99 exactly: no fill.
    e.step(&bar(1, 100.0, 100.5, 99.0, 100.0));
    assert!(e.fills.is_empty());
    // Low trades through 99: maker fill at 99.
    e.step(&bar(2, 100.0, 100.0, 98.9, 99.5));
    let f = e.fills[0];
    assert_eq!((f.price, f.liquidity), (99.0, Liquidity::Maker));
    assert!(
        close(f.fee, 99.0 * 2.5 / 10_000.0),
        "maker 1.5 + builder 1.0"
    );
}

#[test]
fn marketable_limit_on_arrival_takes_at_open_capped_at_its_price() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Limit { price: 100.01 },
        reduce_only: false,
    })
    .unwrap();
    e.step(&bar(1, 100.0, 100.5, 99.5, 100.0));
    let f = e.fills[0];
    assert_eq!(f.liquidity, Liquidity::Taker);
    assert!(
        close(f.price, 100.01),
        "open 100 + 2 bps = 100.02, capped at the limit"
    );
}

#[test]
fn resting_limit_gapped_through_fills_at_its_price_as_maker() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Limit { price: 99.0 },
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    e.step(&bar(2, 97.0, 97.5, 96.0, 97.0));
    assert_eq!(
        (e.fills[0].price, e.fills[0].liquidity),
        (99.0, Liquidity::Maker)
    );
}

#[test]
fn stop_triggers_through_and_fills_at_the_worse_of_open_and_trigger() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Stop { trigger: 101.0 },
        reduce_only: false,
    })
    .unwrap();
    e.step(&bar(1, 100.0, 100.9, 99.0, 100.0));
    assert!(e.fills.is_empty());
    // Opens above the trigger: fills from the open, not the trigger.
    e.step(&bar(2, 102.0, 103.0, 101.5, 102.5));
    assert!(close(e.fills[0].price, 102.0 * 1.0002));
}

#[test]
fn stop_counts_first_when_a_bar_hits_both_stop_and_target() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    let stop = e
        .submit(OrderRequest {
            size: -1.0,
            kind: OrderType::Stop { trigger: 99.0 },
            reduce_only: true,
        })
        .unwrap();
    let target = e
        .submit(OrderRequest {
            size: -1.0,
            kind: OrderType::Limit { price: 101.0 },
            reduce_only: true,
        })
        .unwrap();
    e.step(&bar(2, 100.0, 102.0, 98.0, 100.0));
    assert_eq!(e.position(), 0.0);
    let exit = e.fills.last().unwrap();
    assert_eq!(exit.order_id, Some(stop));
    assert!(close(exit.price, 99.0 * 0.9998));
    assert_eq!(
        e.cancels.last().map(|c| (c.order_id, c.reason)),
        Some((target, CancelReason::NothingToReduce))
    );
}

#[test]
fn reduce_only_never_flips() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    e.submit(OrderRequest {
        size: -5.0,
        kind: OrderType::Market,
        reduce_only: true,
    })
    .unwrap();
    e.step(&flat(2, 100.0));
    assert_eq!(e.position(), 0.0);
}

#[test]
fn exposure_is_capped_at_max_leverage() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    // $10k at 40x is $400k: 4,000 units at 100, asked for 10,000.
    let id = e
        .submit(OrderRequest {
            size: 10_000.0,
            kind: OrderType::Market,
            reduce_only: false,
        })
        .unwrap();
    e.step(&flat(1, 100.0));
    assert!(
        e.position() > 3_900.0 && e.position() < 4_000.0,
        "{}",
        e.position()
    );
    assert_eq!(
        e.cancels.last().map(|c| (c.order_id, c.reason)),
        Some((id, CancelReason::MarginLimit))
    );
}

#[test]
fn a_highly_leveraged_long_is_liquidated_at_the_bar_low() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 3_000.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    assert_eq!(e.position(), 3_000.0);
    // 30x long: a 4% low wipes out more than its equity.
    e.step(&bar(2, 100.0, 100.0, 96.0, 99.0));
    assert_eq!(e.position(), 0.0);
    let f = e.fills.last().unwrap();
    assert_eq!(f.liquidity, Liquidity::Liquidation);
    assert!(close(f.price, 96.0 * 0.9998));
    let r = e.finish();
    assert_eq!(r.liquidations, 1);
    assert!(r.bad_debt > 0.0);
    assert_eq!(r.final_cash, 0.0);
}

#[test]
fn a_modest_long_survives_the_same_bar() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 100.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    e.step(&bar(2, 100.0, 100.0, 97.0, 99.0));
    assert_eq!(e.position(), 100.0);
}

#[test]
fn slippage_uses_the_previous_rows_depth_not_the_fill_rows() {
    let mut e = Engine::new(cfg());
    let mut r0 = flat(0, 100.0);
    r0.spread_bps = Some(2.0);
    r0.depth_ask_10bps = Some(100_000.0);
    r0.depth_bid_10bps = Some(100_000.0);
    r0.depth_ask_50bps = Some(500_000.0);
    r0.depth_bid_50bps = Some(500_000.0);
    e.step(&r0);
    e.submit(OrderRequest {
        size: 500.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    // The fill row has very different depth; it must not be used.
    let mut r1 = flat(1, 100.0);
    r1.spread_bps = Some(50.0);
    r1.depth_ask_10bps = Some(1.0);
    r1.depth_bid_10bps = Some(1.0);
    r1.depth_ask_50bps = Some(2.0);
    r1.depth_bid_50bps = Some(2.0);
    e.step(&r1);
    // $50k into $100k of 10 bps depth: half spread 1 bp + 5 bps walk.
    assert!(
        close(e.fills[0].price, 100.0 * 1.0006),
        "{}",
        e.fills[0].price
    );
}

#[test]
fn orders_wait_through_a_bar_with_no_prices() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 1.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&Row {
        ts: T0 + 60_000,
        ..Row::default()
    });
    assert!(e.fills.is_empty());
    e.step(&flat(2, 101.0));
    assert_eq!(e.fills[0].ts, T0 + 120_000);
}

#[test]
fn longs_pay_positive_funding() {
    let mut e = Engine::new(cfg());
    e.step(&flat(0, 100.0));
    e.submit(OrderRequest {
        size: 2.0,
        kind: OrderType::Market,
        reduce_only: false,
    })
    .unwrap();
    e.step(&flat(1, 100.0));
    let mut r = flat(2, 100.0);
    r.funding_settled = Some(0.0001);
    e.step(&r);
    assert!(close(e.funding_paid, 2.0 * 100.0 * 0.0001));
}

#[test]
fn rejects_bad_orders() {
    let mut e = Engine::new(cfg());
    assert_eq!(
        e.submit(OrderRequest {
            size: 0.0,
            kind: OrderType::Market,
            reduce_only: false
        }),
        Err(Reject::ZeroSize)
    );
    assert_eq!(
        e.submit(OrderRequest {
            size: 1.0,
            kind: OrderType::Limit { price: -1.0 },
            reduce_only: false
        }),
        Err(Reject::BadPrice)
    );
    assert_eq!(e.cancel(42), Err(Reject::UnknownOrder));
}

#[test]
fn run_drives_a_strategy_and_hashes_deterministically() {
    struct BuyThenSell(u32);
    impl Strategy for BuyThenSell {
        fn on_bar(&mut self, ctx: &mut Ctx, _: &Row) {
            match self.0 {
                0 => {
                    ctx.market(1.0).unwrap();
                }
                2 => {
                    ctx.market(-ctx.position()).unwrap();
                }
                _ => {}
            }
            self.0 += 1;
        }
    }
    let rows: Vec<Row> = (0..5).map(|i| flat(i, 100.0 + i as f64)).collect();
    let a = run(&rows, &mut BuyThenSell(0), cfg());
    assert_eq!(a.fills.len(), 2);
    assert_eq!(a.equity_curve.len(), 5);
    assert_eq!(
        a.result_hash,
        run(&rows, &mut BuyThenSell(0), cfg()).result_hash
    );
    let mut c = cfg();
    c.builder_fee_bps = 2.0;
    assert_ne!(
        run(&rows, &mut BuyThenSell(0), c).result_hash,
        a.result_hash
    );
}

// ---------- portfolios ----------

fn two(lev_a: f64, lev_b: f64) -> Engine {
    Engine::with_markets(
        cfg(),
        vec![
            MarketSpec::with_leverage("A", lev_a),
            MarketSpec::with_leverage("B", lev_b),
        ],
    )
}

fn buy(e: &mut Engine, m: MarketId, size: f64) {
    e.submit_to(
        m,
        OrderRequest {
            size,
            kind: OrderType::Market,
            reduce_only: false,
        },
    )
    .unwrap();
}

#[test]
fn reference_hash_is_unchanged_by_portfolios() {
    // Pinned before portfolios existed; check-targets.sh compares it across targets.
    assert_eq!(
        format!("{:016x}", reference::run_reference().result_hash),
        "3e76be646f0cd888"
    );
}

#[test]
fn an_idle_second_market_changes_nothing_but_the_hash() {
    let rows = reference::rows(3_000);
    let idle: Vec<Row> = rows
        .iter()
        .map(|r| Row {
            ts: r.ts,
            ..flat(0, 50.0)
        })
        .collect();
    let single = run(&rows, &mut reference::MeanCross::default(), cfg());

    struct OnFirst(reference::MeanCross);
    impl PortfolioStrategy for OnFirst {
        fn on_bars(&mut self, ctx: &mut Ctx, rows: &[Option<&Row>]) {
            self.0.on_bar(ctx, rows[0].unwrap());
        }
    }
    let specs = vec![
        MarketSpec::from_config("A", &cfg()),
        MarketSpec::from_config("B", &cfg()),
    ];
    let p = run_portfolio(
        &[&rows, &idle],
        specs,
        &mut OnFirst(reference::MeanCross::default()),
        cfg(),
    );
    assert_eq!(p.fills.len(), single.fills.len());
    for (a, b) in p.fills.iter().zip(&single.fills) {
        assert_eq!(
            (a.ts, a.size, a.price, a.fee),
            (b.ts, b.size, b.price, b.fee)
        );
    }
    assert_eq!(p.final_cash, single.final_cash);
    assert_eq!(p.final_positions, vec![single.final_positions[0], 0.0]);
    assert_ne!(
        p.result_hash, single.result_hash,
        "the portfolio hashes markets"
    );
}

#[test]
fn leverage_cap_counts_the_other_positions_margin() {
    let mut e = two(10.0, 10.0);
    e.step_markets(&[Some(&flat(0, 100.0)), Some(&flat(0, 100.0))]);
    // 50k notional on A at 10x uses 5k of the 10k.
    buy(&mut e, 0, 500.0);
    e.step_markets(&[Some(&flat(1, 100.0)), Some(&flat(1, 100.0))]);
    assert_eq!(e.position_of(0), 500.0);
    // B wants 80k; about 5k of margin is left, so about 50k fits.
    buy(&mut e, 1, 800.0);
    e.step_markets(&[Some(&flat(2, 100.0)), Some(&flat(2, 100.0))]);
    let b = e.position_of(1);
    assert!(b > 450.0 && b < 500.0, "B capped by the margin A uses: {b}");
    assert_eq!(e.cancels.last().unwrap().reason, CancelReason::MarginLimit);
}

#[test]
fn a_capped_buy_waits_for_the_minutes_sells() {
    let mut e = two(1.0, 1.0);
    e.step_markets(&[Some(&flat(0, 100.0)), Some(&flat(0, 100.0))]);
    // All the margin on B (market 1); then rotate into A (market 0), which comes first.
    buy(&mut e, 1, 99.0);
    e.step_markets(&[Some(&flat(1, 100.0)), Some(&flat(1, 100.0))]);
    buy(&mut e, 0, 90.0);
    e.submit_to(
        1,
        OrderRequest {
            size: -99.0,
            kind: OrderType::Market,
            reduce_only: false,
        },
    )
    .unwrap();
    e.step_markets(&[Some(&flat(2, 100.0)), Some(&flat(2, 100.0))]);
    // bar-v1 would have cut A to nothing before B's sale freed the margin.
    assert_eq!((e.position_of(0), e.position_of(1)), (90.0, 0.0));
    let order: Vec<usize> = e.fills.iter().skip(1).map(|f| f.market).collect();
    assert_eq!(order, vec![1, 0], "the sale first, then the buy");
}

#[test]
fn a_loss_on_one_market_liquidates_the_whole_account() {
    let mut e = two(10.0, 10.0);
    e.step_markets(&[Some(&flat(0, 100.0)), Some(&flat(0, 100.0))]);
    buy(&mut e, 0, 450.0);
    buy(&mut e, 1, 450.0);
    e.step_markets(&[Some(&flat(1, 100.0)), Some(&flat(1, 100.0))]);
    assert_eq!((e.position_of(0), e.position_of(1)), (450.0, 450.0));
    // A's low is 13% down; B is flat. Equity (about 4.1k) drops below the 5% maintenance
    // on both positions (about 4.2k), though B alone is fine.
    e.step_markets(&[
        Some(&bar(2, 100.0, 100.0, 87.0, 99.0)),
        Some(&flat(2, 100.0)),
    ]);
    assert_eq!((e.position_of(0), e.position_of(1)), (0.0, 0.0));
    let liq: Vec<_> = e
        .fills
        .iter()
        .filter(|f| f.liquidity == Liquidity::Liquidation)
        .map(|f| (f.market, f.size))
        .collect();
    assert_eq!(liq, vec![(0, -450.0), (1, -450.0)]);
    assert_eq!(e.liquidations, 2);
}

#[test]
fn a_market_with_no_row_is_marked_at_its_last_price() {
    let mut e = two(10.0, 10.0);
    e.step_markets(&[Some(&flat(0, 100.0)), Some(&flat(0, 100.0))]);
    buy(&mut e, 1, 100.0);
    e.step_markets(&[Some(&flat(1, 100.0)), Some(&flat(1, 101.0))]);
    // B has no row: its order book and price stand still; A's bar still steps the account.
    e.step_markets(&[Some(&flat(2, 100.0)), None]);
    assert_eq!(e.position_of(1), 100.0);
    assert_eq!(e.equity_curve.len(), 3);
    assert!(close(e.equity(), e.cash() + 100.0 * 101.0));
}

#[test]
fn align_walks_markets_minute_by_minute() {
    let a = vec![flat(0, 1.0), flat(1, 1.0), flat(3, 1.0)];
    let b = vec![flat(1, 2.0), flat(2, 2.0), flat(3, 2.0)];
    let data = [a.as_slice(), b.as_slice()];
    let got: Vec<(i64, bool, bool)> = align(&data)
        .map(|s| {
            let ts = s.iter().flatten().next().unwrap().ts;
            ((ts - T0) / 60_000, s[0].is_some(), s[1].is_some())
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (0, true, false),
            (1, true, true),
            (2, false, true),
            (3, true, true)
        ]
    );
}

#[test]
fn orders_to_an_unknown_market_are_rejected() {
    let mut e = Engine::new(cfg());
    let req = OrderRequest {
        size: 1.0,
        kind: OrderType::Market,
        reduce_only: false,
    };
    assert_eq!(e.submit_to(1, req), Err(Reject::UnknownMarket));
}
