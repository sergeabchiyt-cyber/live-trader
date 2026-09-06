#!/usr/bin/env python3
"""Parity oracle: run the LEGACY Python engine (legacy/live/strategy.py) over a
ts_ms,o,h,l,c,v CSV and emit the trade list as JSON — same shape as
`live-trader replay <csv> --json` (Rust, SL_MODE=close).

Compare with: python3 tools/parity_diff.py trades_py.json trades_rust.json
"""
import json, sys, os

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "legacy", "live"))

from strategy import Strategy, Bar  # noqa: E402


class Cfg:  # mirror of the Rust replay config (engine constants)
    rr = 2.0
    skip_hour0 = False
    trail_on = True
    trail_ladder = [(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)]
    same_bar_exits = True
    atr_n = 14
    day_open_hour = 22
    session_days = (6, 0, 1, 2, 3)
    bins = 48
    sl_min = 0.0005
    sl_max = 0.005


def load_csv(path):
    rows = []
    with open(path) as f:
        next(f)
        for line in f:
            p = line.split(",")
            if len(p) < 6:
                continue
            rows.append((int(float(p[0])) // 1000, float(p[1]), float(p[2]),
                         float(p[3]), float(p[4]), float(p[5])))
    rows.sort()
    return rows


def main(csv_path, out_path):
    strat = Strategy(Cfg())
    for (ts, o, h, l, c, v) in load_csv(csv_path):
        strat.on_bar(Bar(ts, o, h, l, c, v))
    trades = []
    for t in strat.trades:
        trades.append({
            "side": t["side"], "session": t["session"],
            "entry": t["entry"], "sl": t["sl"], "tp": t["tp"],
            "exit": t["exit"], "exit_type": t["exit_type"],
            "pct": t["pct"], "r": t["r"],
            "entry_ts": t["entry_ts"], "exit_ts": t["exit_ts"],
            "bars_held": t["bars_held"], "trail_a": t["trail_a"],
        })
    with open(out_path, "w") as f:
        json.dump(trades, f)
    net = sum(t["pct"] for t in trades)
    print(f"python engine: {len(trades)} trades, net {net:+.2f}% -> {out_path}")


if __name__ == "__main__":
    csv_path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/bars_synth.csv"
    out_path = sys.argv[2] if len(sys.argv) > 2 else "/tmp/trades_py.json"
    main(csv_path, out_path)
