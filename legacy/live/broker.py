"""Order execution layer.

PaperBroker  (default) — matches the strategy's assumed fills (entry at the PoC
              level, stops/TP at levels, SL first), compounds a paper balance,
              subtracts cfg.paper_commission_pct (round-trip % price) per close.

TestnetBroker / LiveBroker — mirror strategy events to Binance
              (spot: testnet.binance.vision / api.binance.com — LIMIT entries,
              stop-limit SL + LIMIT TP pair;
              futures: testnet.binancefuture.com / fapi.binance.com — LIMIT
              entries, reduceOnly STOP_MARKET SL + TAKE_PROFIT_MARKET TP),
              re-armed whenever the trail moves the stop. NOTE: some sandboxes
              are geo-blocked from Binance (HTTP 451) so these are shipped as
              reference code — always validate on testnet from your own machine
              before enabling live.

Every order method is a no-op unless cfg.tradable == True (requires
MODE=testnet|live + keys + BINANCE_DRY_RUN=0). Dry-run is always safe.
"""
import time, hmac, hashlib, json
import urllib.request, urllib.parse


class PaperBroker:
    def __init__(self, cfg):
        self.cfg = cfg
        self.start_equity = cfg.paper_start_usdt
        self.equity = cfg.paper_start_usdt
        self.trades = []
        self.equity_curve = [dict(ts=time.time(), eq=cfg.paper_start_usdt)]

    def on_open(self, ev):
        return None

    def on_trail(self, ev):
        return None

    def apply_trade(self, ev):
        pct = ev["pct"]
        self.equity = self.equity * (1 + (pct - self.cfg.paper_commission_pct) / 100.0)
        self.trades.append(ev)
        self.equity_curve.append(dict(ts=ev.get("exit_ts") or time.time(), eq=self.equity))

    def on_close(self, ev):
        self.apply_trade(ev)

    def balance(self):
        return dict(broker="paper", equity=self.equity, start=self.start_equity,
                    n_trades=len(self.trades), unrealized=None)


class _BinanceRest:
    qty_prec = 6          # quantity rounding (spot default; futures overrides)

    def __init__(self, key, secret, base):
        self.key, self.secret, self.base = key, secret, base

    def _call(self, method, path, params=None, signed=False):
        url = self.base + path
        headers = {"X-MBX-APIKEY": self.key}
        body = None
        if signed:
            params = dict(params or {})
            params["timestamp"] = int(time.time() * 1000)
            params["recvWindow"] = 5000
            params["signature"] = hmac.new(self.secret.encode(),
                                           urllib.parse.urlencode(params).encode(),
                                           hashlib.sha256).hexdigest()
            body = urllib.parse.urlencode(params).encode()
        elif params:
            url += "?" + urllib.parse.urlencode(params)
        req = urllib.request.Request(url, data=body, headers=headers, method=method)
        with urllib.request.urlopen(req, timeout=15) as r:
            return json.loads(r.read().decode())

    def ping(self):
        return self._call("GET", "/api/v3/ping")

    def account(self):
        return self._call("GET", "/api/v3/account", signed=True)

    def place_limit(self, symbol, side, qty, price):
        return self._call("POST", "/api/v3/order", signed=True, params=dict(
            symbol=symbol, side=side, type="LIMIT", timeInForce="GTC",
            quantity=f"{qty:.8f}".rstrip("0").rstrip("."),
            price=f"{price:.2f}"))

    def place_stop_limit(self, symbol, side, qty, stop_price, limit_price):
        return self._call("POST", "/api/v3/order", signed=True, params=dict(
            symbol=symbol, side=side, type="STOP_LOSS_LIMIT", timeInForce="GTC",
            quantity=f"{qty:.8f}".rstrip("0").rstrip("."),
            stopPrice=f"{stop_price:.2f}", price=f"{limit_price:.2f}"))

    def place_take_profit(self, symbol, side, qty, price):
        # spot: plain GTC limit at the TP level
        return self.place_limit(symbol, side, qty, price)

    def cancel_all(self, symbol):
        return self._call("DELETE", "/api/v3/openOrders", signed=True,
                          params={"symbol": symbol})


class _BinanceFuturesRest(_BinanceRest):
    """USDT-M perpetual (fapi) order routing: LIMIT entries, reduceOnly
    STOP_MARKET SL / TAKE_PROFIT_MARKET TP, cancel-all via allOpenOrders.
    Lot precision / min notional are read from the venue's exchangeInfo."""

    def __init__(self, key, secret, base, symbol=""):
        super().__init__(key, secret, base)
        self.qty_prec = 3
        self.min_notional = 5.0
        if symbol:
            try:
                info = self._call("GET", "/fapi/v1/exchangeInfo")
                for s in info.get("symbols", []):
                    if s.get("symbol") == symbol.upper():
                        self.qty_prec = int(s.get("quantityPrecision", 3))
                        for f in s.get("filters", []):
                            if f.get("filterType") == "MIN_NOTIONAL":
                                self.min_notional = float(f.get("notional", 5))
                        break
            except Exception:
                pass   # keep defaults; orders will surface any mismatch

    def _qty(self, q):
        return f"{q:.{self.qty_prec}f}"

    def place_limit(self, symbol, side, qty, price):
        return self._call("POST", "/fapi/v1/order", signed=True, params=dict(
            symbol=symbol, side=side, type="LIMIT", timeInForce="GTC",
            quantity=self._qty(qty), price=f"{price:.2f}"))

    def place_stop_limit(self, symbol, side, qty, stop_price, limit_price):
        # futures: protective stop as a reduce-only STOP_MARKET at the level
        return self._call("POST", "/fapi/v1/order", signed=True, params=dict(
            symbol=symbol, side=side, type="STOP_MARKET", reduceOnly="true",
            quantity=self._qty(qty), stopPrice=f"{stop_price:.2f}"))

    def place_take_profit(self, symbol, side, qty, price):
        # futures: TP as a reduce-only TAKE_PROFIT_MARKET at the level
        return self._call("POST", "/fapi/v1/order", signed=True, params=dict(
            symbol=symbol, side=side, type="TAKE_PROFIT_MARKET",
            reduceOnly="true", quantity=self._qty(qty),
            stopPrice=f"{price:.2f}"))

    def cancel_all(self, symbol):
        return self._call("DELETE", "/fapi/v1/allOpenOrders", signed=True,
                          params={"symbol": symbol})

    def set_leverage(self, symbol, leverage=1):
        return self._call("POST", "/fapi/v1/leverage", signed=True,
                          params=dict(symbol=symbol, leverage=leverage))

    def balance(self):
        return self._call("GET", "/fapi/v2/balance", signed=True)


