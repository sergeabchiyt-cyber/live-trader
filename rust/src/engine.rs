//! FRVP PoC strategy core — streaming port of legacy/live/strategy.py (which is
//! itself a 1:1 port of the validated backtest engine).  Same session calendar,
//! FRVP/PoC math (incl. Python's first-max tie-break), ATR stop, milestone
//! trailing and fill conventions — evaluated on each closed 15M bar.
//!
//! v2 extensions (documented in AUDIT.md):
//!   * event emission (session/open/trail/close) for the live hub
//!   * prev-session profile + bars retained for the dashboard volume profile
//!   * `sl_touch` fill mode: unified touch-based SL/TP (exchange-realistic).
//!     `sl_touch=false` reproduces the legacy side-asymmetric conventions
//!     exactly (long: SL on close / TP on touch; short: SL on touch / TP on
//!     close) — this is the parity mode used against the Python engine.

use serde_json::{json, Value};
use std::collections::VecDeque;

pub const DAY_OPEN_HOUR: i64 = 22; // 22:00 UTC session open
pub const BINS: usize = 48;
pub const ATR_N: usize = 14;
pub const SL_MIN: f64 = 0.0005;
pub const SL_MAX: f64 = 0.005;

#[derive(Clone, Copy, Debug)]
pub struct Bar {
    pub ts: i64, // seconds UTC (bar open)
    pub o: f64,
    pub h: f64,
    pub l: f64,
    pub c: f64,
    pub v: f64,
}

#[derive(Clone, Debug)]
pub struct Trade {
    pub session: String,
    pub side: i8, // 1 long, -1 short
    pub entry: f64,
    pub sl: f64,   // initial stop (sl0 in events)
    pub tp: f64,
    pub exit: f64,
    pub exit_type: String, // TP | SL | EOS | MANUAL
    pub rr: f64,
    pub slp: f64,
    pub pct: f64,
    pub r: f64,
    pub entry_ts: i64,
    pub exit_ts: i64,
    pub bars_held: i64,
    pub trail_a: f64,
}

impl Trade {
    pub fn to_json(&self) -> Value {
        json!({
            "side": if self.side == 1 { "long" } else { "short" },
            "session": self.session,
            "entry": round4(self.entry),
            "sl": round4(self.sl),
            "tp": round4(self.tp),
            "exit": round4(self.exit),
            "exit_type": self.exit_type,
            "rr": self.rr,
            "slp": self.slp,
            "pct": round4(self.pct),
            "r": round4(self.r),
            "entry_ts": self.entry_ts,
            "exit_ts": self.exit_ts,
            "bars_held": self.bars_held,
            "trail_a": self.trail_a,
        })
    }
}

/// Events emitted while the strategy consumes bars (mirrors live/strategy.py).
#[derive(Clone, Debug)]
pub enum Event {
    Session {
        date: String,
        open: f64,
        prev_poc: Option<f64>,
        bias: Option<i8>,
        ts: i64,
    },
    Open {
        side: i8,
        session: String,
        entry: f64,
        sl: f64,
        tp: f64,
        slp: f64,
        rr: f64,
        entry_ts: i64,
        entry_hour: i64,
        session_bars_at_entry: usize,
        last_close: f64,
    },
    Trail {
        ts: i64,
        a: f64,
        sl: f64,
    },
    Close(Trade),
}

impl Event {
    pub fn to_json(&self) -> Value {
        match self {
            Event::Session { date, open, prev_poc, bias, ts } => json!({
                "type": "session", "date": date, "open": open,
                "prev_poc": prev_poc, "bias": bias, "ts": ts,
            }),
            Event::Open { side, session, entry, sl, tp, slp, rr, entry_ts, entry_hour,
                          session_bars_at_entry, last_close } => json!({
                "type": "open",
                "side": if *side == 1 { "long" } else { "short" },
                "session": session, "entry": entry, "sl": sl, "tp": tp,
                "slp": slp, "rr": rr, "entry_ts": entry_ts,
                "entry_hour": entry_hour,
                "session_bars_at_entry": session_bars_at_entry,
                "last_close": last_close,
            }),
            Event::Trail { ts, a, sl } => json!({
                "type": "trail", "ts": ts, "a": a, "sl": sl,
            }),
            Event::Close(t) => {
                let mut v = t.to_json();
                v["type"] = json!("close");
                v
            }
        }
    }
}

