// tvprofile.js — TradingView-methodology volume profile (pure functions).
// Extracted verbatim into live/dashboard.html; this copy is for node testing.
//
// Methodology per TradingView's "Volume profile indicators: basic concepts":
//  * rows: fixed count over the period's hi-lo range
//  * each bar's volume is distributed across the rows its h-l range spans,
//    proportional to the overlap; close>=open counts as up volume, else down
//  * POC = centre of the max-volume row (first max, like the engine)
//  * Value Area (default 70% of total volume): start at the POC row; compare
//    the next row above the VA with the next row below; add the larger —
//    but STOP as soon as a row would overshoot the target. Ties: the row
//    closer to the POC, then the row above. VAH/VAL = top/bottom of the VA.
//
// Sessions mirror live/strategy.py exactly: a session labelled D opens at
// 22:00 UTC on day D, but ONLY when D is Sun..Thu (engine session_days
// (6,0,1,2,3) in Python weekday numbering = Sun,Mon,Tue,Wed,Thu). Friday and
// Saturday labels are off-calendar: those bars extend no session.

"use strict";

function engineSessionKey(ts) {
  // session label ("YYYY-MM-DD") for a bar ts, or null when off-calendar.
  const d = new Date(ts * 1000);
  let y = d.getUTCFullYear(), m = d.getUTCMonth(), day = d.getUTCDate();
  if (d.getUTCHours() < 22) {                 // before the roll -> previous day's session
    const p = new Date(Date.UTC(y, m, day - 1));
    y = p.getUTCFullYear(); m = p.getUTCMonth(); day = p.getUTCDate();
  }
  const wd = new Date(Date.UTC(y, m, day)).getUTCDay();   // JS: Sun=0..Sat=6
  return wd <= 4 ? `${y}-${String(m + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}` : null;
}

function splitSessions(bars) {
  // bars (oldest->newest) -> [{key, bars}] for on-calendar sessions, in order.
  const out = [];
  let cur = null;
  for (const b of bars) {
    const k = engineSessionKey(b.ts);
    if (k === null) continue;                 // off-calendar bar (Fri/Sat label)
    if (!cur || cur.key !== k) { cur = { key: k, bars: [] }; out.push(cur); }
    cur.bars.push(b);
  }
  return out;
}

function buildProfile(bars, rows, vaPct = 70) {
  if (!bars || bars.length < 3) return null;
  let hi = -Infinity, lo = Infinity;
  for (const b of bars) { if (b.h > hi) hi = b.h; if (b.l < lo) lo = b.l; }
  if (!(hi > lo) || !isFinite(hi)) return null;
  const rowH = (hi - lo) / rows;
  const vol = new Array(rows).fill(0);
  const up = new Array(rows).fill(0);
  for (const b of bars) {
    const v = Number(b.v) || 0;
    if (v <= 0) continue;
    const span = (b.h - b.l) || rowH;
    const isUp = b.c >= b.o;
    let r0 = Math.floor((b.l - lo) / rowH);
    let r1 = Math.floor((b.h - lo - 1e-9) / rowH);
    if (r0 < 0) r0 = 0;
    if (r1 > rows - 1) r1 = rows - 1;
    for (let r = r0; r <= r1; r++) {
      const rLo = lo + r * rowH, rHi = rLo + rowH;
      const ov = (Math.min(b.h, rHi) - Math.max(b.l, rLo)) / span;
      if (ov <= 0) continue;
      const add = v * ov;
      vol[r] += add;
      if (isUp) up[r] += add;
    }
  }
  const total = vol.reduce((a, b) => a + b, 0);
  if (total <= 0) return null;
  // POC: max-volume row, first maximum
  let pocIdx = 0;
  for (let r = 1; r < rows; r++) if (vol[r] > vol[pocIdx]) pocIdx = r;
  const poc = lo + (pocIdx + 0.5) * rowH;
  // Value Area — TradingView algorithm
  const target = total * vaPct / 100;
  let acc = vol[pocIdx], vaLo = pocIdx, vaHi = pocIdx;
  let upNext = pocIdx + 1, dnNext = pocIdx - 1;
  while (acc < target) {
    const above = upNext < rows ? vol[upNext] : -1;
    const below = dnNext >= 0 ? vol[dnNext] : -1;
    if (above < 0 && below < 0) break;
    let pickAbove;
    if (above < 0) pickAbove = false;
    else if (below < 0) pickAbove = true;
    else if (above > below) pickAbove = true;
    else if (above < below) pickAbove = false;
    else pickAbove = Math.abs(upNext - pocIdx) <= Math.abs(dnNext - pocIdx);
    const idx = pickAbove ? upNext : dnNext;
    if (acc + vol[idx] > target) break;         // TV: stop on would-be overshoot
    acc += vol[idx];
    if (pickAbove) { vaHi = upNext; upNext++; } else { vaLo = dnNext; dnNext--; }
  }
  return {
    hi, lo, rowH, rows, vol, up, total,
    poc, pocIdx,
    vah: lo + (vaHi + 1) * rowH,
    val: lo + vaLo * rowH,
    vaPct: (acc / total) * 100,
    maxVol: Math.max(...vol),
  };
}

if (typeof module !== "undefined") {
  module.exports = { engineSessionKey, splitSessions, buildProfile };
}
