//! Market data feeds -> closed 15M bars. REST polling (Bybit v5 public /
//! Binance public data-api), with feed-health telemetry — the Python tick loop
//! swallowed all failures silently; here every poll updates a health struct
//! that is surfaced on /api/health and in the dashboard.

use serde_json::Value;
use std::time::Duration;
use crate::config::Config;
use crate::engine::Bar;

#[derive(Clone, Debug, Default)]
pub struct FeedHealth {
    pub polls: u64,
    pub fails: u64,
    pub consecutive_fails: u64,
    pub last_ok_ts: Option<i64>,
    pub last_error: Option<String>,
}

impl FeedHealth {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "polls": self.polls,
            "fails": self.fails,
            "consecutive_fails": self.consecutive_fails,
            "last_ok_ts": self.last_ok_ts,
            "last_error": self.last_error,
            "healthy": self.consecutive_fails < 5,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Venue {
    Bybit,
    BinanceSpot,
    BinanceFutures,
}

pub struct RestFeed {
    pub venue: Venue,
    pub symbol: String,
    pub interval_s: i64,
    base: String,
    pub bars: Vec<Bar>,
    pub last: Option<i64>,
    pub last_price: Option<f64>,
    pub health: FeedHealth,
    boot_limit: usize,
}

fn get_json(url: &str) -> Result<Value, String> {
    let resp = ureq::get(url)
        .set("User-Agent", "frvp-trader/2.0")
        .timeout(Duration::from_secs(12))
        .call()
        .map_err(|e| e.to_string())?;
    resp.into_json::<Value>().map_err(|e| e.to_string())
}

impl RestFeed {
    pub fn new(cfg: &Config) -> RestFeed {
        let venue = match cfg.resolved_data_venue().as_str() {
            "bybit" => Venue::Bybit,
            "futures" => Venue::BinanceFutures,
            _ => Venue::BinanceSpot,
        };
        let base = match venue {
            Venue::Bybit => cfg.bybit_base.clone(),
            Venue::BinanceSpot => crate::config::BINANCE_SPOT_DATA.to_string(),
            Venue::BinanceFutures => crate::config::BINANCE_FUT_DATA.to_string(),
        };
        RestFeed {
            venue,
            symbol: cfg.symbol.clone(),
            interval_s: 900,
            base,
            bars: Vec::new(),
            last: None,
            last_price: None,
            health: FeedHealth::default(),
            boot_limit: 1000,
        }
    }

    fn klines_url(&self, limit: usize) -> String {
        match self.venue {
            Venue::Bybit => format!(
                "{}/v5/market/kline?category=linear&symbol={}&interval=15&limit={}",
                self.base, self.symbol, limit.min(1000)
            ),
            Venue::BinanceSpot => format!(
                "{}/api/v3/klines?symbol={}&interval=15m&limit={}",
                self.base, self.symbol, limit.min(1000)
            ),
            Venue::BinanceFutures => format!(
                "{}/fapi/v1/klines?symbol={}&interval=15m&limit={}",
                self.base, self.symbol, limit.min(1000)
            ),
        }
    }

    fn ticker_url(&self) -> String {
        match self.venue {
            Venue::Bybit => format!(
                "{}/v5/market/tickers?category=linear&symbol={}",
                self.base, self.symbol
            ),
            Venue::BinanceSpot => format!("{}/api/v3/ticker/price?symbol={}", self.base, self.symbol),
            Venue::BinanceFutures => format!("{}/fapi/v1/ticker/price?symbol={}", self.base, self.symbol),
        }
    }

    /// Parse venue kline rows into Bars (oldest-first, closed only, new only).
    fn parse_rows(&self, rows: &[Value], now_ms: i64) -> Vec<Bar> {
        let mut out = Vec::new();
        for row in rows {
            let arr = match row.as_array() {
                Some(a) => a,
                None => continue,
            };
            if arr.len() < 7 {
                continue;
            }
            let ts_ms = arr[0].as_i64().unwrap_or_else(|| {
                arr[0].as_str().and_then(|s| s.parse().ok()).unwrap_or(0)
            });
            if ts_ms + self.interval_s * 1000 > now_ms {
                continue; // still forming
            }
            let g = |i: usize| -> f64 {
                arr[i].as_f64().unwrap_or_else(|| {
                    arr[i].as_str().and_then(|s| s.parse().ok()).unwrap_or(0.0)
                })
            };
            let ts = ts_ms / 1000;
            if let Some(last) = self.last {
                if ts <= last {
                    continue;
                }
            }
            // bybit: [start,o,h,l,c,volume,turnover]; binance: [start,o,h,l,c,
            // baseVol, closeTs, quoteVol, nTrades, ...] -> v = #trades (tick
            // proxy) for binance, real volume for bybit
            let v = match self.venue {
                Venue::Bybit => g(5),
                _ => if arr.len() > 8 { g(8) } else { g(5) },
            };
            out.push(Bar { ts, o: g(1), h: g(2), l: g(3), c: g(4), v });
        }
        out.sort_by_key(|b| b.ts);
        out
    }