// ---------------------------------------------------------------------------
// civil date helpers (no chrono dependency).  Epoch 1970-01-01 == Thursday.
// Howard Hinnant's civil_from_days / days_from_civil.
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

#[allow(dead_code)] // used by tests; kept as a public date util
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Weekday (0=Mon..6=Sun — Python's date.weekday()) of the session date of ts.
pub fn session_weekday(ts: i64) -> u32 {
    // 1970-01-01 was Thursday -> weekday 3 with Mon=0
    let wd0 = (ts.div_euclid(86400) + 3).rem_euclid(7) as u32;
    if ts.rem_euclid(86400) >= DAY_OPEN_HOUR * 3600 {
        wd0
    } else {
        (wd0 + 6) % 7 // previous day
    }
}

/// "YYYY-MM-DD" of the session that owns ts (22:00 UTC roll).
pub fn session_date_key(ts: i64) -> String {
    let days = ts.div_euclid(86400)
        - if ts.rem_euclid(86400) < DAY_OPEN_HOUR * 3600 { 1 } else { 0 };
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Session start ts (midnight of the session date + 22h) — Python parity:
/// computed from the session DATE, so a late first bar (data gap) still maps
/// to the true session open.
pub fn session_start_ts(ts: i64) -> i64 {
    let sd = ts.div_euclid(86400)
        - if ts.rem_euclid(86400) < DAY_OPEN_HOUR * 3600 { 1 } else { 0 };
    sd * 86400 + DAY_OPEN_HOUR * 3600
}

pub fn iso_utc(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let tod = ts.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, m, d, tod / 3600, (tod % 3600) / 60, tod % 60
    )
}

pub fn now_ts() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn now_ts_f64() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub hi: f64,
    pub lo: f64,
    pub w: f64,
    pub prof: Vec<f64>,
    pub poc: f64,
    pub start: i64,
}

impl Profile {
    pub fn to_json(&self) -> Value {
        json!({ "hi": self.hi, "lo": self.lo, "w": self.w,
                "prof": self.prof, "poc": self.poc, "start": self.start })
    }
}

/// FRVP of one session: 4H buckets of 15M bars, volume spread uniformly over
/// each bucket's range -> BINS bins -> PoC = centre of max bin.
pub fn compute_poc(session: &[Bar], start_ts: i64) -> Option<Profile> {
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
    let mut buckets: Vec<Vec<&Bar>> = Vec::new();
    for b in session {
        let idx = ((b.ts - start_ts) / 14400).max(0) as usize;
        while buckets.len() <= idx {
            buckets.push(Vec::new());
        }
        buckets[idx].push(b);
    }
    for b4 in buckets {
        if b4.is_empty() {
            continue;
        }
        let vh = b4.iter().map(|b| b.h).fold(f64::MIN, f64::max);
        let vl = b4.iter().map(|b| b.l).fold(f64::MAX, f64::min);
        let vv: f64 = b4.iter().map(|b| b.v).sum();
        let a = (((vl - lo) / w) as isize).clamp(0, BINS as isize - 1) as usize;
        let z = (((vh.min(hi) - lo) / w) as isize).clamp(a as isize, BINS as isize - 1) as usize;
        let add = vv / (z - a + 1) as f64;
        for k in a..=z {
            prof[k] += add;
        }
    }
    // Python parity: first index of the max value (max_by would pick the last
    // on ties and shift the PoC).
    let mut maxidx = 0usize;
    let mut maxv = prof[0];
    for (i, &v) in prof.iter().enumerate() {
        if v > maxv {
            maxv = v;
            maxidx = i;
        }
    }
    let poc = lo + (maxidx as f64 + 0.5) * w;
    Some(Profile { hi, lo, w, prof, poc, start: start_ts })
}

#[derive(Clone)]
pub struct StrategyCfg {
    pub rr: f64,
    pub skip_hour0: bool,
    pub trail_on: bool,
    pub ladder: Vec<(f64, f64)>, // (progress toward TP, R multiple), sorted
    pub same_bar_exits: bool,
    /// true (production): SL and TP both trigger on touch — matches how a real
    /// exchange stop/TP order fills. false: legacy engine conventions.
    pub sl_touch: bool,
}

