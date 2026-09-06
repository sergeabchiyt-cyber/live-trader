"""Market data feeds -> closed 15M Bars.

BinanceWSFeed : PUSH feed. Bootstraps closed history once from the PUBLIC REST
                data-api, then subscribes to the PUBLIC WebSocket market-data
                stream (wss://.../ws/<symbol>@kline_<interval>).
                venue="spot"   -> data-api/data-stream.binance.vision (SPOT)
                venue="futures"-> fapi.binance.com / fstream.binance.com (USDT-M
                                  perpetuals incl. XAUUSDT gold TRADFI perp)
                Only CLOSED klines (k.x == true) are forwarded as Bars; live
                partial updates only refresh last_price. If the socket drops or
                goes stale, poll() transparently backfills from REST so no closed
                bar is ever missed. No API keys on any path.
BinanceFeed   : legacy REST polling feed (same endpoints, no keys) — kept as a
                fallback (FEED=rest).
BybitFeed     : REST polling feed for Bybit USDT linear perps (XAUUSDT gold
                perp) — public, no keys. DATA_VENUE=bybit (useful where Binance
                blocks datacenter IPs from mainnet futures).
ReplayFeed    : replays the backtest XAUUSD 15m CSV in (simulated) real time.

Tick proxy for the FRVP volume profile = per-kline 'number of trades' (k.n).
"""
import time, json, threading, collections
import urllib.request
from datetime import datetime, timezone, timedelta
from live.strategy import Bar, session_date

PUBLIC = "https://data-api.binance.vision"       # spot public market data (no keys)
STREAM = "wss://data-stream.binance.vision"       # spot public WS
FAPI = "https://fapi.binance.com"                 # USDT-M futures public market data (no keys for md)
FSTREAM = "wss://fstream.binance.com"             # USDT-M futures public WS
BYBIT = "https://api.bybit.com"                   # Bybit v5 public market data (no keys)

# per-venue endpoints (klines rows & kline WS payloads are format-identical)
VENUES = {
    "spot": dict(
        rest=PUBLIC, stream=STREAM,
        klines="/api/v3/klines", ticker="/api/v3/ticker/price",
    ),
    "futures": dict(
        rest=FAPI, stream=FSTREAM,
        klines="/fapi/v1/klines", ticker="/fapi/v1/ticker/price",
    ),
}

try:
    import websocket  # websocket-client
    HAVE_WS = True
except Exception:  # pragma: no cover - optional dependency
    HAVE_WS = False


def _get_json(url, timeout=15, retries=1, backoff=2.0):
    """GET url -> parsed JSON. retries>1 adds linear backoff (boot resilience
    against transient geo-edge 451s / network hiccups)."""
    last = None
    for i in range(max(1, retries)):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "frvp-trader/1.0"})
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read().decode())
        except Exception as ex:
            last = ex
            if i + 1 < max(1, retries):
                time.sleep(backoff * (i + 1))
    raise last


def _interval_ms(interval):
    unit = interval[-1]
    n = int(interval[:-1])
    return {"m": 60_000, "h": 3_600_000, "d": 86_400_000}[unit] * n


