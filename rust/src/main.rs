//! frvp — FRVP PoC gold strategy core (Rust)
//!
//!   frvp replay <csv>          replay a ts(ms),o,h,l,c,v CSV and report trades
//!   frvp bench  <csv>          timing micro-benchmark (whole-history replay)
//!   frvp live [SYMBOL] [POLL]  live loop over Binance public data-api (PAXGUSDT)
//!
//! Parity target: replay of the XAUUSD 2016→2026 cache with
//! RR=2, trail=[0.6:0.1,0.75:0.4,0.9:0.7], same-bar exits, no hour-skip yields
//! n=1804 trades, net=+66.20%, SL/TP/EOS = 1438/347/19 (matches Python engine).

mod engine;

use engine::{Bar, Strategy, StrategyCfg};
use std::time::Instant;

const LADDER: [(f64, f64); 3] = [(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)];

fn parse_csv(path: &str) -> Vec<Bar> {
    let raw = std::fs::read_to_string(path).expect("read csv");
    let mut bars = Vec::with_capacity(260_000);
    for line in raw.lines().skip(1) {
        if line.is_empty() {
            continue;
        }
        let mut it = line.split(',');
        let ts_ms: f64 = match it.next() {
            Some(x) => x.parse().unwrap_or(0.0),
            None => continue,
        };
        let o: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let h: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let l: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let c: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        let v: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        bars.push(Bar { ts: (ts_ms / 1000.0) as i64, o, h, l, c, v });
    }
    bars
}

fn run_replay(bars: &[Bar], cfg: StrategyCfg) -> (Strategy, f64) {
    let t0 = Instant::now();
    let mut s = Strategy::new(cfg);
    for b in bars {
        s.on_bar(b);
    }
    let elapsed = t0.elapsed();
    (s, elapsed.as_secs_f64())
}

fn print_trades(s: &Strategy) {
    let mut sl = 0;
    let mut tp = 0;
    let mut eos = 0;
    for t in &s.trades {
        match t.exit_type.as_str() {
            "SL" => sl += 1,
            "TP" => tp += 1,
            _ => eos += 1,
        }
    }
    let n = s.trades.len();
    let wins = s.trades.iter().filter(|t| t.pct > 0.0).count();
    let net = s.net();
    println!("trades      : {n}");
    println!("win rate    : {:.1}%", 100.0 * wins as f64 / n as f64);
    println!("net         : {net:+.2}%");
    println!("exit mix    : SL {sl} / TP {tp} / EOS {eos}");
    if !s.trades.is_empty() {
        let (first, last) = (&s.trades[0], &s.trades[s.trades.len() - 1]);
        println!("first/last  : {} .. {}", first.session, last.session);
    if std::env::var("FRVP_DUMP").is_ok() {
        for t in &s.trades {
            println!("DUMP\t{}\t{}\t{}\t{:.4}\t{:.4}\t{}\t{}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{}\t{:.4}\t{:.6}", t.session, t.side, t.exit_type, t.pct, t.r, t.entry_ts, t.exit_ts, t.entry, t.sl, t.tp, t.exit, t.bars_held, t.trail_a, t.slp);
        }
    }
    }
}

fn cmd_replay(path: &str, rr: f64, skip0: bool) {
    let bars = parse_csv(path);
    let cfg = StrategyCfg {
        rr,
        skip_hour0: skip0,
        trail_on: true,
        ladder: LADDER.to_vec(),
        same_bar_exits: true,
    };
    let (s, secs) = run_replay(&bars, cfg);
    println!("replay of {} bars in {secs:.4}s ({:.0} bars/s)", bars.len(), bars.len() as f64 / secs);
    print_trades(&s);
}

fn cmd_bench(path: &str) {
    let t0 = Instant::now();
    let bars = parse_csv(path);
    let parse_s = t0.elapsed().as_secs_f64();
    println!("parse {} bars: {parse_s:.4}s", bars.len());
    let cfg = StrategyCfg { rr: 2.0, skip_hour0: false, trail_on: true, ladder: LADDER.to_vec(), same_bar_exits: true };
    // 3 warm-up runs then 10 timed
    let _ = run_replay(&bars, cfg.clone());
    let mut times = Vec::new();
    for _ in 0..10 {
        let (_, secs) = run_replay(&bars, cfg.clone());
        times.push(secs);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let best = times[0];
    println!("full-engine replay best-of-10 : {best:.5}s  -> {:.0} bars/s  ({:.0} µs/bar)", bars.len() as f64 / best, best * 1e6 / bars.len() as f64);
    // FRVP/PoC hot loop benchmark: recompute profiles over rolling windows
    let t0 = Instant::now();
    let mut hits = 0usize;
    let mut prof_acc = 0.0f64;
    let mut idx = 0usize;
    while idx + 96 <= bars.len() {
        let win = &bars[idx..idx + 96];
        if let Some((_hi, lo, _w, prof, poc)) = engine::compute_poc(win, win[0].ts) {
            hits += 1;
            prof_acc += prof.iter().sum::<f64>() + poc - lo;
        }
        idx += 96;
    }
    let dur = t0.elapsed().as_secs_f64();
    println!("FRVP compute  : {hits} sessions in {dur:.4}s -> {:.0} sess/s", hits as f64 / dur);
    std::hint::black_box(prof_acc);
}

// ---------------------------------------------------------------------------
// live Binance public data-api feed (no keys). v-proxy = #trades per kline.
// ---------------------------------------------------------------------------
fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = ureq::get(url)
        .set("User-Agent", "frvp-rust/0.1")
        .timeout(std::time::Duration::from_secs(12))
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_json().map_err(|e| e.to_string())
}