impl Default for StrategyCfg {
    fn default() -> Self {
        StrategyCfg {
            rr: 2.0,
            skip_hour0: true,
            trail_on: true,
            ladder: vec![(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)],
            same_bar_exits: true,
            sl_touch: true,
        }
    }
}

pub struct Strategy {
    pub cfg: StrategyCfg,
    pub bars: Vec<Bar>,
    atr: Option<f64>,
    prev_c: Option<f64>,
    pub prev_poc: Option<f64>,
    pub prev_prof: Option<Profile>,
    pub prev_bars: Vec<Bar>, // retained for the dashboard (TV-style VP)
    active_sd: Option<String>,
    active_start: i64,
    session_bars: Vec<Bar>,
    pub open0: f64,
    pub bias: Option<i8>,
    session_traded: bool,
    eos_done: bool,
    last_bar_ts: i64,
    pos: Option<Pos>,
    pub trades: Vec<Trade>,
    pub recent_events: VecDeque<Value>, // capped ring, exposed in state()
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
        Strategy {
            cfg,
            bars: Vec::new(),
            atr: None,
            prev_c: None,
            prev_poc: None,
            prev_prof: None,
            prev_bars: Vec::new(),
            active_sd: None,
            active_start: 0,
            session_bars: Vec::new(),
            open0: 0.0,
            bias: None,
            session_traded: false,
            eos_done: false,
            last_bar_ts: -1,
            pos: None,
            trades: Vec::new(),
            recent_events: VecDeque::new(),
        }
    }

    fn feed_atr(&mut self, b: &Bar) {
        let tr = match self.prev_c {
            None => b.h - b.l,
            Some(pc) => (b.h - b.l)
                .max((b.h - pc).abs())
                .max((b.l - pc).abs()),
        };
        self.atr = Some(match self.atr {
            None => tr,
            Some(a) => (a * (ATR_N as f64 - 1.0) + tr) / ATR_N as f64,
        });
        self.prev_c = Some(b.c);
    }

    fn push_event(&mut self, ev: &Event) {
        self.recent_events.push_back(ev.to_json());
        while self.recent_events.len() > 400 {
            self.recent_events.pop_front();
        }
        // keep a stable tail window like the Python engine (400 -> 300)
        if self.recent_events.len() == 400 {
            for _ in 0..100 {
                self.recent_events.pop_front();
            }
        }
    }

    /// Fast-forward a list of closed bars (bootstrap / replay warm).
    /// Returns the events emitted (caller decides whether to publish them;
    /// the recent_events ring is still maintained for state()).
    pub fn feed_history(&mut self, bars: &[Bar]) -> Vec<Event> {
        let mut out = Vec::new();
        for b in bars {
            out.extend(self.on_bar(b));
        }
        out
    }

    /// Feed one closed 15M bar; returns emitted events.
    pub fn on_bar(&mut self, b: &Bar) -> Vec<Event> {
        let mut evs: Vec<Event> = Vec::new();
        if b.ts <= self.last_bar_ts {
            return evs; // duplicate / stale
        }
        self.bars.push(*b);
        self.last_bar_ts = b.ts;
        self.feed_atr(b);

        let wd = session_weekday(b.ts);
        let sd = session_date_key(b.ts);
        let in_day = matches!(wd, 6 | 0 | 1 | 2 | 3); // Sun..Thu (Mon=0)

        if !in_day {
            // weekend / off-calendar bars: no session, but close stragglers at EOS
            if self.pos.is_some() && !self.eos_done {
                let fill = self.session_bars.last().map(|x| x.c).unwrap_or(b.c);
                if let Some(ev) = self.close_pos("EOS", fill, b.ts) {
                    evs.push(ev);
                }
                self.eos_done = true;
            }
            return evs;
        }

        if self.active_sd.is_none() {
            // first usable session: open it (bootstrap, no PD PoC yet)
            self.open_session(&sd, b);
        } else if self.active_sd.as_deref() != Some(sd.as_str()) {
            // session boundary: finalise previous session, then open the new one
            if self.pos.is_some() {
                let fill = self.session_bars.last().map(|x| x.c).unwrap_or(b.o);
                if let Some(ev) = self.close_pos("EOS", fill, b.ts) {
                    evs.push(ev);
                }
            }
            if !self.session_bars.is_empty() {
                let prof = compute_poc(&self.session_bars, self.active_start);
                self.prev_prof = prof.clone();
                self.prev_poc = prof.as_ref().map(|p| p.poc);
                self.prev_bars = self.session_bars.clone();
            } else {
                self.prev_prof = None;
                self.prev_poc = None;
                self.prev_bars.clear();
            }
            self.open_session(&sd, b);
            self.bias = self.prev_poc.map(|p| if self.open0 >= p { 1 } else { -1 });
            let ev = Event::Session {
                date: sd.clone(),
                open: self.open0,
                prev_poc: self.prev_poc,
                bias: self.bias,
                ts: self.active_start,
            };
            self.push_event(&ev);
            evs.push(ev);
        }

        self.session_bars.push(*b);
        self.eos_done = false;

        // manage open position on this closed bar
        if self.pos.is_some() {
            evs.extend(self.manage(b));
        }
        // entry trigger (only if no position and session not yet traded)
        if self.pos.is_none()
            && !self.session_traded
            && self.prev_poc.is_some()
            && self.bias.is_some()
        {
            let opened = self.maybe_enter(b, &mut evs);
            // ENGINE PARITY: the backtest scans exits from the entry bar itself.
            if opened && self.cfg.same_bar_exits && self.pos.is_some() {
                evs.extend(self.manage(b));
            }
        }
        evs
    }

    fn open_session(&mut self, sd: &str, b: &Bar) {
        self.active_sd = Some(sd.to_string());
        self.active_start = session_start_ts(b.ts);
        self.session_bars.clear();
        self.open0 = b.o;
        self.bias = None;
        self.session_traded = false;
        self.eos_done = false;
    }

    fn maybe_enter(&mut self, b: &Bar, evs: &mut Vec<Event>) -> bool {
        if self.cfg.skip_hour0 {
            let hh = b.ts.rem_euclid(86400) / 3600;
            if hh == 0 {
                return false; // 00:00-01:00 excluded window
            }
        }
        let poc = match self.prev_poc {
            Some(p) => p,
            None => return false,
        };
        match self.bias {
            Some(1) if b.l <= poc => self.enter(1, b, poc, evs),
            Some(-1) if b.h >= poc => self.enter(-1, b, poc, evs),
            _ => false,
        }
    }

    fn enter(&mut self, side: i8, b: &Bar, entry_price: f64, evs: &mut Vec<Event>) -> bool {
        self.session_traded = true;
        let slp = (self.atr.unwrap_or(0.0) / entry_price).clamp(SL_MIN, SL_MAX);
        let (sl0, tp) = if side == 1 {
            (entry_price * (1.0 - slp), entry_price * (1.0 + slp * self.cfg.rr))
        } else {
            (entry_price * (1.0 + slp), entry_price * (1.0 - slp * self.cfg.rr))
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
        let ev = Event::Open {
            side,
            session: self.active_sd.clone().unwrap_or_default(),
            entry: entry_price,
            sl: sl0,
            tp,
            slp,
            rr: self.cfg.rr,
            entry_ts: b.ts,
            entry_hour: b.ts.rem_euclid(86400) / 3600,
            session_bars_at_entry: self.session_bars.len(),
            last_close: b.c,
        };
        self.push_event(&ev);
        evs.push(ev);
        true
    }

    fn manage(&mut self, b: &Bar) -> Vec<Event> {
        let mut evs = Vec::new();
        if self.pos.is_none() {
            return evs;
        }
        // trail event emitted THIS bar, if the ratchet moved (Python order:
        // runx -> trail ratchet + event -> exit checks)
        let mut trail_ev: Option<Event> = None;
        {
            let p = self.pos.as_mut().unwrap();
            p.bars_held += 1;
            if p.side == 1 {
                p.runx = p.runx.max(b.h);
            } else {
                p.runx = p.runx.min(b.l);
            }
            if self.cfg.trail_on {
                let prev_a = p.trail_a;
                let mut best_a = p.trail_a;
                for &(prog, ar) in &self.cfg.ladder {
                    let ref_px = p.entry + (p.tp - p.entry) * prog;
                    let hit = if p.side == 1 { p.runx >= ref_px } else { p.runx <= ref_px };
                    if hit && ar > best_a {
                        best_a = ar;
                    }
                }
                if best_a != prev_a {
                    p.trail_a = best_a;
                    let cand = if p.side == 1 {
                        p.entry + best_a * p.slp * p.entry
                    } else {
                        p.entry - best_a * p.slp * p.entry
                    };
                    p.sl = if p.side == 1 { p.sl.max(cand) } else { p.sl.min(cand) };
                    trail_ev = Some(Event::Trail { ts: b.ts, a: best_a, sl: p.sl });
                }
            }
        }
        if let Some(ev) = trail_ev {
            self.push_event(&ev);
            evs.push(ev);
        }
        // exits — SL checked first (conservative when both levels are touched)
        let (side, sl, tp) = {
            let p = self.pos.as_ref().unwrap();
            (p.side, p.sl, p.tp)
        };
        let (sl_hit, tp_hit) = if self.cfg.sl_touch {
            if side == 1 {
                (b.l <= sl, b.h >= tp)
            } else {
                (b.h >= sl, b.l <= tp)
            }
        } else {
            // legacy engine conventions (side-asymmetric, see AUDIT.md)
            if side == 1 {
                (b.c <= sl, b.h >= tp)
            } else {
                (b.h >= sl, b.c <= tp)
            }
        };
        if sl_hit {
            if let Some(ev) = self.close_pos("SL", sl, b.ts) {
                evs.push(ev);
            }
        } else if tp_hit {
            if let Some(ev) = self.close_pos("TP", tp, b.ts) {
                evs.push(ev);
            }
        }
        evs
    }

    fn close_pos(&mut self, exit_type: &str, fill: f64, ts: i64) -> Option<Event> {
        let p = match self.pos.take() {
            Some(p) => p,
            None => return None,
        };
        let pct = if p.side == 1 {
            (fill / p.entry - 1.0) * 100.0
        } else {
            (1.0 - fill / p.entry) * 100.0
        };
        let r = pct / (p.slp * 100.0);
        let trade = Trade {
            session: p.session.clone(),
            side: p.side,
            entry: p.entry,
            sl: p.sl0,
            tp: p.tp,
            exit: fill,
            exit_type: exit_type.to_string(),
            rr: p.rr,
            slp: p.slp,
            pct,
            r,
            entry_ts: p.entry_ts,
            exit_ts: ts,
            bars_held: p.bars_held,
            trail_a: p.trail_a,
        };
        self.trades.push(trade.clone());
        let ev = Event::Close(trade);
        self.push_event(&ev);
        Some(ev)
    }

    #[allow(dead_code)]
    pub fn force_flat(&mut self) -> Option<Event> {
        self.pos.as_ref()?;
        let last = self
            .session_bars
            .last()
            .cloned()
            .or_else(|| self.bars.last().cloned())?;
        self.close_pos("MANUAL", last.c, last.ts)
    }

    pub fn net(&self) -> f64 {
        self.trades.iter().map(|t| t.pct).sum()
    }

    pub fn active_session(&self) -> Option<&str> {
        self.active_sd.as_deref()
    }

    #[allow(dead_code)]
    pub fn atr(&self) -> Option<f64> {
        self.atr
    }

    fn unrl(&self) -> Option<f64> {
        let p = self.pos.as_ref()?;
        let lb = self.session_bars.last().or_else(|| self.bars.last())?;
        let px = lb.c;
        Some(if p.side == 1 {
            (px / p.entry - 1.0) * 100.0
        } else {
            (1.0 - px / p.entry) * 100.0
        })
    }

    /// Full state snapshot (field names match legacy/live/strategy.py state()).
    pub fn state(&self) -> Value {
        let lb = self
            .session_bars
            .last()
            .or_else(|| self.bars.last())
            .copied();
        let phase = if self.pos.is_some() {
            "in_position"
        } else if self.session_traded {
            "session_done"
        } else if self.prev_poc.is_none() {
            "bootstrap"
        } else {
            "waiting_trigger"
        };
        json!({
            "time_utc": iso_utc(now_ts()),
            "session": self.active_sd,
            "prev_poc": self.prev_poc,
            "bias": self.bias,
            "open0": self.open0,
            "atr": self.atr,
            "phase": phase,
            "session_traded": self.session_traded,
            "position": self.pos.as_ref().map(|p| json!({
                "side": if p.side == 1 { "long" } else { "short" },
                "entry": p.entry, "sl": p.sl, "sl0": p.sl0, "tp": p.tp,
                "slp": p.slp, "rr": p.rr, "runx": p.runx, "trail_a": p.trail_a,
                "unreal_pct": self.unrl(),
            })),
            "last_bar": lb.map(|b| json!({
                "ts": b.ts, "o": b.o, "h": b.h, "l": b.l, "c": b.c, "v": b.v,
            })),
            "session_bars": self.session_bars.len(),
            "total_bars": self.bars.len(),
            "prev_profile": self.prev_prof.as_ref().map(|p| p.to_json()),
            "prev_session_bars": self.prev_bars.iter()
                .map(|b| json!({"ts": b.ts, "o": b.o, "h": b.h, "l": b.l, "c": b.c, "v": b.v}))
                .collect::<Vec<_>>(),
            "active_session_bars": self.session_bars.iter()
                .map(|b| json!({"ts": b.ts, "o": b.o, "h": b.h, "l": b.l, "c": b.c, "v": b.v}))
                .collect::<Vec<_>>(),
            "recent_events": self.recent_events.iter().cloned().collect::<Vec<_>>(),
        })
    }
}

