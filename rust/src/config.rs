//! Configuration — ENV-driven, drop-in compatible with the Python service's
//! variable names (SYMBOL, DATA_VENUE, MODE, BINANCE_*, RR, SKIP_HOUR0, ...).
//! New economics knobs (SL_MODE, FEE_*, RISK_PCT, LEVERAGE_CAP, LEGACY_SIZING)
//! default to the honest post-audit behaviour; see AUDIT.md.

use std::env;

pub const DEFAULT_LADDER: [(f64, f64); 3] = [(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)];

pub const BYBIT_DEFAULT: &str = "https://api.bybit.com";
pub const BINANCE_SPOT_DATA: &str = "https://data-api.binance.vision";
pub const BINANCE_FUT_DATA: &str = "https://fapi.binance.com";
pub const BINANCE_FUT_TESTNET: &str = "https://testnet.binancefuture.com";
pub const BINANCE_FUT_MAINNET: &str = "https://fapi.binance.com";
pub const BINANCE_SPOT_TESTNET: &str = "https://testnet.binance.vision";
pub const BINANCE_SPOT_MAINNET: &str = "https://api.binance.com";

#[derive(Clone, Debug)]
pub struct Config {
    // mode / venues
    pub mode: String,          // auto|paper|testnet|live
    pub venue: String,         // order venue: spot | futures
    pub data_venue: String,    // bybit | spot | futures
    pub symbol: String,
    // economics (post-audit defaults)
    pub sl_touch: bool,        // SL_MODE=touch (default) | close (legacy parity)
    pub fee_maker_pct: f64,    // FEE_MAKER_PCT (0 disables)
    pub fee_taker_pct: f64,    // FEE_TAKER_PCT (0 disables)
    pub sl_slip_pct: f64,      // SL_SLIP_PCT adverse slippage on stop fills
    pub legacy_sizing: bool,   // LEGACY_SIZING=1 -> fixed POSITION_USD notional
    pub position_usd: f64,     // legacy fixed notional
    pub risk_pct: f64,         // % of paper equity risked per trade
    pub leverage_cap: f64,     // notional cap = equity * LEVERAGE_CAP
    // strategy
    pub rr: f64,
    pub skip_hour0: bool,
    pub trail_on: bool,
    pub ladder: Vec<(f64, f64)>,
    pub same_bar: bool,
    // runtime
    pub poll_s: f64,
    pub tick_s: f64,
    pub port: u16,
    pub host: String,
    pub paper_start_usdt: f64,
    // endpoints / credentials
    pub bybit_base: String,
    pub api_key: String,
    pub api_secret: String,
    pub testnet: bool,
    pub live_ack: bool,
    pub dry_run: bool,
    // feed
    pub feed: String, // rest|ws (bybit ignores; binance = REST in v2)
}

impl Config {
    pub fn from_env() -> Config {
        let e = |k: &str, d: &str| env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
        let f = |k: &str, d: f64| env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d);
        let b = |k: &str, d: bool| {
            env::var(k).ok().map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")).unwrap_or(d)
        };
        let testnet_keys = env::var("BINANCE_TESTNET_API_KEY").is_ok()
            || env::var("BINANCE_TESTNET_API_SECRET").is_ok();
        let mut api_key = e("BINANCE_API_KEY", "");
        let mut api_secret = e("BINANCE_API_SECRET", "");
        if let Ok(k) = env::var("BINANCE_TESTNET_API_KEY") {
            api_key = k;
        }
        if let Ok(s) = env::var("BINANCE_TESTNET_API_SECRET") {
            api_secret = s;
        }
        let ladder = parse_ladder(&e("TRAIL_LADDER", ""));
        Config {
            mode: e("MODE", "auto"),
            venue: e("VENUE", "spot").to_lowercase(),
            data_venue: e("DATA_VENUE", "auto").to_lowercase(),
            symbol: e("SYMBOL", "PAXGUSDT").to_uppercase(),
            sl_touch: e("SL_MODE", "touch").to_lowercase() != "close",
            fee_maker_pct: f("FEE_MAKER_PCT", 0.02),
            fee_taker_pct: f("FEE_TAKER_PCT", 0.055),
            sl_slip_pct: f("SL_SLIP_PCT", 0.0),
            legacy_sizing: b("LEGACY_SIZING", false),
            position_usd: f("POSITION_USD", 100.0),
            risk_pct: f("RISK_PCT", 0.5),
            leverage_cap: f("LEVERAGE_CAP", 3.0),
            rr: f("RR", 2.0),
            skip_hour0: b("SKIP_HOUR0", true),
            trail_on: b("TRAIL", true),
            ladder,
            same_bar: b("SAME_BAR", true),
            poll_s: f("POLL_S", 5.0),
            tick_s: f("TICK_S", 1.0),
            port: env::var("PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(8765),
            host: e("HOST", "0.0.0.0"),
            paper_start_usdt: f("PAPER_START_USDT", 10000.0),
            bybit_base: e("BYBIT_BASE", BYBIT_DEFAULT).trim_end_matches('/').to_string(),
            api_key,
            api_secret,
            testnet: b("BINANCE_TESTNET", false) || testnet_keys,
            live_ack: env::var("BINANCE_LIVE_ACK").unwrap_or_default().to_lowercase() == "yes",
            dry_run: b("BINANCE_DRY_RUN", true),
            feed: e("FEED", "rest").to_lowercase(),
        }
    }

