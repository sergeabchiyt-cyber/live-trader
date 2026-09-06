#!/usr/bin/env python3
"""Mock Bybit v5 public API for local end-to-end tests (Bybit CloudFront is
blocked from many sandboxes). Serves Bybit-shaped /v5/market/kline and
/v5/market/tickers from a ts_ms,o,h,l,c,v CSV, with a jittered lastPrice.
"""
import json, sys, time, random
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

CSV = sys.argv[1] if len(sys.argv) > 1 else "/tmp/bars_real.csv"
PORT = int(sys.argv[2]) if len(sys.argv) > 2 else 8791

rows = []
with open(CSV) as f:
    next(f)
    for line in f:
        p = line.split(",")
        rows.append((int(float(p[0])), float(p[1]), float(p[2]),
                     float(p[3]), float(p[4]), float(p[5])))
rows.sort()
print(f"mock-bybit: {len(rows)} bars {rows[0][0]} .. {rows[-1][0]} on :{PORT}")

rnd = random.Random(7)


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        u = urlparse(self.path)
        q = parse_qs(u.query)
        if u.path == "/v5/market/kline":
            limit = int(q.get("limit", ["200"])[0])
            now = int(time.time() * 1000)
            # only closed bars (start + 900s <= now), newest-first
            closed = [r for r in rows if r[0] + 900_000 <= now]
            take = closed[-limit:]
            take = list(reversed(take))
            body = json.dumps({"retCode": 0, "retMsg": "OK",
                               "result": {"category": "linear", "symbol": "XAUUSDT",
                                          "list": [[str(r[0]), f"{r[1]:.2f}", f"{r[2]:.2f}",
                                                    f"{r[3]:.2f}", f"{r[4]:.2f}", f"{r[5]:.6f}",
                                                    f"{r[5]*r[4]:.2f}"] for r in take]}}).encode()
        elif u.path == "/v5/market/tickers":
            last = rows[-1][4]
            px = last + rnd.uniform(-1.5, 1.5)
            body = json.dumps({"retCode": 0, "retMsg": "OK",
                               "result": {"category": "linear",
                                          "list": [{"symbol": "XAUUSDT",
                                                    "lastPrice": f"{px:.2f}"}]}}).encode()
        else:
            body = b'{"retCode":404}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