fn round4(x: f64) -> f64 {
    // Python round(x, 4) parity: {:.4} rounds the exact binary value with
    // ties-to-even, matching CPython's round() on virtually every input.
    format!("{:.4}", x).parse().unwrap_or(x)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn bar(ts: i64, o: f64, h: f64, l: f64, c: f64, v: f64) -> Bar {
        Bar { ts, o, h, l, c, v }
    }

    #[test]
    fn session_keys_roll_at_22utc() {
        // Wed 2026-09-02 21:59 UTC belongs to Tue's session
        let t = days_from_civil(2026, 9, 2) * 86400 + 21 * 3600 + 59 * 60;
        assert_eq!(session_date_key(t), "2026-09-01");
        // Wed 2026-09-02 22:00 UTC OPENS the Wed (2026-09-02) session: the
        // session labeled D always opens at 22:00 UTC on day D itself
        let t2 = days_from_civil(2026, 9, 2) * 86400 + 22 * 3600;
        assert_eq!(session_date_key(t2), "2026-09-02");
        // Thu 21:59 still belongs to the Wed session
        let t3 = days_from_civil(2026, 9, 3) * 86400 + 21 * 3600 + 59 * 60;
        assert_eq!(session_date_key(t3), "2026-09-02");
        // Sun 22:00 opens the Sunday session (weekday Sun=6 in Mon=0 numbering)
        let sun = days_from_civil(2026, 9, 6) * 86400 + 22 * 3600; // Sun 22:00
        assert_eq!(session_weekday(sun), 6);
        assert_eq!(session_date_key(sun), "2026-09-06");
    }

    #[test]
    fn session_start_uses_session_date_midnight() {
        // first in-calendar bar arrives late (data gap): Mon 01:00 belongs to
        // the Sunday session which opened Sun 22:00
        let mon_0100 = days_from_civil(2026, 9, 7) * 86400 + 3600;
        assert_eq!(session_date_key(mon_0100), "2026-09-06");
        assert_eq!(
            session_start_ts(mon_0100),
            days_from_civil(2026, 9, 6) * 86400 + 22 * 3600
        );
    }

    #[test]
    fn poc_first_max_tiebreak_and_shape() {
        // 8 bars, one 4h bucket, range 100..108 -> 8 bins of width 1; all
        // volume in one bar spanning everything -> uniform profile, PoC centre
        let t0 = days_from_civil(2026, 9, 6) * 86400 + 22 * 3600;
        let bars: Vec<Bar> = (0..8)
            .map(|i| bar(t0 + i * 900, 104.0, 107.0, 101.0, 104.0, 10.0))
            .collect();
        let p = compute_poc(&bars, t0).unwrap();
        assert!((p.hi - 107.0).abs() < 1e-9 && (p.lo - 101.0).abs() < 1e-9);
        // uniform prof over bins covered (101..107 -> bins 0..6 fully), first max = 0
        assert!((p.poc - (101.0 + 0.5 * (107.0 - 101.0) / 48.0)).abs() < 1e-9);
    }

    #[test]
    fn atr_wilder_first_bar() {
        let t0 = days_from_civil(2026, 9, 6) * 86400 + 22 * 3600;
        let b = bar(t0, 100.0, 103.0, 99.0, 102.0, 1.0);
        let mut s = Strategy::new(StrategyCfg::default());
        s.on_bar(&b);
        assert!((s.atr().unwrap() - 4.0).abs() < 1e-9); // h-l = 4
    }
}
