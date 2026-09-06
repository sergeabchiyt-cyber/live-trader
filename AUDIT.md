# System Audit — FRVP live trader (pre-Rust-rewrite)

Full review of the deployed Python system (strategy + engineering), with the
disposition of each finding in the v2 Rust rewrite. Empirical numbers were
measured against the live deployment (XAUUSDT @ Bybit, 2026-09-06).

---

## A. Strategy-level findings

### A1. Transaction costs exceed the risk unit — CRITICAL (fixed)
Bybit USDT-perp base fees are 0.02% maker / 0.055% taker. The engine's
ATR-clamped stop on 15m gold is typically 0.05–0.15% of price, so:

| measured (ATR 2.87, px 4430) | value |
|---|---|
| 1R stop distance (slp) | 0.065% |
| fees, stop-out path (maker in + taker out) | 0.075% = **1.16R** |
| fees, TP path (TP is TAKE_PROFIT_MARKET = taker) | 0.075% = 1.16R |
| 2R winner net of fees | **+1.38R** (not +2R) |
| 1R loser net of fees | **−2.16R** (not −1R) |

The legacy paper ledger charged zero commission (`PAPER_COMMISSION_PCT=0`), so
the equity curve was fiction: at these costs the strategy needs ≈59% win rate
at 2:1 RR just to break even (vs 33% at zero fees).

**Fix (v2):** fee-aware ledger — maker entry, taker exit (matches the real
mirror order types), USDT fees + net-R on every trade, `fees_paid` surfaced in
the dashboard. `FEE_MAKER_PCT` / `FEE_TAKER_PCT` env-tunable (0 = legacy).

### A2. Side-asymmetric fill conventions bias the backtest — CRITICAL (fixed)
The validated engine's exit rules were:
* long: SL on bar **close** (survives intra-bar stop-throughs) + TP on
  **touch** (fills on a single trade at the level);
* short: SL on **touch** + TP on bar **close**.

Longs are doubly favoured, shorts doubly penalised. A real exchange
`STOP_MARKET` triggers on touch for both sides, so live results would take
materially more long stop-outs than the backtest showed, and the published
backtest stats (+66% over 1804 trades) carry this long-favouring asymmetry.

**Fix (v2):** `SL_MODE=touch` (default) — SL and TP both fill on touch for
both sides (exchange-realistic), SL checked first when both are touched in one
bar (conservative). `SL_MODE=close` preserves the legacy conventions exactly
(used by the parity harness; parity verified: 133/133 synthetic + 6/6 real
trades identical to the Python engine).

### A3. Ledger vs mirror sizing inconsistency — HIGH (fixed)
The Python ledger compounded each trade's % return on the **full** paper
equity while the exchange mirror traded a fixed `POSITION_USD` notional
(20 USDT in prod) — a ~100x disagreement on what a trade was worth, and the
per-trade $ risk varied ~10x (slp ranges 0.05–0.5%) with no normalisation.

**Fix (v2):** single sizing model shared by ledger and mirror:
`notional = min(equity × RISK_PCT/100 ÷ slp, equity × LEVERAGE_CAP)`
(default 0.5% / 3x). Trade records carry qty/notional/risk. `LEGACY_SIZING=1`
reverts to the fixed notional.

### A4. Single-venue volume profile — MEDIUM (documented, inherent)
The PoC is computed from Bybit-only XAUUSDT volume. Gold's real liquidity is
on COMEX/London; a crypto gold-perp's tape is a thin proxy for "accepted
value", and differs from the XAUUSD spot data the engine was validated on.
No code fix — treat venue as part of the experiment. (The engine PoC and the
dashboard's TV-methodology PoC also differ by design: 4H-bucket FRVP vs
per-bar uniform spread; Δ ≈ $6.7 on the live session checked.)

### A5. Parameter overfit transfer — MEDIUM (documented)
RR=2, the trail ladder (0.6/0.75/0.9 → 0.1R/0.4R/0.7R), skip-hour-0, sl clamps,
22:00 sessions, one-trade-per-session were all tuned on the XAUUSD backtest
and are now running unchanged on a different instrument with different volume
semantics. Revalidation on venue data is recommended before any live sizing.

### A6. Binary bias with zero hysteresis — MEDIUM (documented, unchanged)
Session bias = `open ≥ prev PoC` → a 1-tick difference flips the whole day's
direction. Signal logic was deliberately left 1:1 in v2 (scope decision);
a dead-zone (e.g. ±0.25 ATR → no trade) is the obvious candidate improvement.

### A7. No news/gap filter — MEDIUM (documented, partially mitigated)
Gold is macro-event driven (FOMC/CPI/NFP at 13:30–19:00 UTC). The only filter
is skip-hour-0. With SL_MODE=touch the paper results now at least reflect
intra-bar spikes honestly instead of hiding them behind close-based stops.

