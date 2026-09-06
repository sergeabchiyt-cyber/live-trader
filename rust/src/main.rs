//! FRVP live trader v2 (Rust) — full service: Bybit/Binance REST feed, FRVP
//! strategy engine, fee-aware paper ledger, Binance order mirroring, threaded
//! HTTP + WebSocket push server, embedded terminal dashboard.
//!
//!   live-trader                 serve (default; env-configured)
//!   live-trader replay <csv>    parity replay: ts(ms),o,h,l,c,v -> trades JSON
//!   live-trader bench <csv>     timing micro-benchmark

mod broker;
mod config;
mod dashboard;
mod engine;
mod feed;
mod http;
mod hub;
mod ledger;
mod ws;

use engine::{Bar, Event, Strategy, StrategyCfg};
use hub::Hub;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn strategy_cfg(c: &config::Config) -> StrategyCfg {
    StrategyCfg {
        rr: c.rr,
        skip_hour0: c.skip_hour0,
        trail_on: c.trail_on,
        ladder: c.ladder.clone(),
        same_bar_exits: c.same_bar,
        sl_touch: c.sl_touch,
    }
}

// ---------------------------------------------------------------------------
// replay / parity
// ---------------------------------------------------------------------------
fn parse_csv(path: &str) -> Vec<Bar> {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read csv: {e}"));
    let mut bars = Vec::with_capacity(260_000);
    for line in raw.lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let ts_ms: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let o = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let h = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let l = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let c = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let v = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        bars.push(Bar { ts: (ts_ms / 1000.0) as i64, o, h, l, c, v });
    }
    bars.sort_by_key(|b| b.ts);
    bars
}

fn cmd_replay(path: &str, touch: bool, json_out: bool) {
    let bars = parse_csv(path);
    let cfg = StrategyCfg {
        rr: 2.0,
        skip_hour0: false,
        trail_on: true,
        ladder: config::DEFAULT_LADDER.to_vec(),
        same_bar_exits: true,
        sl_touch: touch,
    };
    let t0 = std::time::Instant::now();
    let mut s = Strategy::new(cfg);
    for b in &bars {
        s.on_bar(b);
    }
    let elapsed = t0.elapsed().as_secs_f64();
    if json_out {
        let trades: Vec<Value> = s.trades.iter().map(|t| t.to_json()).collect();
        println!("{}", serde_json::to_string(&trades).unwrap());
        return;
    }
    let n = s.trades.len();
    let sl = s.trades.iter().filter(|t| t.exit_type == "SL").count();
    let tp = s.trades.iter().filter(|t| t.exit_type == "TP").count();
    let eos = s.trades.iter().filter(|t| t.exit_type == "EOS").count();
    let wins = s.trades.iter().filter(|t| t.pct > 0.0).count();
    println!("replay of {} bars in {elapsed:.4}s ({:.0} bars/s)", bars.len(), bars.len() as f64 / elapsed);
    println!("trades      : {n}");
    println!("win rate    : {:.1}%", 100.0 * wins as f64 / n.max(1) as f64);
    println!("net         : {:+.2}%", s.net());
    println!("exit mix    : SL {sl} / TP {tp} / EOS {eos}");
}

