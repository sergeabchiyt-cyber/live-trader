//! Low-latency FRVP PoC strategy core — an exact Rust port of the validated
//! `live/strategy.py` engine (which in turn is a 1:1 port of the backtest
//! `engine.py`).  Same session calendar, FRVP/PoC math, ATR stop, milestone
//! trailing and fill conventions, so signals are byte-for-byte equivalent.

pub const DAY_OPEN_HOUR: i64 = 22;          // 22:00 UTC session open
pub const BINS: usize = 48;
pub const ATR_N: usize = 14;
pub const SL_MIN: f64 = 0.0005;
pub const SL_MAX: f64 = 0.005;

#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub ts: i64,      // seconds UTC (bar open)
    pub o: f64, pub h: f64, pub l: f64, pub c: f64, pub v: f64,
}

#[derive(Clone, Debug)]
pub struct Trade {
    pub session: String,
    pub side: i8,              // 1 long, -1 short
    pub entry: f64, pub sl: f64, pub tp: f64, pub exit: f64,
    pub exit_type: String,     // TP | SL | EOS
    #[allow(dead_code)] pub rr: f64, pub slp: f64,
    pub pct: f64, pub r: f64,
    pub entry_ts: i64, pub exit_ts: i64,
    pub bars_held: i64, pub trail_a: f64,
}

