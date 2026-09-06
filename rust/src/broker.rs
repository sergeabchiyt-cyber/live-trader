//! Order execution layer — mirrors strategy events to Binance (futures
//! testnet/mainnet: LIMIT entries, reduceOnly STOP_MARKET SL +
//! TAKE_PROFIT_MARKET TP, re-armed on trail moves; spot: LIMIT entry,
//! stop-limit + limit pair). The paper Ledger remains the PnL source of truth.
//!
//! Every order method is a no-op unless cfg.tradable() (MODE=testnet|live +
//! keys + BINANCE_DRY_RUN=0). Dry-run is always safe.
//!
//! v2: order quantity comes from the Ledger's sizing (same as paper), fixing
//! the legacy 100x ledger-vs-mirror inconsistency (AUDIT.md fix #3).

use serde_json::{json, Value};
use std::time::Duration;
use crate::config::Config;
use crate::engine::Trade;
use crate::ledger::{FeeModel, Ledger, Sizing, SizingCfg};

pub struct RestCli {
    key: String,
    secret: String,
    base: String,
    pub qty_prec: u32,
    pub min_notional: f64,
}

impl RestCli {
    pub fn new(key: &str, secret: &str, base: &str, symbol: &str, futures: bool) -> RestCli {
        let mut cli = RestCli {
            key: key.to_string(),
            secret: secret.to_string(),
            base: base.to_string(),
            qty_prec: if futures { 3 } else { 6 },
            min_notional: 5.0,
        };
        if futures && !symbol.is_empty() {
            // read lot precision / min notional from exchangeInfo (best effort)
            if let Ok(info) = cli.call("GET", "/fapi/v1/exchangeInfo", &[], false) {
                if let Some(syms) = info.get("symbols").and_then(|v| v.as_array()) {
                    for s in syms {
                        if s.get("symbol").and_then(|v| v.as_str()) == Some(symbol) {
                            cli.qty_prec = s
                                .get("quantityPrecision")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(3) as u32;
                            if let Some(filters) = s.get("filters").and_then(|v| v.as_array()) {
                                for f in filters {
                                    if f.get("filterType").and_then(|v| v.as_str()) == Some("MIN_NOTIONAL") {
                                        cli.min_notional = f
                                            .get("notional")
                                            .and_then(|v| v.as_f64())
                                            .unwrap_or(5.0);
                                    }
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
        cli
    }

    fn call(&self, method: &str, path: &str, params: &[(&str, String)], signed: bool) -> Result<Value, String> {
        let mut p: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        let mut url = format!("{}{}", self.base, path);
        let body: Option<String>;
        if signed {
            p.push(("timestamp".into(), format!("{}", now_ms())));
            p.push(("recvWindow".into(), "5000".into()));
            let qs = urlencode(&p);
            let sig = hmac_sha256_hex(self.secret.as_bytes(), qs.as_bytes());
            body = Some(format!("{}&signature={}", qs, sig));
        } else {
            body = None;
            if !p.is_empty() {
                url.push('?');
                url.push_str(&urlencode(&p));
            }
        }
        let mut req = ureq::request(method, &url)
            .set("X-MBX-APIKEY", &self.key)
            .timeout(Duration::from_secs(15));
        if body.is_some() {
            req = req.set("Content-Type", "application/x-www-form-urlencoded");
        }
        let resp = match &body {
            Some(b) => req.send_string(b),
            None => req.call(),
        };
        match resp {
            Ok(r) => r.into_json::<Value>().map_err(|e| e.to_string()),
            Err(ureq::Error::Status(code, r)) => {
                let b = r.into_string().unwrap_or_default();
                Err(format!("HTTP {code}: {}", b.chars().take(200).collect::<String>()))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn fmt_qty(&self, q: f64) -> String {
        format!("{:.*}", self.qty_prec as usize, q)
    }

    // ---- futures (USDT-M) order routing -----------------------------------
    pub fn fut_limit(&self, symbol: &str, side: &str, qty: f64, price: f64) -> Result<Value, String> {
        self.call("POST", "/fapi/v1/order", &[
            ("symbol", symbol.into()),
            ("side", side.into()),
            ("type", "LIMIT".into()),
            ("timeInForce", "GTC".into()),
            ("quantity", self.fmt_qty(qty)),
            ("price", format!("{price:.2}")),
        ], true)
    }

    pub fn fut_stop(&self, symbol: &str, side: &str, qty: f64, stop: f64) -> Result<Value, String> {
        self.call("POST", "/fapi/v1/order", &[
            ("symbol", symbol.into()),
            ("side", side.into()),
            ("type", "STOP_MARKET".into()),
            ("reduceOnly", "true".into()),
            ("quantity", self.fmt_qty(qty)),
            ("stopPrice", format!("{stop:.2}")),
        ], true)
    }

    pub fn fut_tp(&self, symbol: &str, side: &str, qty: f64, stop: f64) -> Result<Value, String> {
        self.call("POST", "/fapi/v1/order", &[
            ("symbol", symbol.into()),
            ("side", side.into()),
            ("type", "TAKE_PROFIT_MARKET".into()),
            ("reduceOnly", "true".into()),
            ("quantity", self.fmt_qty(qty)),
            ("stopPrice", format!("{stop:.2}")),
        ], true)
    }

    pub fn fut_cancel_all(&self, symbol: &str) -> Result<Value, String> {
        self.call("DELETE", "/fapi/v1/allOpenOrders", &[("symbol", symbol.into())], true)
    }

    pub fn fut_set_leverage(&self, symbol: &str, leverage: u32) -> Result<Value, String> {
        self.call("POST", "/fapi/v1/leverage", &[
            ("symbol", symbol.into()),
            ("leverage", leverage.to_string()),
        ], true)
    }

    // ---- spot order routing -----------------------------------------------
    pub fn spot_limit(&self, symbol: &str, side: &str, qty: f64, price: f64) -> Result<Value, String> {
        self.call("POST", "/api/v3/order", &[
            ("symbol", symbol.into()),
            ("side", side.into()),
            ("type", "LIMIT".into()),
            ("timeInForce", "GTC".into()),
            ("quantity", self.fmt_qty(qty)),
            ("price", format!("{price:.2}")),
        ], true)
    }

    pub fn spot_stop_limit(&self, symbol: &str, side: &str, qty: f64, stop: f64, limit: f64) -> Result<Value, String> {
        self.call("POST", "/api/v3/order", &[
            ("symbol", symbol.into()),
            ("side", side.into()),
            ("type", "STOP_LOSS_LIMIT".into()),
            ("timeInForce", "GTC".into()),
            ("quantity", self.fmt_qty(qty)),
            ("stopPrice", format!("{stop:.2}")),
            ("price", format!("{limit:.2}")),
        ], true)
    }

    pub fn spot_cancel_all(&self, symbol: &str) -> Result<Value, String> {
        self.call("DELETE", "/api/v3/openOrders", &[("symbol", symbol.into())], true)
    }
}

fn now_ms() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.bytes() {
        match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(c as char),
            _ => out.push_str(&format!("%{:02X}", c)),
        }
    }
    out
}

fn urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", escape(k), escape(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn hmac_sha256_hex(key: &[u8], msg: &[u8]) -> String {
    use ring::hmac::{Key, HMAC_SHA256};
    let key = Key::new(HMAC_SHA256, key);
    let tag = ring::hmac::sign(&key, msg);
    tag.as_ref().iter().map(|b| format!("{:02x}", b)).collect()
}

// ---------------------------------------------------------------------------

struct MirrorState {
    side: i8,
    qty: f64,
    sl: f64,
    tp: f64,
    armed: bool,
}

pub struct Broker {
    cfg: Config,
    cli: Option<RestCli>,
    pub ledger: Ledger,
    status: Vec<Value>,
    mirror: Option<MirrorState>,
    sizing: Option<Sizing>,
}

impl Broker {
    pub fn new(cfg: &Config) -> Broker {
        let fee = FeeModel {
            maker_pct: cfg.fee_maker_pct,
            taker_pct: cfg.fee_taker_pct,
            slip_pct: cfg.sl_slip_pct,
        };
        let scfg = SizingCfg {
            legacy: cfg.legacy_sizing,
            position_usd: cfg.position_usd,
            risk_pct: cfg.risk_pct,
            leverage_cap: cfg.leverage_cap,
        };
        let ledger = Ledger::new(cfg.paper_start_usdt, scfg, fee);
        let mut b = Broker {
            cfg: cfg.clone(),
            cli: None,
            ledger,
            status: Vec::new(),
            mirror: None,
            sizing: None,
        };
        if cfg.tradable() && !cfg.api_key.is_empty() {
            let futures = cfg.venue == "futures";
            let cli = RestCli::new(&cfg.api_key, &cfg.api_secret, &cfg.order_base(), &cfg.symbol, futures);
            if futures {
                match cli.fut_set_leverage(&cfg.symbol, 1) {
                    Ok(_) => b.log(format!("leverage set to 1x on {}", cfg.symbol)),
                    Err(e) => b.log(format!("leverage set failed: {e}")),
                }
            }
            b.cli = Some(cli);
        }
        b
    }

    fn log(&mut self, msg: String) {
        let d = json!({"ts": now_ms() as f64 / 1000.0, "msg": msg});
        self.status.push(d);
        if self.status.len() > 200 {
            let drop = self.status.len() - 150;
            self.status.drain(0..drop);
        }
        println!("[broker] {msg}");
    }

    pub fn status_json(&self) -> Value {
        self.status.iter().rev().take(20).cloned().collect::<Vec<_>>().into()
    }

    fn tradable(&self) -> bool {
        self.cfg.tradable() && self.cli.is_some()
    }

    fn arm(&mut self) {
        let (side, qty, sl, tp) = match &self.mirror {
            Some(m) => (m.side, m.qty, m.sl, m.tp),
            None => return,
        };
        if !self.tradable() {
            return;
        }
        let close_side = if side == 1 { "SELL" } else { "BUY" };
        let futures = self.cfg.venue == "futures";
        let symbol = self.cfg.symbol.clone();
        let res = match &self.cli {
            None => return,
            Some(cli) => (|| -> Result<(), String> {
                if futures {
                    cli.fut_cancel_all(&symbol)?;
                    cli.fut_stop(&symbol, close_side, qty, sl)?;
                    cli.fut_tp(&symbol, close_side, qty, tp)?;
                } else {
                    cli.spot_cancel_all(&symbol)?;
                    cli.spot_stop_limit(&symbol, close_side, qty, sl, sl)?;
                    cli.spot_limit(&symbol, close_side, qty, tp)?;
                }
                Ok(())
            })(),
        };
        match res {
            Ok(()) => {
                if let Some(m) = &mut self.mirror {
                    m.armed = true;
                }
                self.log(format!("protection armed: SL {sl} TP {tp}"));
            }
            Err(e) => {
                if let Some(m) = &mut self.mirror {
                    m.armed = false;
                }
                self.log(format!("arm failed: {e}"));
            }
        }
    }

    /// Live open event: size from the ledger, mirror the entry if tradable.
    pub fn on_open(&mut self, side: i8, entry: f64, sl: f64, tp: f64, slp: f64) {
        let sizing = self.ledger.on_open(entry, slp);
        self.sizing = Some(sizing);
        let prec = self.cli.as_ref().map(|c| c.qty_prec).unwrap_or(3);
        let min_notional = self.cli.as_ref().map(|c| c.min_notional).unwrap_or(5.0);
        let q = round_to(sizing.qty, prec);
        let tradable = self.tradable();
        let futures = self.cfg.venue == "futures";
        let symbol = self.cfg.symbol.clone();
        let order_side = if side == 1 { "BUY" } else { "SELL" };
        if tradable {
            if q * entry < min_notional {
                self.log(format!(
                    "entry skipped: qty {q} x {entry:.2} below venue min notional {min_notional}"
                ));
            } else {
                let r = match &self.cli {
                    None => return,
                    Some(cli) => {
                        if futures {
                            cli.fut_limit(&symbol, order_side, q, entry)
                        } else {
                            cli.spot_limit(&symbol, order_side, q, entry)
                        }
                    }
                };
                match r {
                    Ok(o) => self.log(format!(
                        "entry order {} {} qty {q} @ {entry:.2} (notional {:.0} USDT, risk {:.2} USDT)",
                        o.get("orderId").map(|v| v.to_string()).unwrap_or_default(),
                        if side == 1 { "long" } else { "short" },
                        sizing.notional_usd, sizing.risk_usd
                    )),
                    Err(e) => self.log(format!("entry order failed: {e}")),
                }
            }
        } else {
            self.log(format!(
                "(dry) entry {} qty {q} @ {entry:.2} (notional {:.0} USDT, risk {:.2} USDT)",
                if side == 1 { "long" } else { "short" },
                sizing.notional_usd, sizing.risk_usd
            ));
        }
        self.mirror = Some(MirrorState { side, qty: q, sl, tp, armed: false });
        self.arm();
    }

    pub fn on_trail(&mut self, sl: f64) {
        if let Some(m) = &mut self.mirror {
            m.sl = sl;
        }
        if self.tradable() {
            self.arm();
        } else {
            self.log(format!("(dry) trail -> SL {sl:.2}"));
        }
    }

    /// Live close event: apply the ledger (fees + sizing) and clean up orders.
    /// Returns the enriched close payload.
    pub fn on_close(&mut self, t: &Trade) -> Value {
        let enriched = self.ledger.on_close(t, t.exit_ts);
        let tradable = self.tradable();
        let futures = self.cfg.venue == "futures";
        let symbol = self.cfg.symbol.clone();
        if tradable {
            let r = match &self.cli {
                None => None,
                Some(cli) => Some(if futures {
                    cli.fut_cancel_all(&symbol)
                } else {
                    cli.spot_cancel_all(&symbol)
                }),
            };
            match r {
                Some(Ok(_)) => self.log(format!(
                    "closed {} @ {:.2} net {} USDT (fees {}) equity {}",
                    t.exit_type, t.exit, enriched["pnl_usd"], enriched["fee_usd"], enriched["equity"]
                )),
                Some(Err(e)) => self.log(format!("cancel on close failed: {e}")),
                None => {}
            }
        } else {
            self.log(format!(
                "closed {} @ {:.2} net {} USDT (fees {}) equity {}",
                t.exit_type, t.exit, enriched["pnl_usd"], enriched["fee_usd"], enriched["equity"]
            ));
        }
        self.mirror = None;
        self.sizing = None;
        enriched
    }

    /// Warm-up replay of historical trades: ledger accounting only (no orders,
    /// no hub publishing) — same as the legacy bootstrap behaviour.
    /// Returns the enriched close payload (for the trades log).
    pub fn quiet_close(&mut self, t: &Trade) -> Value {
        self.ledger.apply_warm(t)
    }

    pub fn balance(&self) -> Value {
        let armed = self.mirror.as_ref().map(|m| m.armed).unwrap_or(false);
        self.ledger.balance(&self.cfg.resolved_mode(), armed)
    }

}

fn round_to(q: f64, prec: u32) -> f64 {
    let m = 10f64.powi(prec as i32);
    (q * m).round() / m
}
