# live-trader — FRVP PoC gold trading bot (Python dashboard + Rust core)

A real-time **prev-day FRVP PoC** intraday strategy for tokenised/spot gold, shipping in two
languages that are **byte-for-byte engine-parity equivalent**:

| Component | Language | What it is |
|---|---|---|
| `live/` | Python | Streaming strategy, WebSocket data feed (push, no polling), broker layer (paper/testnet/live), HTTP+SSE dashboard, full-history parity check |
| `rust/` | Rust | Same engine as a low-latency core — replay, benchmark and live feed loop (`frvp` binary) |

The strategy: each session opens **22:00 UTC Sun–Thu**; the **previous session's volume
profile** (FRVP, 4H buckets → 48 bins, PoC = centre of max-volume bin) sets the day bias
(open above PD PoC → long the first touch of PoC; below → short). Stops are ATR14(15M),
TP is fixed at `RR × SL`, and a **milestone trail** (60 % → +0.1R, 75 % → +0.4R,
90 % → +0.7R) ratchets the stop toward profit. SL fills first on a same-bar conflict;
one trade per session, flat at session end.

> **Validation.** Replaying the full XAUUSD 15m history (2016-01 → 2026-09, 252,158 bars,
> RR 2, trail ladder on, no hour-skip) reproduces the validated engine trade-for-trade:
> **1,804 trades, net +66.20 %, SL 1438 / TP 347 / EOS 19** — identical in Python and Rust
> (see [Parity](#parity)).

---

## Why Rust?

Python (`live/strategy.py`) is the reference engine and the dashboard backend — but a feed +
strategy core is pure CPU work, so the same engine is ported 1:1 to Rust
(`rust/src/engine.rs`). Measured on this machine (best-of-N full-history replay):

| Engine | Best time (252,158 bars) | Throughput |
|---|---|---|
| Python `live.strategy` | ~1.23 s | ~0.20 M bars/s |
| Rust `frvp replay` (release) | ~0.037 s | ~6.9 M bars/s |

**≈ 33× faster engine**, identical output. Ten years of 15-minute bars replay in ~37 ms;
a single closed bar (the live hot path) is ~150 ns of strategy work. The Rust `bench`
subcommand reports both full-engine and FRVP/PoC-compute throughput.

---

## Layout

```
live-trader/
├── live/                  # Python package (validated engine + dashboard)
│   ├── strategy.py        #   streaming engine (1:1 with the backtest engine)
│   ├── datafeed.py        #   Binance public klines (PAXG) / XAU replay feed
│   ├── broker.py          #   PaperBroker + Binance testnet/live, auto dry-run
│   ├── config.py          #   env/CLI config + mode resolution
│   ├── run.py             #   main loop + CLI entry (python3 -m live.run)
│   ├── server.py          #   HTTP + SSE endpoints
│   ├── dashboard.html     #   self-contained UI (no CDN)
│   ├── validate.py        #   full-history parity test
│   └── README.md          #   Python product docs (modes, env, testnet/live flow)
├── rust/                  # Rust core (crate frvp-core → binary frvp)
│   ├── src/engine.rs      #   exact engine port (no broker)
│   ├── src/main.rs        #   frvp replay | bench | live
│   ├── Cargo.toml
│   └── README.md          #   build / parity / bench / live-feed docs
└── README.md
```

---

## Python quick start

```bash
python3 -m venv .venv && . .venv/bin/activate   # optional
cd live-trader

# 1) paper trading on LIVE PAXGUSDT data (default) -> dashboard on :8765
python3 -m live.run --port 8765

# 2) demo on gold history (fast replay of recent XAUUSD)
python3 -m live.run --source xau --replay-start 2026-08-25 --replay-speed 300 --port 8765

# 3) state check while running
curl http://127.0.0.1:8765/api/state
```

Open http://localhost:8765/ — candles with the **PD volume-profile + PoC overlay**,
position/SL/TP/trail markers, trade log, equity curve and live events.

**Data note:** the XAUUSD 15m replay dataset (`cache/XAUUSD_15m.csv`, 252,158 bars) is
*not* committed to this repo — see `live/README.md` for the fetch/regen instructions,
and `python3 live/validate.py` once you have it at `cache/XAUUSD_15m.csv`. PAXGUSDT live
data needs no keys — the feed is a **public WebSocket push**
(`data-stream.binance.vision`; set `FEED=rest` to fall back to polling).

Modes (env-driven): `paper` (default, exact-fill on live data) · `testnet` · `live`.
Live/testnet are always dry-run unless you opt out — see the env table in `live/README.md`.

## Rust quick start

```bash
cd rust
cargo build --release                      # -> rust/target/release/frvp

# replay with the parity config (expect: 1804 trades, net +66.20%)
./target/release/frvp replay ../cache/XAUUSD_15m.csv 2.0 0

# latency benchmark (best-of-10 full-history replay + FRVP compute)
./target/release/frvp bench ../cache/XAUUSD_15m.csv

# live PAXGUSDT 15m loop over Binance public klines (no keys)
./target/release/frvp live PAXGUSDT 10 2.0 1
```

## Parity

`rust/src/engine.rs` is a straight port of `live/strategy.py` — same session calendar
(22:00 roll, Sun–Thu starts), same FRVP/PoC math (first-max bin convention), same ATR14,
same asymmetric SL/TP fill conventions, same-bar exit scan and milestone ladder. Proven by
replaying identical bars through both engines:

```
python3 live/validate.py          # Python vs backtest oracle  -> 0 mismatches
frvp replay cache/XAUUSD_15m.csv 2.0 0   # Rust vs Python trade list -> byte-identical
```

Result (both): **1804 trades · net +66.20 % · SL 1438 / TP 347 / EOS 19**.

## Honest limitations

- Binance **spot order endpoints** (`api.binance.com`, `testnet.binance.vision`) are
  geo-blocked from some sandboxes (HTTP 451) — the order code is included and validated
  from a normal machine; from restricted environments you get paper execution on live data
  or the XAUUSD replay feed.
- Backtest fills are exact (touch-at-PoC, level SL/TP). Expect modest slippage live —
  start paper, then testnet, then small size.
- The 00:00–01:00 UTC entry skip (`SKIP_HOUR0`) matters more live than in backtest; keep it on.