class BinanceFeed:
    """REST polling feed (public data-api, no keys). Used for WS bootstrap and
    as a fallback when FEED=rest or the websocket-client lib is unavailable."""

    def __init__(self, symbol="PAXGUSDT", interval="15m", limit_boot=1000,
                 venue="spot"):
        self.symbol = symbol.upper()
        self.interval = interval
        self.venue = venue if venue in VENUES else "spot"
        v = VENUES[self.venue]
        self.unit_ms = _interval_ms(interval)
        self._base = v["rest"]
        self._klines_path = v["klines"]
        self._klines_url = (f"{self._base}{v['klines']}?symbol={self.symbol}"
                            f"&interval={interval}&limit={limit_boot}")
        self.ticker_url = f"{self._base}{v['ticker']}?symbol={self.symbol}"
        self.last = None
        self.last_price = None
        self.bars = []

    def _parse(self, k):
        out = []
        for row in k:
            if len(row) < 9:
                continue
            ts = int(row[0]) // 1000
            if ts <= (self.last if self.last is not None else -1):
                continue
            b = Bar(ts, row[1], row[2], row[3], row[4], float(row[8]), t=float(row[8]))
            out.append(b)
        return out

    def bootstrap(self):
        """Initial snapshot of closed bars (fetches once, with retries — a
        transient failure here would otherwise kill the whole process)."""
        k = _get_json(self._klines_url, retries=6)
        now = time.time() * 1000
        closed = [r for r in k if int(r[0]) + self.unit_ms <= now]
        self.bars = self._parse(closed)
        if self.bars:
            self.last = self.bars[-1].ts
        try:
            self.last_price = float(_get_json(self.ticker_url)["price"])
        except Exception:
            pass
        return self.bars

    def poll(self):
        """Return (new_bars, last_price). Fetches the tail of klines; robust to
        misses (limit small so cheap)."""
        n = max(2, min(20, len(self.bars) // 500 + 2))
        url = (f"{self._base}{self._klines_path}?symbol={self.symbol}"
               f"&interval={self.interval}&limit={max(n,3)}")
        try:
            k = _get_json(url)
        except Exception:
            return [], self.last_price
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

    def poll_ticker(self):
        """Cheap last-price refresh only (used by the ~1s dashboard tick pub)."""
        try:
            self.last_price = float(_get_json(self.ticker_url)["price"])
        except Exception:
            pass


class BinanceWSFeed:
    """PUSH market-data feed over the public Binance WebSocket.

    Architecture (single strategy writer):
      * bootstrap()   — one REST fetch of ~1000 closed klines (warm-up history).
      * start()       — daemon thread subscribes to the kline stream and queues
                        only CLOSED bars (k.x == true). Partial updates refresh
                        last_price for the dashboard but never touch the engine.
      * poll()        — drains queued closed bars (called from the main loop,
                        which remains the ONLY caller of strategy.on_bar, so the
                        engine has a single writer). If the socket is stale or
                        down, poll() backfills the gap from REST automatically.

    The stream auto-reconnects with backoff; websocket-client answers Binance
    server pings and we also send our own keepalive pings.
    """

    def __init__(self, symbol="PAXGUSDT", interval="15m", limit_boot=1000,
                 rest_fallback=True, venue="spot"):
        self.symbol = symbol.upper()
        self.interval = interval
        self.venue = venue if venue in VENUES else "spot"
        self.unit_ms = _interval_ms(interval)
        self.url = (f"{VENUES[self.venue]['stream']}/ws/"
                    f"{self.symbol.lower()}@kline_{interval}")
        self._rest = BinanceFeed(self.symbol, interval, limit_boot,
                                 venue=self.venue)
        self.rest_fallback = rest_fallback and HAVE_WS is False  # no WS lib at all
        self.bars = []                       # warm-up history (bootstrap)
        self.last_price = None
        self._q = collections.deque()
        self._lock = threading.Lock()
        self._running = False
        self._thread = None
        self._wsapp = None
        self._connected = False
        self._connect_fails = 0
        self._last_msg = None                # time.time() of last WS message
        self._last_emitted = None            # ts (s) of newest bar handed out
        self._booted_at = time.time()

    # ---------------- lifecycle ----------------
    def bootstrap(self):
        self.bars = self._rest.bootstrap()
        if self.bars:
            self._last_emitted = self.bars[-1].ts
            self._rest.last = self._last_emitted
        self.last_price = self._rest.last_price
        return self.bars

    def start(self):
        if self._running:
            return
        self._running = True
        self._thread = threading.Thread(target=self._run, daemon=True,
                                        name=f"ws-{self.symbol}")
        self._thread.start()

    def stop(self):
        self._running = False
        ws = self._wsapp
        if ws is not None:
            try:
                ws.close()
            except Exception:
                pass
        self._thread = None

    def connected(self):
        return self._connected

    # ---------------- socket loop (daemon thread) ----------------
    def _on_open(self, ws):
        self._connected = True
        self._connect_fails = 0
        self._last_msg = time.time()
        print(f"[ws-feed] {self.symbol} {self.interval} connected: {self.url}",
              flush=True)

    def _on_message(self, ws, message):
        self._last_msg = time.time()
        try:
            m = json.loads(message)
        except Exception:
            return
        if m.get("e") != "kline":
            return
        k = m.get("k") or {}
        if not k:
            return
        try:
            close_ = float(k["c"])
        except Exception:
            close_ = None
        if close_ is not None:
            self.last_price = close_
        if not k.get("x"):
            return                       # partial open-bar update
        try:
            b = Bar(int(k["t"]) // 1000,
                    float(k["o"]), float(k["h"]), float(k["l"]), close_ or 0.0,
                    float(k.get("n") or 0), t=float(k.get("n") or 0))
        except Exception:
            return
        with self._lock:
            self._q.append(b)

    def _on_error(self, ws, error):
        self._connected = False

    def _on_close(self, ws, code, msg):
        self._connected = False

    def _run(self):
        backoff = 1.0
        while self._running:
            if not HAVE_WS:
                self._connected = False
                return
            try:
                self._wsapp = websocket.WebSocketApp(
                    self.url,
                    on_open=self._on_open,
                    on_message=self._on_message,
                    on_error=self._on_error,
                    on_close=self._on_close,
                )
                # ping_interval keeps the connection alive; websocket-client also
                # auto-answers Binance's server pings.
                self._wsapp.run_forever(ping_interval=60, ping_timeout=15,
                                        skip_utf8_validation=False)
            except Exception:
                pass
            finally:
                self._connected = False
                self._wsapp = None
            if not self._running:
                return
            self._connect_fails += 1
            time.sleep(backoff)
            backoff = min(backoff * 2, 15.0)

    # ---------------- main-thread consumption ----------------
    def _rest_backfill(self):
        """REST tail fetch used only when the stream is stale/down."""
        newb, price = self._rest.poll()
        if price is not None:
            self.last_price = price
        out = []
        for b in newb:
            if self._last_emitted is None or b.ts > self._last_emitted:
                out.append(b)
        return out

    def poll(self):
        """Drain closed bars received over WS since last call.

        Returns (new_bars, last_price). When the socket is down or no message
        has arrived for a while, transparently backfills from REST so the
        strategy never misses a closed bar (acts as a slow poll fallback)."""
        newb = []
        with self._lock:
            while self._q:
                newb.append(self._q.popleft())
        if newb:
            newb.sort(key=lambda b: b.ts)
            keep = []
            for b in newb:
                if self._last_emitted is None or b.ts > self._last_emitted:
                    keep.append(b)
            newb = keep
            if newb:
                self._last_emitted = newb[-1].ts
                self.bars.extend(newb)
        # stale / down? backfill from REST (only after an initial grace period)
        stale = (self._last_msg is not None and
                 time.time() - self._last_msg > 75.0) or \
                (self._last_msg is None and time.time() - self._booted_at > 90.0)
        if self.rest_fallback or (stale and time.time() - self._booted_at > 30.0):
            try:
                fill = self._rest_backfill()
                if fill:
                    if newb:
                        print(f"[ws-feed] {self.symbol}: REST backfill "
                              f"{len(fill)} closed bar(s) — stream stale/down",
                              flush=True)
                    else:
                        print(f"[ws-feed] {self.symbol}: REST fallback "
                              f"({len(fill)} bar(s)); socket "
                              f"{'reconnecting' if HAVE_WS else 'disabled'}",
                              flush=True)
                    self._last_emitted = fill[-1].ts
                    self.bars.extend(fill)
                    newb.extend(fill)
            except Exception:
                pass
        if newb:
            newb.sort(key=lambda b: b.ts)
        return newb, self.last_price


class BybitFeed:
    """REST polling feed for Bybit USDT linear perpetuals (e.g. XAUUSDT gold
    perp) — public, no keys. Used when DATA_VENUE=bybit: Binance blocks
    datacenter IPs from mainnet futures (418), and Bybit's CloudFront WS can
    reject datacenter handshakes, so this is a plain REST poll (POLL_S cadence;
    for a 15m bar strategy a 5s poll is latency-equivalent to a push stream).

    Bybit v5 kline rows: [startMs, open, high, low, close, volume, turnover],
    NEWEST-first; only start+interval <= now rows are closed. The strategy
    weights the volume profile by b.v, so Bybit base volume maps to v (real
    traded volume — Binance feeds use per-bar trade count there instead).
    """

    _IV = {"1m": "1", "3m": "3", "5m": "5", "15m": "15", "30m": "30",
           "1h": "60", "2h": "120", "4h": "240", "1d": "D"}

    def __init__(self, symbol="XAUUSDT", interval="15m", limit_boot=1000):
        self.symbol = symbol.upper()
        self.interval = interval
        self.unit_ms = _interval_ms(interval)
        self._iv = self._IV.get(interval, "15")
        self._klines_url = (f"{BYBIT}/v5/market/kline?category=linear"
                            f"&symbol={self.symbol}&interval={self._iv}"
                            f"&limit={min(limit_boot, 1000)}")
        self.ticker_url = (f"{BYBIT}/v5/market/tickers?category=linear"
                           f"&symbol={self.symbol}")
        self.last = None
        self.last_price = None
        self.bars = []

    @staticmethod
    def _klines(j):
        if j.get("retCode") not in (0, None):
            raise ValueError(f"bybit retCode {j.get('retCode')}: {j.get('retMsg')}")
        return list(reversed(j.get("result", {}).get("list", [])))  # -> oldest-first

    def _parse(self, k):
        out = []
        for row in k:
            if len(row) < 7:
                continue
            ts = int(row[0]) // 1000
            if ts <= (self.last if self.last is not None else -1):
                continue
            vol = float(row[5])
            out.append(Bar(ts, row[1], row[2], row[3], row[4], vol, t=vol))
        return out

    def _ticker(self):
        j = _get_json(self.ticker_url)
        if j.get("retCode") not in (0, None):
            return
        lst = j.get("result", {}).get("list") or []
        if lst:
            self.last_price = float(lst[0]["lastPrice"])

    def bootstrap(self):
        """Initial snapshot of closed bars (retries on transient failures)."""
        rows = self._klines(_get_json(self._klines_url, retries=6))
        now = time.time() * 1000
        closed = [r for r in rows if int(r[0]) + self.unit_ms <= now]
        self.bars = self._parse(closed)
        if self.bars:
            self.last = self.bars[-1].ts
        try:
            self._ticker()
        except Exception:
            pass
        return self.bars

    def poll(self):
        """Return (new_bars, last_price); tolerant of misses."""
        n = max(3, min(20, len(self.bars) // 500 + 3))
        url = (f"{BYBIT}/v5/market/kline?category=linear&symbol={self.symbol}"
               f"&interval={self._iv}&limit={n}")
        try:
            rows = self._klines(_get_json(url))
        except Exception:
            return [], self.last_price
        now = time.time() * 1000
        closed = [r for r in rows if int(r[0]) + self.unit_ms <= now]
        newb = self._parse(closed)
        if newb:
            self.bars.extend(newb)
            self.last = self.bars[-1].ts
        try:
            self._ticker()
        except Exception:
            pass
        return newb, self.last_price

    def poll_ticker(self):
        """Cheap last-price refresh only (used by the ~1s dashboard tick pub)."""
        try:
            self._ticker()
        except Exception:
            pass


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
