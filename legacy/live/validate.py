#!/usr/bin/env python3
"""Parity check: replay the full XAUUSD backtest CSV through the live Strategy
and compare trade-by-trade with the validated backtest engine (trail, no-skip,
RR 2). Verifies the streaming port matches the backtest within documented
edge cases (same-bar fill timing)."""
import os, sys, csv
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from live.config import Config
from live.strategy import Strategy, Bar

LADDER = [(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)]

def load_csv(path):
    rows = []
    with open(path) as f:
        next(f)
        for line in f:
            p = line.split(",")
            rows.append((int(float(p[0])) // 1000, float(p[1]), float(p[2]),
                         float(p[3]), float(p[4]), float(p[5])))
    rows.sort()
    return rows

def run_live(cfg):
    strat = Strategy(cfg)
    n = 0
    for (ts, o, h, l, c, v) in load_csv(cfg_CSV):
        strat.on_bar(Bar(ts, o, h, l, c, v))
        n += 1
    return strat

cfg_CSV = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                       "cache", "XAUUSD_15m.csv")
cfg = Config(rr=2.0, skip_hour0=False, trail_on=True, trail_ladder=LADDER,
             data_source="xau", xau_csv=cfg_CSV)
strat = run_live(cfg)
trades = strat.trades
net = sum(t["pct"] for t in trades)
import collections
cnt = collections.Counter(t["exit_type"] for t in trades)
print(f"LIVE port : n={len(trades)}  net={net:+.2f}%  exit={dict(cnt)}")

# compare vs backtest file
import pandas as pd
bt = pd.read_csv(os.path.join(os.path.dirname(cfg_CSV), "results", "trades_trail_noSkip.csv"))
bt = bt[bt.rr == 2.0]
print(f"BACKTEST  : n={len(bt)}  net={bt.pct.sum():+.2f}%  exit={bt.exit_type.value_counts().to_dict()}")
print(f"\nTrade-level closeness: live={len(trades)} vs backtest={len(bt)}")

# join on session to find differences
ls = {}
for t in trades:
    ls[t["session"]] = t
btmap = {}
for _, r in bt.iterrows():
    btmap[r.date] = dict(pct=r.pct, exit_type=r.exit_type, entry=r.entry, exit=r.exit, side=r.side)
both = set(ls) & set(btmap)
diffs = 0
for d in sorted(both):
    a, b = ls[d], btmap[d]
    if abs(a["pct"] - b["pct"]) > 1e-6 or a["exit_type"] != b["exit_type"]:
        diffs += 1
only_live = set(ls) - set(btmap)
only_bt = set(btmap) - set(ls)
print(f"sessions in both={len(both)} differing={diffs}  only-live={len(only_live)} only-backtest={len(only_bt)}")
net_both_live = sum(ls[d]["pct"] for d in both)
net_both_bt = sum(btmap[d]["pct"] for d in both)
print(f"net on shared sessions: live={net_both_live:+.2f}  bt={net_both_bt:+.2f}  diff={net_both_live-net_both_bt:+.2f}")
print("\nsample differing sessions (date, live pct/type vs bt pct/type):")
shown = 0
for d in sorted(both):
    a, b = ls[d], btmap[d]
    if abs(a["pct"] - b["pct"]) > 1e-6:
        print(f"  {d}: live {a['pct']:+.4f} {a['exit_type']}  vs  bt {b['pct']:+.4f} {b['exit_type']}")
        shown += 1
        if shown >= 8:
            break
if only_live:
    print("only-live sessions:", sorted(only_live)[:8])
if only_bt:
    print("only-backtest sessions:", sorted(only_bt)[:8])
