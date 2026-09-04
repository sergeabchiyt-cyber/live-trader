# FRVP PoC Live Trader — real-time dashboard + Binance integration

Runs the strategy you validated in the backtests (PD FRVP PoC retest, ATR stop,
milestone trailing, optional 00:00–01:00 skip) on **closed 15-minute bars** with
a live dashboard (chart with PD volume profile / PoC, trade log, equity curve).

```
┌───────────────┐   closed 15m bars   ┌──────────────────┐  events  ┌───────────────┐
│  Data feed    │ ──────────────────▶ │  Strategy core   │ ───────▶ │ Broker layer  │
│ PAXGUSDT live │   (same conventions │  (validated 1:1  │          │ paper/testnet │
│  or XAU replay│    as backtest)     │  with backtest)  │          │ live          │
└───────────────┘                     └────────┬─────────┘          └───────┬───────┘
                                               │ SSE events + state         │ orders
                                               ▼                            ▼
                                      ┌─────────────────┐          Binance spot (testnet/live)
                                      │  Dashboard (JS) │
                                      └─────────────────┘
```

**Parity with the backtest is exact.** Replaying the full 2016→2026 XAUUSD CSV
(`cache/XAUUSD_15m.csv`, not committed — see *Data* note)
through this streaming engine reproduces the validated engine trade-for-trade:
1804/1804 sessions identical pct and exit type; net +66.20% on both. Run
`python3 live/validate.py` to confirm yourself.

## Data / symbols — the gold decision
Binance does not list physical XAUUSD; the best liquid tokenised-gold spot is
**PAXG/USDT** (price tracks ~1 oz gold; ~$4,470 now). You can also point
`SYMBOL=XAUTUSDT` at Tether Gold.

* Market data always comes from **Binance public `data-api.binance.vision`**
  (no keys needed; reachable from most regions incl. this sandbox).
* Spot order endpoints (`api.binance.com`, `testnet.binance.vision`) are
  **geo-blocked from this sandbox (HTTP 451)** — order code is included and
  works from your own machine; from here you run **paper** execution on live
  data (fills identical to the backtest assumptions), or the **XAUUSD replay**
  feed to watch it trade the real gold history.

## Quick start

```bash
cd live-trader
# 1) paper on LIVE PAXGUSDT data (default)
python3 -m live.run --port 8765
# 2) demo on gold history (fast replay of the last week of XAUUSD)
python3 -m live.run --source xau --replay-start 2026-08-25 --replay-speed 300 --port 8765
# 3) watch it work headless: curl http://127.0.0.1:8765/api/state
```
Open http://localhost:8765/ → dashboard (candles + gold PD volume-profile/PoC
overlay, position/SL/TP/trail markers, trade log, equity, live events).

> **Data note:** the XAUUSD replay cache is kept out of the repo. To run the replay
> demo and `validate.py`, regenerate it (see the project backtest tooling / Dukascopy
> fetch) or copy your CSV to `cache/XAUUSD_15m.csv` (columns `ts_ms,o,h,l,c,v`).
> Live PAXGUSDT needs no keys.

## Env vars / which mode to use

| Variable | Meaning | Default |
|---|---|---|
| `MODE` | `auto` \| `paper` \| `testnet` \| `live` | `auto` |
| `BINANCE_TESTNET` | `1` to prefer testnet in auto mode | off |
| `BINANCE_API_KEY` / `BINANCE_API_SECRET` | testnet **or** live keys | – |
| `BINANCE_LIVE_ACK` | must equal `yes` to enable live (safety) | – |
| `BINANCE_DRY_RUN` | `0` to actually place orders | `1` (safe) |
| `SYMBOL` | `PAXGUSDT` (or `XAUTUSDT`) | `PAXGUSDT` |
| `DATA_SOURCE` | `paxg` (live) \| `xau` (replay) | `paxg` |
| `RR` | reward:risk target | `2.0` |
| `SKIP_HOUR0` | no entries 00:00–01:00 UTC | `1` |
| `TRAIL` / `TRAIL_LADDER` | milestone trailing on/off & grid | on, `0.6:0.1,0.75:0.4,0.9:0.7` |
| `SAME_BAR` | backtest-parity exits on the entry bar | `1` |
| `POSITION_USD` | notional per trade (paper/live mirror) | `100` |
| `PORT` / `POLL_S` | dashboard port / poll seconds | `8765` / `5` |

**Mode resolution (auto):** live if `BINANCE_API_KEY+SECRET` set and
`BINANCE_LIVE_ACK=yes`; else testnet if `BINANCE_TESTNET=1` or testnet keys are
set; else **paper**. Live/testnet are *always* dry-run unless
`BINANCE_DRY_RUN=0`.

### Testnet (from your machine — recommended first)
```bash
export BINANCE_TESTNET=1
export BINANCE_API_KEY=…            # testnet keys from testnet.binance.vision
export BINANCE_API_SECRET=…
python3 -m live.run --port 8765     # still dry-run; add BINANCE_DRY_RUN=0 to send orders
```
Testnet PAXGUSDT may not exist — pick the most liquid testnet pair and set
`SYMBOL` (the strategy only needs OHLCV + number-of-trades for the profile).

### Live (real money — only after testnet validation)
```bash
export MODE=live BINANCE_LIVE_ACK=yes BINANCE_DRY_RUN=0
export BINANCE_API_KEY=… BINANCE_API_SECRET=…
export SYMBOL=PAXGUSDT POSITION_USD=20
python3 -m live.run --port 8765
```
Live spot flow per trade: LIMIT entry at the PD-PoC level → on fill, a
STOP_LOSS_LIMIT + TAKE_PROFIT_LIMIT pair → re-armed each time the milestone
trail moves the stop → cancelled at exit. **Validate fills vs the paper
columns before scaling up** — real spreads/slippage will differ from the
backtest's exact-fill assumption (backtest sensitivity showed ~0.03%/trade
round-trip cost tolerance; PAXG spreads are small but not free).

## Files
- `run.py`       main loop (feed → strategy → broker → dashboard)
- `config.py`    env/CLI config & mode resolution
- `strategy.py`  streaming engine, validated 1:1 vs `engine.py` (`live/validate.py`)
- `datafeed.py`  BinanceFeed (public klines; v = #trades) & ReplayFeed (XAU CSV)
- `broker.py`    PaperBroker / BinanceBroker(testnet+live), auto dry-run
- `server.py`    http/SSE endpoints; `dashboard_html.py` → UI
- `validate.py`  full-history parity test vs backtest

## Dashboard (served, self-contained, no CDN)
- KPI row: last price, PD PoC, session open, bias, phase, ATR, session, equity
- Main chart: 15m candles, **PD volume profile histogram + PoC line**,
  position entry/SL/TP and trail-lock markers
- Prior-session profile mini-panel · trade log · equity curve · live event log ·
  broker/order status · config panel
- Data endpoints: `/` (app), `/api/state` (JSON), `/api/snapshot` (JSON),
  `/api/events` (SSE push)

## Honest limitations
- Backtest fills are exact (entry at PoC touch, stop/TP at level, bar-close SL
  semantics from the engine). Live will be a little worse; hence paper → testnet
  order and small live size.
- The 00:00–01:00 skip matters more live than in backtest (spread), keep it on.
- PAXG ≈ XAU price but has its own (usually small) basis vs spot gold; volume
  profile uses trade-count proxy.