    pub fn resolved_mode(&self) -> String {
        let m = self.mode.to_lowercase();
        if m == "paper" || m == "testnet" || m == "live" {
            return m;
        }
        if !self.api_key.is_empty() && !self.api_secret.is_empty() && self.live_ack {
            return "live".to_string();
        }
        if self.testnet {
            return "testnet".to_string();
        }
        "paper".to_string()
    }

    pub fn tradable(&self) -> bool {
        let m = self.resolved_mode();
        (m == "live" || m == "testnet") && !self.dry_run
    }

    pub fn display_mode(&self) -> String {
        let m = self.resolved_mode();
        if self.dry_run && (m == "live" || m == "testnet") {
            format!("{m} (dry-run)")
        } else {
            m
        }
    }

    /// Market-data venue ("bybit" | "spot" | "futures").
    pub fn resolved_data_venue(&self) -> String {
        match self.data_venue.as_str() {
            "bybit" | "spot" | "futures" => self.data_venue.clone(),
            _ => {
                if self.venue == "futures" {
                    "futures".to_string()
                } else {
                    "spot".to_string()
                }
            }
        }
    }

    /// Order-routing base URL (Binance spot or USDT-M futures testnet/mainnet).
    pub fn order_base(&self) -> String {
        let live = self.resolved_mode() == "live";
        if self.venue == "futures" {
            if live {
                BINANCE_FUT_MAINNET.to_string()
            } else {
                BINANCE_FUT_TESTNET.to_string()
            }
        } else if live {
            BINANCE_SPOT_MAINNET.to_string()
        } else {
            BINANCE_SPOT_TESTNET.to_string()
        }
    }

    pub fn source_label(&self) -> String {
        match self.resolved_data_venue().as_str() {
            "bybit" => format!("{} live via Bybit public REST klines (15m, poll)", self.symbol),
            v => format!("{} live via Binance {} REST klines (15m, poll)", self.symbol, v),
        }
    }
}

fn parse_ladder(s: &str) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    for tok in s.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        let mut it = tok.split(':');
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            if let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse()) {
                out.push((a, b));
            }
        }
    }
    if out.is_empty() {
        return DEFAULT_LADDER.to_vec();
    }
    out.sort_by(|x: &(f64, f64), y: &(f64, f64)| x.0.partial_cmp(&y.0).unwrap());
    out
}

pub fn banner(c: &Config) {
    let icon = match c.resolved_mode().as_str() {
        "paper" => "\u{1f9fe}",
        "testnet" => "\u{1f6e1}",
        _ => "\u{1f534}",
    };
    println!("====================================================================");
    println!("  {icon} FRVP live trader v2 (Rust)   mode = {}", c.display_mode().to_uppercase());
    println!("     data     : {} ({})", c.source_label(), c.resolved_data_venue());
    if c.resolved_mode() != "paper" {
        println!("     orders   -> {} (venue={}, dry-run={})", c.order_base(), c.venue, c.dry_run);
    } else {
        println!("     paper fills (no exchange orders). Set MODE/BINANCE_* env to go live.");
    }
    println!("     settings : RR={}  skip-00:01={}  trail={} {:?}",
             c.rr, c.skip_hour0, c.trail_on, c.ladder);
    println!("     fills    : SL_MODE={}  fees maker/taker={:.3}/{:.3}%  slip={:.3}%",
             if c.sl_touch { "touch" } else { "close" }, c.fee_maker_pct, c.fee_taker_pct, c.sl_slip_pct);
    println!("     sizing   : {}", if c.legacy_sizing {
        format!("legacy fixed {} USDT notional", c.position_usd)
    } else {
        format!("risk {:.2}% of equity, notional cap {:.1}x equity", c.risk_pct, c.leverage_cap)
    });
    if c.feed == "ws" && c.resolved_data_venue() != "bybit" {
        println!("     note     : FEED=ws requested — v2 uses REST polling on all venues (15m strategy)");
    }
    println!("     runtime  : http://{}:{}  poll={}s tick={}s", c.host, c.port, c.poll_s, c.tick_s);
    if c.tradable() && c.venue == "futures" && c.resolved_mode() == "live" {
        println!("     \u{26a0} LIVE REAL MONEY on USDT-M perps \u{2014} leveraged!");
    }
    println!("====================================================================");
}
