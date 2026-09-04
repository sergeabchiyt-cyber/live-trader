"""Market data feeds -> closed 15M Bars.

BinanceFeed : live PAXGUSDT (or any spot symbol) 15m klines from the PUBLIC
              data-api endpoint (data-api.binance.vision — reachable without keys).
              Tick proxy for the FRVP volume = per-kline 'number of trades'.
ReplayFeed  : replays the backtest XAUUSD 15m CSV in (simulated) real time.
"""
import time, json, threading
import urllib.request
from datetime import datetime, timezone, timedelta
from live.strategy import Bar, session_date

PUBLIC = "https://data-api.binance.vision"


def _get_json(url, timeout=15):
    req = urllib.request.Request(url, headers={"User-Agent": "frvp-trader/1.0"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


class BinanceFeed:
    def __init__(self, symbol="PAXGUSDT", interval="15m", limit_boot=1000):
        self.symbol = symbol
        self.interval = interval
        self.unit_ms = 900_000
        self._klines_url = (f"{PUBLIC}/api/v3/klines?symbol={symbol}&interval={interval}"
                            f"&limit={limit_boot}")
        self.ticker_url = f"{PUBLIC}/api/v3/ticker/price?symbol={symbol}"
        self.last = None
        self.last_price = None
        self.bars = []

    # ---------- klines -> Bars (v = number of trades as tick proxy) ----------
    def _parse(self, k):
        out = []
        for row in k:
            if len(row) < 9:
                continue
            ts = int(row[0]) // 1000
            if ts <= (self.last or -1):
                continue
            b = Bar(ts, row[1], row[2], row[3], row[4], float(row[8]), t=float(row[8]))
            out.append(b)
        return out

    def bootstrap(self):
        """Initial snapshot of closed bars (fetches once)."""
        k = _get_json(self._klines_url)
        now = time.time() * 1000
        closed = [r for r in k if int(r[0]) + self.unit_ms <= now]
        self.bars = self._parse(closed)
        try:
            self.last_price = float(_get_json(self.ticker_url)["price"])
        except Exception:
            pass
        return self.bars

    def poll(self):
        """Return (new_bars, last_price). Fetches the tail of klines; robust to
        misses (limit small so cheap)."""
        n = max(2, min(20, len(self.bars) // 500 + 2))
        url = f"{PUBLIC}/api/v3/klines?symbol={self.symbol}&interval={self.interval}&limit={max(n,3)}"
        k = _get_json(url)
        now = time.time() * 1000
        closed = [r for r in k if int(r[0]) + self.unit_ms <= now]
        newb = self._parse(closed)
        if newb:
            self.bars.extend(newb)
            self.last = self.bars[-1].ts
        try:
            self.last_price = float(_get_json(self.ticker_url)["price"])
        except Exception:
            pass
        return newb, self.last_price


class ReplayFeed:
    """Replays the XAUUSD backtest CSV on the wall clock.
    speed=1 -> realtime (bar every 15 min). speed>1 accelerates.
    All bars before the anchor are served as warm-up (quiet history); only the
    remaining tail is paced live. Default anchor keeps `keep` bars to pace."""
    def __init__(self, csv_path, speed=30.0, start=None, keep=10):
        self.speed = float(speed)
        self.rows = []
        with open(csv_path) as f:
            next(f)
            for line in f:
                p = line.split(",")
                self.rows.append((int(float(p[0])) // 1000, float(p[1]), float(p[2]),
                                  float(p[3]), float(p[4]), float(p[5])))
        self.rows.sort()
        if start:
            ts0 = datetime.strptime(start, "%Y-%m-%d").replace(tzinfo=timezone.utc).timestamp()
            # warm 36h before the requested start so a full PD session exists
            warm_idx = 0
            while warm_idx < len(self.rows) and self.rows[warm_idx][0] < ts0 - 36 * 3600:
                warm_idx += 1
            anchor_idx = warm_idx
            while anchor_idx < len(self.rows) and self.rows[anchor_idx][0] < ts0:
                anchor_idx += 1
        else:
            anchor_idx = max(0, len(self.rows) - keep)
        self.anchor_idx = anchor_idx
        self.idx = self.anchor_idx
        self.last_price = self.rows[anchor_idx][4] if anchor_idx < len(self.rows) else None
        self.pacing_start = None

    def warm_bars(self):
        """Bars strictly before the anchor (feed quietly to the strategy)."""
        out = []
        for r in self.rows[:self.anchor_idx]:
            out.append(Bar(r[0], r[1], r[2], r[3], r[4], r[5]))
        return out

    def start_pacing(self):
        self.pacing_start = time.time()

    def poll(self):
        """Emit due bars: those whose (virtual) offset has passed since pacing began."""
        out = []
        if self.pacing_start is None:
            return out, self.last_price
        if self.idx >= len(self.rows):
            return out, self.last_price
        anchor_ts = self.rows[self.anchor_idx][0]
        elapsed_virtual = (time.time() - self.pacing_start) * self.speed
        while self.idx < len(self.rows):
            r = self.rows[self.idx]
            if (r[0] - anchor_ts) <= elapsed_virtual:
                b = Bar(r[0], r[1], r[2], r[3], r[4], r[5])
                self.last_price = b.c
                self.idx += 1
                out.append(b)
            else:
                break
        return out, self.last_price

    def remaining(self):
        return len(self.rows) - self.idx