fn fetch_klines(symbol: &str, limit: usize) -> Result<Vec<Bar>, String> {
    let url = format!(
        "https://data-api.binance.vision/api/v3/klines?symbol={symbol}&interval=15m&limit={limit}"
    );
    let j = http_json(&url)?;
    let arr = j.as_array().ok_or("bad payload")?;
    let now_ms = now_ms();
    let mut out = Vec::with_capacity(arr.len());
    for k in arr {
        let row = k.as_array().ok_or("bad row")?;
        let open_ms: i64 = row[0].as_i64().unwrap_or(0);
        if open_ms + 900_000 > now_ms {
            continue; // only closed bars
        }
        let get = |i: usize| row[i].as_str().and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        out.push(Bar {
            ts: open_ms / 1000,
            o: get(1),
            h: get(2),
            l: get(3),
            c: get(4),
            v: get(8), // number of trades -> tick proxy
        });
    }
    Ok(out)
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn cmd_live(symbol: &str, poll_s: u64, skip0: bool, rr: f64) {
    let cfg = StrategyCfg { rr, skip_hour0: skip0, trail_on: true, ladder: LADDER.to_vec(), same_bar_exits: true };
    let boot = match fetch_klines(symbol, 1000) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("bootstrap failed: {e}");
            return;
        }
    };
    println!("[frvp-live] {symbol} 15m — bootstrapped {} closed bars (PD profile warm)", boot.len());
    let mut s = Strategy::new(cfg);
    for b in &boot {
        s.on_bar(b);
    }
    let mut last_ts = boot.last().map(|b| b.ts).unwrap_or(0);
    println!("[state] session {} bias {:?} prev_poc {:?} trades {}", s.active_session(), s.bias, s.prev_poc, s.trades.len());
    loop {
        std::thread::sleep(std::time::Duration::from_secs(poll_s));
        match fetch_klines(symbol, 6) {
            Ok(nb) => {
                for b in nb {
                    if b.ts > last_ts {
                        s.on_bar(&b);
                        last_ts = b.ts;
                        println!(
                            "[bar] {} o {:.2} c {:.2} | session {} bias {:?} prev_poc {:.1} | trades {} net {:.2}% | last exit: {}",
                            iso(b.ts), b.o, b.c,
                            s.active_session(),
                            s.bias,
                            s.prev_poc.unwrap_or(0.0),
                            s.trades.len(),
                            s.net(),
                            s.trades.last().map(|t| t.exit_type.clone()).unwrap_or_default(),
                        );
                    }
                }
            }
            Err(e) => eprintln!("[feed] {e}"),
        }
    }
}

fn iso(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let (y, m, d) = engine::civil_from_days(days);
    let tod = ts.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", tod / 3600, (tod % 3600) / 60, tod % 60)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        println!("frvp (FRVP PoC core)\n  frvp replay <csv> [rr] [skip0 0|1]\n  frvp bench <csv>\n  frvp live [SYMBOL] [POLL_S] [rr] [skip0]");
        return;
    }
    match args[1].as_str() {
        "replay" => {
            let path = args.get(2).expect("csv path");
            let rr = args.get(3).and_then(|x| x.parse().ok()).unwrap_or(2.0);
            let skip0 = args.get(4).map(|x| x == "1").unwrap_or(false);
            cmd_replay(path, rr, skip0);
        }
        "bench" => cmd_bench(args.get(2).expect("csv path")),
        "live" => {
            let symbol = args.get(2).map(|s| s.as_str()).unwrap_or("PAXGUSDT");
            let poll = args.get(3).and_then(|x| x.parse().ok()).unwrap_or(10u64);
            let rr = args.get(4).and_then(|x| x.parse().ok()).unwrap_or(2.0);
            let skip0 = args.get(5).map(|x| x == "1").unwrap_or(true);
            cmd_live(symbol, poll, skip0, rr);
        }
        _ => {
            eprintln!("unknown command {}", args[1]);
        }
    }
    std::hint::black_box(());
}
