#!/usr/bin/env python3
"""Deterministic synthetic 15m bar generator for engine parity testing.

Same seed -> same bars, so the Rust and Python engines can be compared
trade-by-trade on a deep history (many sessions, entries, trails, exits).
Output CSV: ts_ms,o,h,l,c,v  (same format as the backtest cache).
"""
import sys

def xorshift64(seed):
    s = seed & 0xFFFFFFFFFFFFFFFF
    assert s != 0
    while True:
        s ^= (s << 13) & 0xFFFFFFFFFFFFFFFF
        s ^= s >> 7
        s ^= (s << 17) & 0xFFFFFFFFFFFFFFFF
        yield s

def main(path, n=30000, seed=0x123456789ABCDEF, start_ts=1735689600000):
    # start: 2025-01-01T00:00:00Z, 15m bars, contiguous (incl. weekends)
    g = xorshift64(seed)
    def r01():
        return next(g) / 2**64
    px = 4400.0
    drift = 0.0
    rows = []
    for i in range(n):
        if i % 96 == 0:  # new "session" regime
            drift = (r01() - 0.5) * 30.0
        o = px
        step = drift / 96.0 + (r01() - 0.5) * 3.0
        c = o + step
        hi = max(o, c) + r01() * 1.8
        lo = min(o, c) - r01() * 1.8
        v = 40.0 + r01() * 160.0
        px = c
        ts = start_ts + i * 900_000
        rows.append((ts, o, hi, lo, c, v))
    with open(path, "w") as f:
        f.write("ts,o,h,l,c,v\n")
        for r in rows:
            f.write("%d,%.4f,%.4f,%.4f,%.4f,%.4f\n" % r)
    print(f"wrote {len(rows)} bars -> {path}")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "/tmp/bars_synth.csv",
         int(sys.argv[2]) if len(sys.argv) > 2 else 30000)
