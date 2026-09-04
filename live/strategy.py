"""FRVP PoC strategy core — a streaming, bar-close port of the validated backtest
engine (engine.py). Produces the same session/FRVP/PoC math, trigger, ATR stop,
milestone trailing and fill conventions, evaluated on each closed 15M bar.
"""
from datetime import datetime, timezone, timedelta
from typing import List, Optional


class Bar:
    __slots__ = ("ts", "o", "h", "l", "c", "v", "t")
    def __init__(self, ts, o, h, l, c, v, t=0.0):
        self.ts = int(ts)          # seconds UTC (bar open)
        self.o, self.h, self.l, self.c = float(o), float(h), float(l), float(c)
        self.v = float(v)          # tick-volume proxy / volume
        self.t = float(t)


def session_date(ts_s, day_open_hour=22):
    dt = datetime.fromtimestamp(ts_s, timezone.utc)
    d = dt.date() if dt.hour >= day_open_hour else (dt - timedelta(days=1)).date()
    return d


def compute_poc(session_bars: List[Bar], start_ts: int, bins=48):
    """FRVP of one session: 4H buckets of 15M bars, tick-volume spread uniformly
       over each bucket's range -> bins -> PoC = centre of max bin."""
    if not session_bars:
        return None
    hi = max(b.h for b in session_bars)
    lo = min(b.l for b in session_bars)
    if hi <= lo or len(session_bars) < 8:
        return None
    w = (hi - lo) / bins
    prof = [0.0] * bins
    fourh = {}
    for b in session_bars:
        fourh.setdefault((b.ts - start_ts) // 14400, []).append(b)
    for b4 in fourh.values():
        vh = max(b.h for b in b4)
        vl = min(b.l for b in b4)
        vv = sum(b.v for b in b4)
        a = max(0, min(bins - 1, int((vl - lo) / w)))
        z = max(a, min(bins - 1, int((min(vh, hi) - lo) / w)))
        add = vv / (z - a + 1)
        for k in range(a, z + 1):
            prof[k] += add
    poc = lo + (prof.index(max(prof)) + 0.5) * w
    return dict(hi=hi, lo=lo, w=w, prof=prof, poc=poc, start=start_ts)


class Position:
    def __init__(self, side, entry, sl0, tp, slp, session, entry_ts, rr):
        self.side = side
        self.entry = entry
        self.sl0 = sl0
        self.sl = sl0
        self.tp = tp
        self.slp = slp
        self.session = session
        self.entry_ts = entry_ts
        self.rr = rr
        self.runx = entry
        self.trail_a = 0.0
        self.closed = None
        self.exit_type = None
        self.exit_price = None
        self.exit_ts = None
        self.bars_held = 0

    def pnl_pct(self):
        if self.closed is None:
            return None
        if self.side == "long":
            return (self.exit_price / self.entry - 1) * 100
        return (1 - self.exit_price / self.entry) * 100

    def pnl_r(self):
        p = self.pnl_pct()
        return None if p is None else p / (self.slp * 100)


class Strategy:
    def __init__(self, cfg, emit=None, log_trades=None):
        self.cfg = cfg
        self.emit = emit or (lambda ev: None)
        self.log_trades = log_trades
        self.bars: List[Bar] = []
        self.atr = None
        self.prev_c = None
        self.prev_poc = None
        self.prev_prof = None
        self.active_sd = None
        self.active_start = None
        self.session_bars: List[Bar] = []
        self.open0 = None
        self.bias = None
        self.session_traded = False
        self.pos: Optional[Position] = None
        self.events = []
        self.trades = []                     # completed trade dicts
        self.last_bar_ts = None
        self._eos_done = False

    # ---------------- ATR (Wilder, engine-identical) -----------
    def _feed_atr(self, b: Bar):
        tr = b.h - b.l if self.prev_c is None else max(b.h - b.l, abs(b.h - self.prev_c),
                                                       abs(b.l - self.prev_c))
        if self.atr is None:
            self.atr = tr
        else:
            self.atr = (self.atr * (self.cfg.atr_n - 1) + tr) / self.cfg.atr_n
        self.prev_c = b.c

    # ---------------- intake ----------------
    def feed_history(self, bars: List[Bar], quiet=False):
        """Fast-forward a list of closed bars (bootstrap / replay warm)."""
        saved = self.emit
        if quiet:
            self.emit = lambda ev: None
        for b in bars:
            self.on_bar(b)
        self.emit = saved

    def on_bar(self, b: Bar):
        evs = []
        if self.last_bar_ts is not None and b.ts <= self.last_bar_ts:
            return []
        self.bars.append(b)
        self.last_bar_ts = b.ts
        self._feed_atr(b)

        sd = session_date(b.ts, self.cfg.day_open_hour)
        in_day = sd.weekday() in self.cfg.session_days

        if not in_day:
            # weekend / off-calendar bars: no session, but close stragglers at EOS
            if self.pos is not None and not self._eos_done:
                evs += self._close_pos("EOS", self.session_bars[-1].c if self.session_bars else b.c,
                                       b.ts)
                self._eos_done = True
            return evs

        if self.active_sd is None:
            # first usable session: open it (no PD PoC yet -> bootstrap)
            self.active_sd = sd
            self.active_start = int(datetime.combine(
                sd, datetime.min.time(), tzinfo=timezone.utc).timestamp()) \
                + self.cfg.day_open_hour * 3600
            self.session_bars = []
            self.open0 = b.o
            self.bias = None
            self.session_traded = False
            self._eos_done = False

        elif sd != self.active_sd:
            # ---- session boundary: finalise previous session, then open new ----
            if self.pos is not None:
                evs += self._close_pos("EOS", self.session_bars[-1].c if self.session_bars else b.o, b.ts)
            if self.session_bars:
                prof = compute_poc(self.session_bars, self.active_start, self.cfg.bins)
                self.prev_prof = prof
                self.prev_poc = prof["poc"] if prof else None
            else:
                self.prev_prof = None
                self.prev_poc = None
            self.active_sd = sd
            self.active_start = int(datetime.combine(
                sd, datetime.min.time(), tzinfo=timezone.utc).timestamp()) \
                + self.cfg.day_open_hour * 3600
            self.session_bars = []
            self.open0 = b.o
            self.bias = (1 if self.open0 >= self.prev_poc else -1) if self.prev_poc is not None else None
            self.session_traded = False
            self._eos_done = False
            evs.append(dict(type="session", date=str(sd), open=self.open0,
                            prev_poc=self.prev_poc, bias=self.bias, ts=self.active_start))

        self.session_bars.append(b)
        self._eos_done = False

        # manage open position on this closed bar
        if self.pos is not None:
            evs += self._manage(b)
        # entry trigger (only if no position and session not yet traded)
        if (self.pos is None and not self.session_traded and self.prev_poc is not None
                and self.bias is not None):
            evs += self._maybe_enter(b)
            # ENGINE PARITY: the backtest scans exits from the entry bar itself
            # (entry is assumed filled at the PoC touch within that bar), so the
            # remainder of the same bar can reach SL/TP or trigger the trail.
            if self.cfg.same_bar_exits and self.pos is not None and evs and evs[-1].get("type") == "open":
                evs += self._manage(b)
        return evs

    # ---------------- entry ----------------
    def _maybe_enter(self, b: Bar):
        if self.cfg.skip_hour0:
            hh = datetime.fromtimestamp(b.ts, timezone.utc).hour
            if hh == 0:
                return []                      # 00:00-01:00 excluded window
        poc = self.prev_poc
        if self.bias > 0:
            if b.l <= poc:
                return self._enter("long", b, poc)
        else:
            if b.h >= poc:
                return self._enter("short", b, poc)
        return []

    def _enter(self, side, b: Bar, entry_price):
        self.session_traded = True
        slp = min(max(self.atr / entry_price, self.cfg.sl_min), self.cfg.sl_max)
        sl0 = entry_price * (1 - slp) if side == "long" else entry_price * (1 + slp)
        tp = entry_price * (1 + slp * self.cfg.rr) if side == "long" else \
             entry_price * (1 - slp * self.cfg.rr)
        self.pos = Position(side, entry_price, sl0, tp, slp, str(self.active_sd),
                            b.ts, self.cfg.rr)
        ev = dict(type="open", side=side, session=str(self.active_sd),
                  entry=entry_price, sl=sl0, tp=tp, slp=slp, rr=self.cfg.rr,
                  entry_ts=b.ts, entry_hour=datetime.fromtimestamp(b.ts, timezone.utc).hour,
                  session_bars_at_entry=len(self.session_bars),
                  last_close=b.c)
        self._push(ev)
        return [ev]

    # ---------------- management ----------------
    def _manage(self, b: Bar):
        p = self.pos
        if p is None:
            return []
        p.bars_held += 1
        if p.side == "long":
            p.runx = max(p.runx, b.h)
        else:
            p.runx = min(p.runx, b.l)
        # milestone trailing (ratchet toward profit only)
        if self.cfg.trail_on:
            best_a = p.trail_a
            for prog, aR in self.cfg.trail_ladder:
                ref = p.entry + (p.tp - p.entry) * prog
                if (p.side == "long" and p.runx >= ref) or (p.side == "short" and p.runx <= ref):
                    best_a = max(best_a, aR)
            if best_a != p.trail_a:
                p.trail_a = best_a
                cand = (p.entry + best_a * p.slp * p.entry if p.side == "long"
                        else p.entry - best_a * p.slp * p.entry)
                p.sl = max(p.sl, cand) if p.side == "long" else min(p.sl, cand)
                self._push(dict(type="trail", ts=b.ts, a=best_a, sl=p.sl))
        # exits — EXACT engine conventions (engine.py, from the original spec):
        #   long : SL on close <= sl, TP on high >= tp
        #   short: SL on high  >= sl, TP on close <= tp
        # (mirrors the validated backtest, incl. its SL-fills-first asymmetry)
        if p.side == "long":
            if b.c <= p.sl:
                return self._close_pos("SL", p.sl, b.ts)
            if b.h >= p.tp:
                return self._close_pos("TP", p.tp, b.ts)
        else:
            if b.h >= p.sl:
                return self._close_pos("SL", p.sl, b.ts)
            if b.c <= p.tp:
                return self._close_pos("TP", p.tp, b.ts)
        return []

    def _close_pos(self, exit_type, fill, ts):
        p = self.pos
        if p is None:
            return []
        p.closed = True
        p.exit_type = exit_type
        p.exit_price = fill
        p.exit_ts = ts
        trade = dict(side=p.side, session=p.session, entry=round(p.entry, 4),
                     sl=round(p.sl0, 4), tp=round(p.tp, 4), exit=round(fill, 4),
                     exit_type=exit_type, rr=p.rr, slp=round(p.slp, 6),
                     pct=round(p.pnl_pct(), 4), r=round(p.pnl_r(), 4),
                     entry_ts=p.entry_ts, exit_ts=ts,
                     bars_held=p.bars_held, trail_a=p.trail_a)
        self.trades.append(trade)
        if self.log_trades:
            self.log_trades(trade)
        self.pos = None
        ev = dict(type="close", **trade)
        self._push(ev)
        return [ev]

    def force_flat(self):
        if self.pos is None:
            return []
        last = self.session_bars[-1] if self.session_bars else self.bars[-1]
        return self._close_pos("MANUAL", last.c, last.ts)

    # ---------------- events/state ----------------
    def _push(self, ev):
        self.events.append(ev)
        if len(self.events) > 400:
            self.events = self.events[-300:]
        self.emit(ev)

    def state(self):
        lb = self.session_bars[-1] if self.session_bars else (self.bars[-1] if self.bars else None)
        if self.pos is not None:
            phase = "in_position"
        elif self.session_traded:
            phase = "session_done"
        elif self.prev_poc is None:
            phase = "bootstrap"
        else:
            phase = "waiting_trigger"
        return dict(
            time_utc=datetime.now(timezone.utc).isoformat(timespec="seconds"),
            session=str(self.active_sd) if self.active_sd else None,
            prev_poc=self.prev_poc,
            bias=self.bias,
            open0=self.open0,
            atr=self.atr,
            phase=phase,
            session_traded=self.session_traded,
            position=(dict(side=self.pos.side, entry=self.pos.entry, sl=self.pos.sl,
                           sl0=self.pos.sl0, tp=self.pos.tp, slp=self.pos.slp,
                           rr=self.pos.rr, runx=self.pos.runx, trail_a=self.pos.trail_a,
                           unreal_pct=self._unrl()) if self.pos else None),
            last_bar=dict(ts=lb.ts, o=lb.o, h=lb.h, l=lb.l, c=lb.c, v=lb.v) if lb else None,
            session_bars=len(self.session_bars),
            total_bars=len(self.bars),
            prev_profile=self.prev_prof,
        )

    def _unrl(self):
        if not self.pos:
            return None
        lb = self.session_bars[-1] if self.session_bars else self.bars[-1]
        px = lb.c if lb else self.pos.entry
        return (px / self.pos.entry - 1) * 100 if self.pos.side == "long" \
            else (1 - px / self.pos.entry) * 100