fn cmd_bench(path: &str) {
    let bars = parse_csv(path);
    let cfg = StrategyCfg {
        rr: 2.0,
        skip_hour0: false,
        trail_on: true,
        ladder: config::DEFAULT_LADDER.to_vec(),
        same_bar_exits: true,
        sl_touch: false,
    };
    let _ = {
        let mut s = Strategy::new(cfg.clone());
        for b in &bars {
            s.on_bar(b);
        }
        s.net()
    };
    let mut times = Vec::new();
    for _ in 0..10 {
        let t = std::time::Instant::now();
        let mut s = Strategy::new(cfg.clone());
        for b in &bars {
            s.on_bar(b);
        }
        std::hint::black_box(s.net());
        times.push(t.elapsed().as_secs_f64());
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let best = times[0];
    println!(
        "full-engine replay best-of-10: {best:.5}s -> {:.0} bars/s ({:.2} us/bar)",
        bars.len() as f64 / best,
        best * 1e6 / bars.len() as f64
    );
}

// ---------------------------------------------------------------------------
// serve (the live service)
// ---------------------------------------------------------------------------
fn cmd_serve() {
    let cfg = config::Config::from_env();
    config::banner(&cfg);
    let hub = Arc::new(Hub::new());
    if http::serve(&cfg, hub.clone()).is_err() {
        eprintln!("[fatal] could not bind {}:{} — exiting", cfg.host, cfg.port);
        std::process::exit(1);
    }

    let feed = Arc::new(Mutex::new(feed::RestFeed::new(&cfg)));
    let mut broker = broker::Broker::new(&cfg);
    let mut strat = Strategy::new(strategy_cfg(&cfg));
    let mut trades_log: Vec<Value> = Vec::new();

    // ---- bootstrap (retry forever; HTTP is already serving health checks) --
    let boot_bars = loop {
        let mut f = feed.lock().unwrap();
        match f.bootstrap() {
            Ok(bars) => break bars,
            Err(e) => {
                println!("[feed] bootstrap failed: {e} — retrying in 5s");
                drop(f);
                hub.publish(
                    "health",
                    json!({"status": "bootstrapping", "error": e}),
                );
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    };
    println!("[feed] bootstrapped {} closed bars", boot_bars.len());

    // ---- warm the engine on history (quiet: no hub publish, no orders,
    //      but the ledger replays every historical trade with fees) ---------
    let warm_events = strat.feed_history(&boot_bars);
    for ev in &warm_events {
        match ev {
            Event::Open { entry, slp, .. } => {
                broker.ledger.on_open(*entry, *slp);
            }
            Event::Close(t) => {
                let enriched = broker.quiet_close(t);
                trades_log.push(enriched);
            }
            _ => {}
        }
    }
    println!(
        "[engine] warm: {} bars, session {:?}, prev_poc {:?}, {} trades replayed (ledger equity {:.2})",
        boot_bars.len(),
        strat.active_session(),
        strat.prev_poc,
        trades_log.len(),
        broker.ledger.equity
    );

    // ---- ~1s tick publisher (cheap ticker poll; WS + SSE fan-out) ---------
    {
        let hub_t = hub.clone();
        let feed_t = feed.clone();
        let tick_s = cfg.tick_s.clamp(0.25, 10.0);
        std::thread::Builder::new()
            .name("tick-pub".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_secs_f64(tick_s));
                let px = {
                    let mut f = feed_t.lock().unwrap();
                    f.poll_ticker()
                };
                if let Some(p) = px {
                    hub_t.publish("tick", json!({"price": p, "ts": engine::now_ts_f64()}));
                }
            })
            .ok();
    }

    // ---- main loop: poll klines -> engine -> publish state ----------------
    let poll = cfg.poll_s.clamp(0.5, 10.0);
    loop {
        std::thread::sleep(Duration::from_secs_f64(poll));
        let new_bars = {
            let mut f = feed.lock().unwrap();
            f.poll()
        };
        for b in new_bars {
            let evs = strat.on_bar(&b);
            for ev in evs {
                match ev {
                    Event::Session { .. } => {
                        hub.publish("session", ev.to_json());
                    }
                    Event::Open { side, entry, sl, tp, slp, .. } => {
                        hub.publish("open", ev.to_json());
                        broker.on_open(side, entry, sl, tp, slp);
                    }
                    Event::Trail { sl, .. } => {
                        hub.publish("trail", ev.to_json());
                        broker.on_trail(sl);
                    }
                    Event::Close(ref t) => {
                        let enriched = broker.on_close(t);
                        trades_log.push(enriched.clone());
                        if trades_log.len() > 400 {
                            let drop = trades_log.len() - 300;
                            trades_log.drain(0..drop);
                        }
                        hub.publish("close", enriched);
                    }
                }
            }
        }
        publish_state(&hub, &cfg, &strat, &broker, &feed, &trades_log);
    }
}

fn publish_state(
    hub: &Arc<Hub>,
    cfg: &config::Config,
    strat: &Strategy,
    broker: &broker::Broker,
    feed: &Arc<Mutex<feed::RestFeed>>,
    trades_log: &[Value],
) {
    let mut st = strat.state();
    let (last_price, feed_health, venue_label) = {
        let f = feed.lock().unwrap();
        (f.last_price, f.health.to_json(), f.venue_label())
    };
    st["mode"] = json!(cfg.display_mode());
    st["symbol"] = json!(cfg.symbol);
    st["source"] = json!(format!("{} ({})", cfg.source_label(), venue_label));
    let lb_close = st.get("last_bar").and_then(|b| b.get("c")).cloned();
    st["last_price"] = match last_price {
        Some(p) => json!(p),
        None => lb_close.unwrap_or(Value::Null),
    };
    st["balance"] = broker.balance();
    st["equity_curve"] = broker.ledger.equity_curve_json();
    st["recent_bars"] = strat
        .bars
        .iter()
        .rev()
        .take(240)
        .rev()
        .map(|b| json!({"ts": b.ts, "o": b.o, "h": b.h, "l": b.l, "c": b.c, "v": b.v}))
        .collect::<Vec<_>>()
        .into();
    st["last_trades"] = trades_log.iter().rev().take(40).cloned().collect::<Vec<_>>().into();
    st["broker_status"] = broker.status_json();
    st["feed_health"] = feed_health.clone();
    st["engine"] = json!(http::ENGINE_VERSION);
    st["config"] = json!({
        "rr": cfg.rr, "skip_hour0": cfg.skip_hour0, "trail_on": cfg.trail_on,
        "ladder": cfg.ladder, "symbol": cfg.symbol,
        "sl_mode": if cfg.sl_touch { "touch" } else { "close" },
        "fee_maker_pct": cfg.fee_maker_pct, "fee_taker_pct": cfg.fee_taker_pct,
        "sl_slip_pct": cfg.sl_slip_pct,
        "legacy_sizing": cfg.legacy_sizing, "position_usd": cfg.position_usd,
        "risk_pct": cfg.risk_pct, "leverage_cap": cfg.leverage_cap,
        "poll_s": cfg.poll_s,
    });
    hub.publish("state", st);
    hub.publish(
        "health",
        json!({"status": "ok", "engine": http::ENGINE_VERSION, "feed": feed_health}),
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("replay") => {
            let path = args.get(2).expect("usage: live-trader replay <csv> [--touch] [--json]");
            let touch = args.iter().any(|a| a == "--touch");
            let json_out = args.iter().any(|a| a == "--json");
            cmd_replay(path, touch, json_out);
        }
        Some("bench") => {
            let path = args.get(2).expect("usage: live-trader bench <csv>");
            cmd_bench(path);
        }
        _ => cmd_serve(),
    }
}