### A8. Same-bar entry+exit fills — LOW (documented, unchanged)
`SAME_BAR=1` lets the entry bar's remainder hit SL/TP (backtest parity).
Real fills make this plausible (resting orders), but it inflates turnover on
the tightest stops. `SAME_BAR=0` available for next-bar management.

### A9. ATR floor binding — LOW (documented)
With 15m gold ATR ≈ 0.06–0.10% of price, the 0.05% `SL_MIN` clamp is often
near-binding → the tightest (most fee-dominated) stops dominate the sample.
Interaction with A1; consider raising the floor or widening the timeframe.

---

## B. Engineering findings

### B1. Render free tier sleeps after 15 min without inbound traffic — CRITICAL (mitigated)
The trader was **offline whenever nobody had the dashboard open**. In paper
mode the replay-bootstrap masked it (strategy = pure function of bars), but
with `BINANCE_DRY_RUN=0` (testnet mirroring armed), any signal firing while
asleep never reaches the exchange (warm replay suppresses emits by design).

**Mitigation (v2):** `/api/ping` + `/api/health` endpoints for an external
uptime pinger, 30s server-side WS keepalive pings, and this runbook note:
point a free pinger (e.g. UptimeRobot, 5-min interval) at
`https://live-trader-pxjv.onrender.com/api/ping`, or upgrade to a paid
instance ($7/mo Starter) for always-on. This cannot be fully fixed in code —
Render's spin-down is inbound-traffic based.

### B2. Silent tick-loop failure — HIGH (fixed)
`except Exception: pass` in the Python tick loop meant a Bybit outage (e.g. a
403 IP ban) silently froze ticks forever with zero visibility.

**Fix (v2):** every poll updates a feed-health struct (consecutive failures,
last error, last OK ts) published in `state.feed_health`, on `/api/health`
and as a dashboard LED.

### B3. Hand-rolled WS without server pings — MEDIUM (fixed)
No keepalive from the server side; idle proxies could drop connections.
**Fix:** 30s server-initiated pings; dead-client pruning unchanged.

### B4. Resource footprint & cold starts — MEDIUM (fixed by the rewrite)
Python: ~90MB RSS, 30–60s cold start on free tier (which sleeps — see B1),
deploys pull pip packages. Rust v2: **2.9MB static musl binary, ~10MB RSS,
<1s cold start**, zero runtime dependencies (crates compile at build time
only). Note: tick latency is network-bound (Bybit RTT), so ticks are not
"faster" — the wins are footprint, restart speed and deploy simplicity.

### B5. No auth on endpoints — LOW for paper, REQUIRED before live
Dashboard/WS/state are public read-only (no mutation endpoints exist, secrets
never leave the server). Acceptable for a paper testnet tracker; **add auth
before attaching a funded live key**.

### B6. Statelessness (no disk) — turned out to be a strength
Render free tier has no persistent disk, but the engine is a pure function of
bars: on every restart it bootstraps ~1000 closed bars and deterministically
replays the current session, and the ledger replays all trades with fees.
(Exchange divergence while asleep remains — see B1.)

### B7. Rate limits — cleared
Bybit allows 600 requests / 5s / IP. The service uses ~0.72 req/s (1s ticker +
5s klines) ≈ 0.6% of budget. Not a constraint.

---

## C. v2 behavioural changes vs the Python service (complete list)

1. `SL_MODE=touch` by default (was: side-asymmetric close/touch conventions).
   `SL_MODE=close` restores legacy behaviour exactly.
2. Fees on by default in the paper ledger: 0.02% maker entry, 0.055% taker on
   every exit (SL/TP/EOS are all market-type exits on the mirror). Was 0.
3. Sizing: risk-based (0.5% of equity, notional capped at 3x equity), shared
   by ledger and mirror. Was: fixed 20 USDT notional on the exchange while the
   ledger compounded full equity. `LEGACY_SIZING=1` reverts.
4. Close events / trade records now carry qty, notional, risk_usd, fees,
   pnl_usd and net_r (additive fields; existing field names unchanged).
5. `/api/health` + `/api/ping` added; `/api/state` gains `feed_health`,
   `engine`, extended `config`; `recent_events` seeds the dashboard console.
6. All env variable names from v1 still work unchanged on Render.

Everything else — session calendar, FRVP/PoC math, ATR stops, trail ladder,
entry trigger, one-trade-per-session, EOS handling, warm-replay bootstrap,
event shapes, the SSE endpoint, the WS message envelope — is 1:1, verified by
trade-level parity (synthetic 30k bars: 133/133 identical; real testnet bars:
6/6 identical).
