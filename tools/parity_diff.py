#!/usr/bin/env python3
"""Diff two trade-list JSONs (python oracle vs rust replay) field by field."""
import json, sys

FIELDS = ["side", "session", "exit_type", "entry", "sl", "tp", "exit",
          "pct", "r", "entry_ts", "exit_ts", "bars_held", "trail_a"]
FLOATS = {"entry", "sl", "tp", "exit", "pct", "r", "trail_a"}


def main(a_path, b_path):
    a = json.load(open(a_path))
    b = json.load(open(b_path))
    print(f"python trades: {len(a)}   rust trades: {len(b)}")
    if len(a) != len(b):
        print("FAIL: trade count mismatch")
        # show first divergence context
        for i in range(min(len(a), len(b))):
            if a[i]["session"] != b[i]["session"] or a[i]["entry_ts"] != b[i]["entry_ts"]:
                print(f"  first divergence at index {i}:")
                print(f"    py  : {a[i]}")
                print(f"    rust: {b[i]}")
                break
        sys.exit(1)
    bad = 0
    for i, (x, y) in enumerate(zip(a, b)):
        for f in FIELDS:
            xv, yv = x[f], y[f]
            if f in FLOATS:
                if abs(float(xv) - float(yv)) > 1e-6:
                    bad += 1
                    if bad <= 10:
                        print(f"  trade[{i}] {f}: py={xv} rust={yv}")
            else:
                if xv != yv:
                    bad += 1
                    if bad <= 10:
                        print(f"  trade[{i}] {f}: py={xv!r} rust={yv!r}")
    if bad:
        print(f"FAIL: {bad} field mismatches")
        sys.exit(1)
    net_a = sum(t["pct"] for t in a)
    net_b = sum(t["pct"] for t in b)
    print(f"PARITY OK: {len(a)} trades identical "
          f"(net py {net_a:+.4f}% vs rust {net_b:+.4f}%)")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
