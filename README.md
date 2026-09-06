# FRVP live trader v2 — Rust

Live session-mean-reversion trader for the XAUUSDT gold perp: prior-session
volume-profile value (PoC) as the reference level, ATR-scaled stops, milestone
trailing, one trade per session, flat by session end. **Rust service**:
Bybit/Binance REST feed → FRVP engine → fee-aware paper ledger → optional
Binance futures-testnet order mirroring → HTTP + WebSocket push → embedded
terminal dashboard.

> **v2 audit:** see [`AUDIT.md`](AUDIT.md) for the full flaw research this
> rewrite is based on (fee economics, fill asymmetry, sizing inconsistency,
> Render free-tier sleep, feed-health visibility) and the complete list of
> behavioural changes. The Python implementation is preserved under
> [`legacy/`](legacy/) — trade-level parity is verified by
> `tools/parity_*.py` (133/133 synthetic + 6/6 real trades identical).

## Layout

```
rust/            the Rust service (cargo crate)
  src/engine.rs    FRVP strategy core (1:1 port + SL_MODE, events, state)
  src/feed.rs      Bybit / Binance public REST feeds + health telemetry
  src/ledger.rs    fee-aware paper ledger + risk-based sizing
  src/broker.rs    Binance futures/spot order mirror (testnet/live)
  src/http.rs      threaded HTTP server (dashboard, state, SSE, WS, health)
  src/ws.rs        RFC 6455 framing + 30s keepalive pings
  src/main.rs      serve loop + replay/bench subcommands
  dashboard/       terminal UI (embedded into the binary via include_str!)
bin/             prebuilt static musl binary (what the live service runs)
legacy/          the previous Python implementation (reference)
tools/           parity harness + mock Bybit server + bar generator
```

## Run locally

```bash
cd rust && cargo run                # serve on :8765 (env-configured)
cargo test                          # engine unit tests

# parity check against the legacy Python engine
python3 tools/gen_bars.py /tmp/bars.csv 30000
cargo run --release -- replay /tmp/bars.csv --json > /tmp/trades_rust.json
python3 tools/parity_python.py /tmp/bars.csv /tmp/trades_py.json
python3 tools/parity_diff.py /tmp/trades_py.json /tmp/trades_rust.json

# end-to-end without Bybit access (mock v5 API)
python3 tools/mock_bybit.py /tmp/bars_real.csv 8791 &
BYBIT_BASE=http://127.0.0.1:8791 PORT=8790 cargo run
```

## Environment

All v1 variable names still work. New v2 economics knobs (defaults shown):

| var | default | meaning |
|---|---|---|
| `SL_MODE` | `touch` | `touch` = exchange-realistic fills; `close` = legacy backtest conventions |
| `FEE_MAKER_PCT` | `0.02` | paper-ledger maker fee (entry LIMIT) |
| `FEE_TAKER_PCT` | `0.055` | taker fee (SL/TP/EOS are market-type exits) |
| `SL_SLIP_PCT` | `0` | adverse slippage charged on stop fills |
| `RISK_PCT` | `0.5` | % of paper equity risked per trade |
| `LEVERAGE_CAP` | `3.0` | notional cap = cap × equity |
| `LEGACY_SIZING` | `0` | `1` = fixed `POSITION_USD` notional (v1 behaviour) |
| `BYBIT_BASE` | bybit.com | override for tests / geo-blocks |

Legacy: `SYMBOL`, `DATA_VENUE` (bybit|spot|futures), `VENUE`, `MODE`,
`BINANCE_TESTNET`, `BINANCE_DRY_RUN`, `BINANCE_API_KEY/SECRET`, `RR`,
`SKIP_HOUR0`, `TRAIL`, `TRAIL_LADDER`, `SAME_BAR`, `POSITION_USD`, `POLL_S`,
`PORT`, `HOST`, `PAPER_START_USDT`.

## HTTP API

| endpoint | description |
|---|---|
| `/` | terminal dashboard (single file, no CDN) |
| `/api/ws` | WebSocket push: `state` (every poll), `tick` (~1s), `open`/`close`/`trail`/`session` events — envelope `{type, ts, data}` |
| `/api/state` | latest state snapshot (REST fallback / initial load) |
| `/api/events` | SSE stream (legacy compatibility) |
| `/api/health` | liveness + engine version (uptime pingers: `/api/ping`) |
| `/api/snapshot` | all hub snapshots (state + last event per kind) |

## Deployment (Render)

* **Fresh service:** use `render.yaml` (Docker build, ~15MB image).
* **The existing service** (`live-trader-pxjv.onrender.com`, created with
  runtime: python — runtime is immutable on Render): runs the committed static
  binary directly:
  * buildCommand: `echo "no build (prebuilt static binary in repo)"`
  * startCommand: `./bin/live-trader-x86_64`
* Rebuild the committed binary after code changes:
  ```bash
  cd rust && cargo build --release --target x86_64-unknown-linux-musl
  cp target/x86_64-unknown-linux-musl/release/live-trader ../bin/live-trader-x86_64
  ```
* **Keep-alive (free tier):** services sleep after 15 min without inbound
  traffic — point an uptime pinger at `/api/ping`, or use a paid instance
  (AUDIT.md B1). This matters as soon as order mirroring is enabled.

## Honest-economics quick reference

At 15m-gold ATR ≈ 0.065% stops, one round trip costs ≈ 0.075% ≈ **1.16R** —
a 2R winner nets ≈ +1.38R, a 1R loser nets ≈ −2.16R. The dashboard shows
fees, net-R and `fee/1R` live so results are never flattering by accident.
