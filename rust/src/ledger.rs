//! Fee-aware paper ledger — the honest accounting layer (AUDIT.md fix #1/#3).
//!
//! Legacy behaviour (Python PaperBroker) compounded each trade's % return on
//! the full paper equity and charged 0 fees, while the exchange mirror traded
//! a fixed POSITION_USD notional: the ledger and the orders disagreed ~100x
//! and ignored the strategy's single biggest cost (Bybit perp fees).
//!
//! v2 ledger:
//!   * sizing: notional = min(equity * RISK_PCT/100 / slp, equity * LEVERAGE_CAP)
//!     (LEGACY_SIZING=1 reverts to the fixed POSITION_USD notional)
//!   * fees: entry is a resting LIMIT -> maker; every exit (STOP_MARKET SL,
//!     TAKE_PROFIT_MARKET TP, market EOS) -> taker. SL_SLIP_PCT adds adverse
//!     slippage on stop fills.
//!   * the same sizing feeds the exchange mirror, so ledger and orders agree.

use serde_json::{json, Value};
use crate::engine::Trade;

#[derive(Clone, Copy, Debug)]
pub struct FeeModel {
    pub maker_pct: f64, // percent of notional, e.g. 0.02
    pub taker_pct: f64, // e.g. 0.055
    pub slip_pct: f64,  // adverse slippage on SL/MANUAL fills, percent
}

#[derive(Clone, Copy, Debug)]
pub struct Sizing {
    pub qty: f64,
    pub notional_usd: f64,
    pub risk_usd: f64, // actual $ at risk at entry (notional * slp)
}

#[derive(Clone)]
pub struct SizingCfg {
    pub legacy: bool,
    pub position_usd: f64,
    pub risk_pct: f64,
    pub leverage_cap: f64,
}

pub struct Ledger {
    pub start: f64,
    pub equity: f64,
    pub n_trades: usize,
    pub curve: Vec<(i64, f64)>, // (ts, equity)
    pub fees_paid: f64,
    sizing: SizingCfg,
    fee: FeeModel,
    open_sizing: Option<Sizing>, // captured at the live open event
}

impl Ledger {
    pub fn new(start: f64, sizing: SizingCfg, fee: FeeModel) -> Self {
        Ledger {
            start,
            equity: start,
            n_trades: 0,
            curve: vec![(crate::engine::now_ts(), start)],
            fees_paid: 0.0,
            sizing,
            fee,
            open_sizing: None,
        }
    }

    /// Compute (and remember) the position sizing for an entry.
    pub fn on_open(&mut self, entry: f64, slp: f64) -> Sizing {
        let notional = if self.sizing.legacy {
            self.sizing.position_usd
        } else {
            let risk_usd = self.equity * self.sizing.risk_pct / 100.0;
            let cap = self.equity * self.sizing.leverage_cap;
            (risk_usd / slp.max(1e-9)).min(cap).max(0.0)
        };
        let qty = if entry > 0.0 { notional / entry } else { 0.0 };
        let risk_usd = notional * slp;
        let s = Sizing { qty, notional_usd: notional, risk_usd };
        self.open_sizing = Some(s);
        s
    }

    /// Apply a closed trade: compute fees/PnL in USDT, update equity, and
    /// return the enriched close-event payload (adds qty/notional/fees/net R).
    pub fn on_close(&mut self, t: &Trade, ts: i64) -> Value {
        let s = self.open_sizing.take().unwrap_or(Sizing {
            qty: 0.0,
            notional_usd: 0.0,
            risk_usd: 0.0,
        });
        let entry_notional = s.qty * t.entry;
        let exit_notional = s.qty * t.exit;
        let entry_fee = entry_notional * self.fee.maker_pct / 100.0;
        let exit_fee = exit_notional * self.fee.taker_pct / 100.0;
        let market_exit = matches!(t.exit_type.as_str(), "SL" | "EOS" | "MANUAL");
        let slip = if market_exit {
            exit_notional * self.fee.slip_pct / 100.0
        } else {
            0.0
        };
        let dir = t.side as f64;
        let gross_usd = s.qty * (t.exit - t.entry) * dir;
        let pnl_usd = gross_usd - entry_fee - exit_fee - slip;
        let fee_usd = entry_fee + exit_fee;
        self.equity += pnl_usd;
        self.fees_paid += fee_usd + slip;
        self.n_trades += 1;
        self.curve.push((ts, (self.equity * 100.0).round() / 100.0));
        if self.curve.len() > 400 {
            let drop = self.curve.len() - 400;
            self.curve.drain(0..drop);
        }
        let net_r = if s.risk_usd > 0.0 { pnl_usd / s.risk_usd } else { 0.0 };
        json!({
            "side": if t.side == 1 { "long" } else { "short" },
            "session": t.session,
            "entry": t.entry, "sl": t.sl, "tp": t.tp, "exit": t.exit,
            "exit_type": t.exit_type, "rr": t.rr, "slp": t.slp,
            "pct": t.pct, "r": t.r,
            "entry_ts": t.entry_ts, "exit_ts": t.exit_ts,
            "bars_held": t.bars_held, "trail_a": t.trail_a,
            "qty": s.qty,
            "notional_usd": s.notional_usd,
            "risk_usd": s.risk_usd,
            "entry_fee_usd": round6(entry_fee),
            "exit_fee_usd": round6(exit_fee),
            "fee_usd": round6(fee_usd),
            "slip_usd": round6(slip),
            "pnl_usd": round6(pnl_usd),
            "net_r": (net_r * 10000.0).round() / 10000.0,
            "equity": (self.equity * 100.0).round() / 100.0,
        })
    }

    /// Replay a historical trade (bootstrap warm-up): recompute the sizing as
    /// of that trade, then close it. Keeps ledger + replay deterministic.
    pub fn apply_warm(&mut self, t: &Trade) -> Value {
        self.on_open(t.entry, t.slp);
        self.on_close(t, t.exit_ts)
    }

    pub fn balance(&self, broker: &str, mirror_armed: bool) -> Value {
        json!({
            "broker": broker,
            "equity": self.equity,
            "start": self.start,
            "n_trades": self.n_trades,
            "unrealized": Value::Null,
            "mirror_armed": mirror_armed,
            "fees_paid": (self.fees_paid * 1e6).round() / 1e6,
        })
    }

    pub fn equity_curve_json(&self) -> Value {
        self.curve
            .iter()
            .map(|(ts, eq)| json!({"ts": ts, "eq": eq}))
            .collect::<Vec<_>>()
            .into()
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}