    fn fetch_klines(&mut self, limit: usize, retries: u32) -> Result<Vec<Bar>, String> {
        let url = self.klines_url(limit);
        let mut last_err = String::new();
        for attempt in 0..retries.max(1) {
            match get_json(&url) {
                Ok(j) => {
                    let rows: Vec<Value> = match self.venue {
                        Venue::Bybit => {
                            let rc = j.get("retCode").and_then(|v| v.as_i64()).unwrap_or(0);
                            if rc != 0 {
                                Err(format!("bybit retCode {rc}: {}", j.get("retMsg").and_then(|v| v.as_str()).unwrap_or("")))?
                            }
                            let mut list = j
                                .pointer("/result/list")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default();
                            list.reverse(); // bybit is newest-first -> oldest-first
                            list
                        }
                        _ => j.as_array().cloned().unwrap_or_default(),
                    };
                    let now_ms = crate::engine::now_ts() * 1000;
                    return Ok(self.parse_rows(&rows, now_ms));
                }
                Err(e) => {
                    last_err = e;
                    if attempt + 1 < retries {
                        std::thread::sleep(Duration::from_millis(1500u64 * (attempt as u64 + 1)));
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Initial snapshot of closed bars (with retries — a failure here must not
    /// kill the process; the main loop retries bootstrap if it returns empty).
    pub fn bootstrap(&mut self) -> Result<Vec<Bar>, String> {
        let bars = self.fetch_klines(self.boot_limit, 6)?;
        self.bars = bars.clone();
        self.last = bars.last().map(|b| b.ts);
        self.health.polls += 1;
        self.health.consecutive_fails = 0;
        self.health.last_ok_ts = Some(crate::engine::now_ts());
        let _ = self.poll_ticker();
        Ok(bars)
    }

    /// Poll the tail of klines; returns new closed bars.
    pub fn poll(&mut self) -> Vec<Bar> {
        let n = 3 + (self.bars.len() / 500).min(17);
        match self.fetch_klines(n, 1) {
            Ok(newb) => {
                if !newb.is_empty() {
                    self.bars.extend(newb.iter().cloned());
                    self.last = self.bars.last().map(|b| b.ts);
                    // cap memory: keep last 30k bars
                    if self.bars.len() > 30_000 {
                        let drop = self.bars.len() - 30_000;
                        self.bars.drain(0..drop);
                    }
                }
                self.health.polls += 1;
                self.health.consecutive_fails = 0;
                self.health.last_ok_ts = Some(crate::engine::now_ts());
                newb
            }
            Err(e) => {
                self.health.fails += 1;
                self.health.consecutive_fails += 1;
                self.health.last_error = Some(e);
                Vec::new()
            }
        }
    }

    /// Cheap last-price refresh (the ~1s dashboard tick publisher).
    pub fn poll_ticker(&mut self) -> Option<f64> {
        let url = self.ticker_url();
        match get_json(&url) {
            Ok(j) => {
                let px = match self.venue {
                    Venue::Bybit => j
                        .pointer("/result/list/0/lastPrice")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok()),
                    _ => j.get("price").and_then(|v| v.as_str()).and_then(|s| s.parse::<f64>().ok()),
                };
                if let Some(p) = px {
                    self.last_price = Some(p);
                    self.health.consecutive_fails = 0;
                    self.health.last_ok_ts = Some(crate::engine::now_ts());
                    return Some(p);
                }
                self.health.fails += 1;
                self.health.consecutive_fails += 1;
                self.health.last_error = Some("ticker: no lastPrice".into());
                None
            }
            Err(e) => {
                self.health.fails += 1;
                self.health.consecutive_fails += 1;
                self.health.last_error = Some(e);
                None
            }
        }
    }

    /// Human label for state.source
    pub fn venue_label(&self) -> &'static str {
        match self.venue {
            Venue::Bybit => "bybit",
            Venue::BinanceSpot => "binance-spot",
            Venue::BinanceFutures => "binance-futures",
        }
    }
}
