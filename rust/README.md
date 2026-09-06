# live-trader (Rust) — v2 service crate

The full live service (feed + engine + ledger + broker mirror + HTTP/WS
server + embedded dashboard). See the repo-root [README](../README.md) and
[AUDIT](../AUDIT.md) for architecture, environment and deployment.

## Commands

```
cargo run                                   # serve (env-configured)
cargo run --release -- replay <csv>         # parity replay -> human summary
cargo run --release -- replay <csv> --json  # parity replay -> trades JSON
cargo run --release -- replay <csv> --touch # SL_MODE=touch variant
cargo run --release -- bench <csv>          # timing micro-benchmark
cargo test                                  # engine unit tests
```

`<csv>` rows are `ts_ms,o,h,l,c,v` (one per closed 15m bar).

## Parity contract

`replay` with default flags (`SL_MODE=close`, RR 2, trail ladder on, no
hour-skip, same-bar exits) must produce a trade list identical to
`legacy/live/strategy.py` on the same CSV — verified by `tools/parity_diff.py`
(synthetic 30k bars: 133/133 identical; real Binance-futures-testnet bars:
6/6 identical). The historical contract vs the original backtest engine
(n=1804 trades, net +66.20%, SL/TP/EOS = 1438/347/19 on the XAUUSD cache)
remains valid for `--json` output under the same flags.

## Release build (static musl)

```
rustup target add x86_64-unknown-linux-musl
apt-get install musl-tools        # provides musl-gcc (ring needs it)
cargo build --release --target x86_64-unknown-linux-musl
```

→ ~2.9MB static binary (`bin/live-trader-x86_64` in the repo is what the
live Render service executes).