class BinanceBroker:
    """Mirrors strategy events onto Binance (spot or USDT-M futures per cfg).
    Falls back to a logged status on any error and always keeps the paper
    record as the PnL source of truth."""

    def __init__(self, cfg, base_url, venue="spot"):
        self.cfg = cfg
        self.venue = venue
        if venue == "futures":
            self.cli = _BinanceFuturesRest(cfg.api_key, cfg.api_secret,
                                           base_url, cfg.symbol)
        else:
            self.cli = _BinanceRest(cfg.api_key, cfg.api_secret, base_url)
        self.status = []
        self._mirror = None          # dict(side, qty, sl, tp, armed)
        self.paper = PaperBroker(cfg)
        if venue == "futures" and cfg.tradable:
            try:                     # keep the mirror 1x (no leverage surprises)
                self.cli.set_leverage(cfg.symbol, 1)
                self._log(f"leverage set to 1x on {cfg.symbol}")
            except Exception as ex:
                self._log(f"leverage set failed: {ex}")

    def _log(self, msg):
        d = dict(ts=time.time(), msg=str(msg))
        self.status.append(d)
        if len(self.status) > 200:
            self.status = self.status[-150:]
        return d

    def _tradable(self):
        return self.cfg.tradable

    def _arm(self):
        m = self._mirror
        if not (self._tradable() and m):
            return
        try:
            self.cli.cancel_all(self.cfg.symbol)
            close_side = "SELL" if m["side"] == "long" else "BUY"
            if m.get("sl"):
                self.cli.place_stop_limit(self.cfg.symbol, close_side, m["qty"],
                                          m["sl"], m["sl"])
            if m.get("tp"):
                self.cli.place_take_profit(self.cfg.symbol, close_side,
                                           m["qty"], m["tp"])
            m["armed"] = True
            self._log(f"protection armed: SL {m.get('sl')} TP {m.get('tp')}")
        except Exception as ex:
            m["armed"] = False
            self._log(f"arm failed: {ex}")

    def on_open(self, ev):
        q = round(self.cfg.position_usd / ev["entry"], self.cli.qty_prec)
        if self._tradable():
            min_not = getattr(self.cli, "min_notional", None)
            if min_not and q * ev["entry"] < min_not:
                self._log(f"entry skipped: qty {q} x {ev['entry']:.2f} below "
                          f"venue min notional {min_not}")
            else:
                try:
                    order = self.cli.place_limit(self.cfg.symbol,
                                                 "BUY" if ev["side"] == "long" else "SELL",
                                                 q, ev["entry"])
                    self._log(f"entry {order.get('orderId')} {ev['side']} {q} @ {ev['entry']:.2f}")
                except Exception as ex:
                    self._log(f"entry order failed: {ex}")
        else:
            self._log(f"(dry) entry {ev['side']} {q} @ {ev['entry']:.2f}")
        self._mirror = dict(side=ev["side"], qty=q, sl=ev["sl"], tp=ev["tp"], armed=False)
        if self._tradable():
            self._arm()

    def on_trail(self, ev):
        if self._mirror:
            self._mirror["sl"] = ev["sl"]
        if self._tradable():
            self._arm()
        else:
            self._log(f"(dry) trail -> {ev['a']}R SL {ev['sl']:.2f}")

    def on_close(self, ev):
        self.paper.on_close(ev)
        if self._tradable():
            try:
                self.cli.cancel_all(self.cfg.symbol)
                self._log(f"closed; orders cancelled (equity {self.paper.equity:,.2f})")
            except Exception as ex:
                self._log(f"cancel on close failed: {ex}")
        else:
            self._log(f"closed {ev['side']} {ev['exit_type']} @ {ev['exit']:.2f} "
                      f"({ev['pct']:+.3f}%) equity {self.paper.equity:,.2f}")
        self._mirror = None

    def balance(self):
        b = self.paper.balance()
        b["broker"] = self.cfg.resolved_mode
        b["mirror_armed"] = bool(self._mirror and self._mirror.get("armed"))
        return b


def make_broker(cfg):
    mode = cfg.resolved_mode
    if cfg.venue == "futures":
        if mode == "live":
            return BinanceBroker(cfg, "https://fapi.binance.com", venue="futures")
        if mode == "testnet":
            return BinanceBroker(cfg, "https://testnet.binancefuture.com",
                                 venue="futures")
        return PaperBroker(cfg)
    if mode == "live":
        return BinanceBroker(cfg, "https://api.binance.com")
    if mode == "testnet":
        return BinanceBroker(cfg, "https://testnet.binance.vision")
    return PaperBroker(cfg)