// ---------------------------------------------------------------------------
// civil date helpers (no chrono dependency).  Epoch 1970-01-01 == Thursday.
// Howard Hinnant's civil_from_days.
// ---------------------------------------------------------------------------
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let zz = z + 719_468;
    let era = if zz >= 0 { zz / 146_097 } else { (zz - 146_096) / 146_097 };
    let doe = (zz - era * 146_097) as u64; // [0,146096]
    let yoe = ((doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365) as i64; // [0,399]
    let y = yoe + era * 400;
    let doy = doe as i64 - (365 * yoe + yoe / 4 - yoe / 100); // [0,365]
    let mp = (5 * doy + 2) / 153; // [0,11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1,31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1,12]
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Weekday of the date that owns `ts` under the 22:00 roll (0=Mon..6=Sun).
pub fn session_date_weekday(ts: i64) -> u32 {
    let dt = ts.div_euclid(86400);
    let wd0 = (dt + 3).rem_euclid(7) as u32; // 1970-01-01 Thu -> 3
    if ts.rem_euclid(86400) >= DAY_OPEN_HOUR * 3600 {
        wd0
    } else {
        (wd0 + 6) % 7 // previous day
    }
}

/// date-key string (YYYY-MM-DD) of the session that owns ts (22:00 roll)
pub fn session_date_key(ts: i64) -> String {
    let dt = ts.div_euclid(86400);
    let rollback = ts.rem_euclid(86400) < DAY_OPEN_HOUR * 3600;
    let days = if rollback { dt - 1 } else { dt };
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// FRVP of one session: 4H buckets of 15M bars, tick volume spread uniformly
/// over each bucket's range -> BINS bins -> PoC = centre of max bin.
pub fn compute_poc(session: &[Bar], start_ts: i64) -> Option<(f64, f64, f64, Vec<f64>, f64)> {
    if session.len() < 8 {
        return None;
    }
    let hi = session.iter().map(|b| b.h).fold(f64::MIN, f64::max);
    let lo = session.iter().map(|b| b.l).fold(f64::MAX, f64::min);
    if hi <= lo {
        return None;
    }
    let w = (hi - lo) / BINS as f64;
    let mut prof = vec![0.0f64; BINS];
    // bucket bars into 4h groups in order
    let mut buckets: Vec<Vec<&Bar>> = Vec::new();
    for b in session {
        let idx = ((b.ts - start_ts) / 14400) as usize;
        while buckets.len() <= idx {
            buckets.push(Vec::new());
        }
        buckets[idx].push(b);
    }
    for b4 in buckets {
        let vh = b4.iter().map(|b| b.h).fold(f64::MIN, f64::max);
        let vl = b4.iter().map(|b| b.l).fold(f64::MAX, f64::min);
        let vv: f64 = b4.iter().map(|b| b.v).sum();
        let a = (((vl - lo) / w) as isize).clamp(0, BINS as isize - 1) as usize;
        let z = ((((vh.min(hi) - lo) / w) as isize)).clamp(a as isize, BINS as isize - 1) as usize;
        let add = vv / (z - a + 1) as f64;
        for k in a..=z {
            prof[k] += add;
        }
    }
    // Python parity: prof.index(max(prof)) picks the FIRST index of the max
    // value. (max_by would pick the last on ties, shifting PoC by bins.)
    let mut maxidx = 0usize;
    let mut maxv = prof[0];
    for (i, &v) in prof.iter().enumerate() {
        if v > maxv {
            maxv = v;
            maxidx = i;
        }
    }
    let poc = lo + (maxidx as f64 + 0.5) * w;
    Some((hi, lo, w, prof, poc))
}

#[derive(Clone)]
pub struct StrategyCfg {
    pub rr: f64,
    pub skip_hour0: bool,
    pub trail_on: bool,
    pub ladder: Vec<(f64, f64)>, // (progress, R multiple), sorted by progress
    pub same_bar_exits: bool,
}

pub struct Strategy {
    pub cfg: StrategyCfg,
    pub bars: Vec<Bar>,
    atr: f64,
    prev_c: Option<f64>,
    pub prev_poc: Option<f64>,
    pub prev_prof_hi: f64,
    pub prev_prof_lo: f64,
    active_sd: Option<String>,
    active_wd: Option<u32>,
    active_start: i64,
    session_bars: Vec<Bar>,
    pub open0: f64,
    pub bias: Option<i8>,
    session_traded: bool,
    eos_done: bool,
    last_bar_ts: i64,
    pos: Option<Pos>,
    pub trades: Vec<Trade>,
}

struct Pos {
    side: i8,
    entry: f64,
    sl: f64,
    sl0: f64,
    tp: f64,
    slp: f64,
    session: String,
    entry_ts: i64,
    rr: f64,
    runx: f64,
    trail_a: f64,
    bars_held: i64,
}

impl Strategy {
    pub fn new(cfg: StrategyCfg) -> Self {
        Self {
            cfg,
            bars: Vec::new(),
            atr: 0.0,
            prev_c: None,
            prev_poc: None,
            prev_prof_hi: 0.0,
            prev_prof_lo: 0.0,
            active_sd: None,
            active_wd: None,
            active_start: 0,
            session_bars: Vec::new(),
            open0: 0.0,
            bias: None,
            session_traded: false,
            eos_done: false,
            last_bar_ts: -1,
            pos: None,
            trades: Vec::new(),
        }
    }

    fn feed_atr(&mut self, b: &Bar) {
        let tr = match self.prev_c {
            None => b.h - b.l,
            Some(pc) => (b.h - b.l)
                .max((b.h - pc).abs())
                .max((b.l - pc).abs()),
        };
        self.atr = if self.atr == 0.0 {
            tr
        } else {
            (self.atr * (ATR_N as f64 - 1.0) + tr) / ATR_N as f64
        };
        self.prev_c = Some(b.c);
    }

    fn open_session(&mut self, sd: String, wd: u32, ts: i64, b: &Bar) {
        self.active_sd = Some(sd);
        self.active_wd = Some(wd);
        let days = ts.div_euclid(86400);
        self.active_start = (days) * 86400 + DAY_OPEN_HOUR * 3600;
        // NB: session start belongs to the session date (bar at/after 22:00 of sd)
        self.session_bars.clear();
        self.open0 = b.o;
        self.bias = None;
        self.session_traded = false;
        self.eos_done = false;
    }

    /// Feed one closed 15M bar.
    pub fn on_bar(&mut self, b: &Bar) {
        if b.ts <= self.last_bar_ts {
            return; // duplicate / stale
        }
        self.bars.push(*b);
        self.last_bar_ts = b.ts;
        self.feed_atr(b);

        let wd = session_date_weekday(b.ts);
        let sd = session_date_key(b.ts);
        let in_day = matches!(wd, 6 | 0 | 1 | 2 | 3); // Sun..Thu session start

        if !in_day {
            // weekend: close any straggler position at last session close once
            if self.pos.is_some() && !self.eos_done {
                let fill = self.session_bars.last().map(|x| x.c).unwrap_or(b.c);
                self.close_pos("EOS", fill, b.ts);
                self.eos_done = true;
            }
            return;
        }

        let mut opened_new = false;
        if self.active_sd.is_none() {
            self.open_session(sd, wd, b.ts, b);
            opened_new = true;
        } else if self.active_sd.as_deref() != Some(sd.as_str()) {
            // session boundary: finalise prior session, open new
            if self.pos.is_some() {
                let fill = self.session_bars.last().map(|x| x.c).unwrap_or(b.o);
                self.close_pos("EOS", fill, b.ts);
            }
            if !self.session_bars.is_empty() {
                let poc = compute_poc(&self.session_bars, self.active_start);
                match poc {
                    Some((hi, lo, _w, _prof, poc)) => {
                        self.prev_poc = Some(poc);
                        self.prev_prof_hi = hi;
                        self.prev_prof_lo = lo;
                    }
                    None => {
                        self.prev_poc = None;
                        self.prev_prof_hi = 0.0;
                        self.prev_prof_lo = 0.0;
                    }
                }
            } else {
                self.prev_poc = None;
            }
            self.open_session(sd, wd, b.ts, b);
            if let Some(poc) = self.prev_poc {
                self.bias = if self.open0 >= poc { Some(1) } else { Some(-1) };
            }
            opened_new = true;
        }
        self.session_bars.push(*b);
        self.eos_done = false;

        // manage open position
        if self.pos.is_some() {
            self.manage(b);
        }
        // entry trigger
        if self.pos.is_none() && !self.session_traded && self.prev_poc.is_some() && self.bias.is_some() {
            let entered = self.maybe_enter(b);
            if entered && self.cfg.same_bar_exits && self.pos.is_some() {
                self.manage(b);
            }
        }
        if opened_new && std::env::var("FRVP_TRACE").is_ok() {
            let sdv = self.active_sd.clone().unwrap_or_default();
            let poc = match self.prev_poc {
                Some(p) => format!("{p:.4}"),
                None => "None".to_string(),
            };
            let bv = match self.bias {
                Some(x) => format!("{x}"),
                None => "None".to_string(),
            };
            let pos = match &self.pos {
                Some(q) => format!("side{}@{}", q.side, q.entry_ts),
                None => "-".to_string(),
            };
            println!("SS\t{sdv}\topen0 {:.4}\tpoc {poc}\tbias {bv}\tn {}\tpos {pos}", self.open0, self.session_bars.len());
        }
    }

    fn maybe_enter(&mut self, b: &Bar) -> bool {
        if self.cfg.skip_hour0 {
            let hh = (b.ts.rem_euclid(86400)) / 3600;
            if hh == 0 {
                return false; // 00:00-01:00 excluded window
            }
        }
        let poc = self.prev_poc.unwrap();
        match self.bias {
            Some(1) if b.l <= poc => self.enter(1, b, poc),
            Some(-1) if b.h >= poc => self.enter(-1, b, poc),
            _ => false,
        }
    }

    fn enter(&mut self, side: i8, b: &Bar, entry_price: f64) -> bool {
        if std::env::var("FRVP_TRACE").is_ok() {
            println!("TR E {}/side{} entry{:.4} atr{:.4} sd{}", b.ts, side, entry_price, self.atr, self.active_sd.clone().unwrap_or_default());
        }
        self.session_traded = true;
        let slp = (self.atr / entry_price).clamp(SL_MIN, SL_MAX);
        let sl0 = if side == 1 {
            entry_price * (1.0 - slp)
        } else {
            entry_price * (1.0 + slp)
        };
        let tp = if side == 1 {
            entry_price * (1.0 + slp * self.cfg.rr)
        } else {
            entry_price * (1.0 - slp * self.cfg.rr)
        };
        self.pos = Some(Pos {
            side,
            entry: entry_price,
            sl: sl0,
            sl0,
            tp,
            slp,
            session: self.active_sd.clone().unwrap_or_default(),
            entry_ts: b.ts,
            rr: self.cfg.rr,
            runx: entry_price,
            trail_a: 0.0,
            bars_held: 0,
        });
        true
    }

    fn manage(&mut self, b: &Bar) {
        if self.pos.is_none() {
            return;
        }
        let _trace = std::env::var("FRVP_TRACE").is_ok();
        let p = self.pos.as_mut().unwrap();
        p.bars_held += 1;
        if p.side == 1 {
            p.runx = p.runx.max(b.h);
        } else {
            p.runx = p.runx.min(b.l);
        }
        if self.cfg.trail_on {
            let mut best_a = p.trail_a;
            for &(prog, ar) in &self.cfg.ladder {
                let ref_px = p.entry + (p.tp - p.entry) * prog;
                let hit = if p.side == 1 { p.runx >= ref_px } else { p.runx <= ref_px };
                if hit && ar > best_a {
                    best_a = ar;
                }
            }
            if best_a != p.trail_a {
                let prev_a = p.trail_a;
                p.trail_a = best_a;
                let cand = if p.side == 1 {
                    p.entry + best_a * p.slp * p.entry
                } else {
                    p.entry - best_a * p.slp * p.entry
                };
                p.sl = if p.side == 1 { p.sl.max(cand) } else { p.sl.min(cand) };
                if _trace {
                    println!("TR M {}/ts{} a{}->{} runx{:.4} sl{:.4} tp{:.4}", b.ts, b.ts, prev_a, best_a, p.runx, p.sl, p.tp);
                }
            }
        }
        // exits — EXACT engine conventions (SL-fill asymmetry from the original spec)
        if _trace {
            println!("TR M {}/side{} c{:.4} sl{:.4} tp{:.4} runx{:.4}", b.ts, p.side, b.c, p.sl, p.tp, p.runx);
        }
        let (sl, tp, side, c, h) = {
            let q = self.pos.as_ref().unwrap();
            (q.sl, q.tp, q.side, b.c, b.h)
        };
        if side == 1 {
            if c <= sl {
                self.close_pos("SL", sl, b.ts);
            } else if h >= tp {
                self.close_pos("TP", tp, b.ts);
            }
        } else {
            if h >= sl {
                self.close_pos("SL", sl, b.ts);
            } else if c <= tp {
                self.close_pos("TP", tp, b.ts);
            }
        }
    }

    fn close_pos(&mut self, exit_type: &str, fill: f64, ts: i64) {
        let p = match self.pos.take() {
            Some(p) => p,
            None => return,
        };
        let pct = if p.side == 1 {
            (fill / p.entry - 1.0) * 100.0
        } else {
            (1.0 - fill / p.entry) * 100.0
        };
        let r = pct / (p.slp * 100.0);
        self.trades.push(Trade {
            session: p.session.clone(),
            side: p.side,
            entry: round4(p.entry),
            sl: round4(p.sl0),
            tp: round4(p.tp),
            exit: round4(fill),
            exit_type: exit_type.to_string(),
            rr: p.rr,
            slp: p.slp,
            pct: round4(pct),
            r: round4(r),
            entry_ts: p.entry_ts,
            exit_ts: ts,
            bars_held: p.bars_held,
            trail_a: p.trail_a,
        });
    }

    #[allow(dead_code)]
    pub fn force_flat(&mut self) {
        if self.pos.is_none() {
            return;
        }
        let last = self.session_bars.last().cloned().or_else(|| self.bars.last().cloned());
        if let Some(b) = last {
            self.close_pos("MANUAL", b.c, b.ts);
        }
    }

    pub fn net(&self) -> f64 {
        self.trades.iter().map(|t| t.pct).sum()
    }

    pub fn active_session(&self) -> String {
        self.active_sd.clone().unwrap_or_default()
    }

    #[allow(dead_code)]
    pub fn session_bars_len(&self) -> usize {
        self.session_bars.len()
    }

    #[allow(dead_code)]
    pub fn atr(&self) -> f64 {
        self.atr
    }
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}
