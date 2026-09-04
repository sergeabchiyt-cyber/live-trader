# frvp-core — low-latency FRVP PoC engine (Rust)

Exact Rust port of `live/strategy.py` (which is itself a 1:1 port of the validated
backtest engine). No broker, no I/O in the engine — a `Strategy` is fed closed 15m
`Bar`s and emits `Trade`s, so it is the natural hot loop for low-latency feed + strategy
processing. `src/main.rs` wraps it in three commands.

```
$ cargo build --release
$ ./target/release/frvp
frvp (FRVP PoC core)
  frvp replay <csv> [rr] [skip0 0|1]
  frvp bench <csv>
  frvp live [SYMBOL] [POLL_S] [rr] [skip0]
```

## Commands

### `replay` — full-history backtest / parity oracle
`<csv>` is `ts_ms,o,h,l,c,v` rows (one per 15m bar, `v` = tick volume / #trades).
```bash
./target/release/frvp replay ../cache/XAUUSD_15m.csv 2.0 0
```
Acceptance (RR 2, trail ladder on, no hour-skip) prints:
```
trades      : 1804
win rate    : 69.3%
net         : +66.20%
exit mix    : SL 1438 / TP 347 / EOS 19
```
These exact numbers are the parity contract with the Python engine. Set `FRVP_DUMP=1` to
emit one `DUMP` line per trade (compare 1:1 against `python3 live/validate.py` output),
and `FRVP_TRACE=1` for a per-bar entry/manage trace when debugging a session.

### `bench` — latency micro-benchmark
Warm-up run, then a best-of-10 full-engine replay over all bars, plus a hot FRVP/PoC
compute loop (2626 rolling 24 h windows).
```bash
./target/release/frvp bench ../cache/XAUUSD_15m.csv
```
Reference numbers (this repo's test machine, `--release`):
- full-engine replay of 252,158 bars: **~0.037 s best-of-10** (~6.9 M bars/s ≈ 145 ns/bar)
- FRVP/PoC compute: **~0.006 s for 2,626 sessions** (~0.45 M sessions/s)

### `live` — streaming loop over Binance public klines (no keys)
Polls the public market-data endpoint `data-api.binance.vision` for closed 15m klines and
runs the same engine bar-by-bar. `v` is proxied by kline #trades.
```bash
./target/release/frvp live PAXGUSDT 10 2.0 1
./target/release/frvp live XAUTUSDT 5 2.0 0
```
Order execution is *not* part of this binary — it reports session/bias/PoC/trades. Pair it
with `live/broker.py` (Python) or your own execution layer for order routing.

## Parity & conventions (do not "fix" silently)
- Sessions open 22:00 UTC, Sun–Thu starts (weekends flat, no hour 00:00–01:00 entry when `skip0`).
- FRVP profile: 6 × 4H buckets of 15m bars → 48 bins, tick volume spread uniformly across
  each bucket range, **PoC = centre of the first max-volume bin** (Python `prof.index(max)`
  convention — Rust must not use "last max").
- Asymmetric engine exits: long SL on `close ≤ sl` / TP on `high ≥ tp`; short SL on
  `high ≥ sl` / TP on `close ≤ tp`. SL checked first. Same-bar exit scan after entry.
- Milestone trail: 60/75/90 % of the way to TP ratchets the stop to +0.1/+0.4/+0.7R
  (profit-only); TP stays fixed at RR × SL.
- Entry at PoC touch: long `low ≤ PoC`, short `high ≥ PoC`, filled at the PoC level.

`src/engine.rs` has no floating-point surprises vs Python beyond display rounding in the
4th decimal of `entry` (Python `round` = banker's rounding); all per-trade `pct`, exit
type/timestamps and session stats are identical.
