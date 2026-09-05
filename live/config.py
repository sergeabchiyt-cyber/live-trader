"""Configuration for the live FRVP PoC trader — all settings come from ENV or CLI.

Execution mode resolution (order of precedence):
  1. MODE env / --mode : paper | testnet | live
  2. auto:
     - BINANCE_API_KEY & BINANCE_API_SECRET set AND BINANCE_LIVE_ACK == yes  -> live
     - BINANCE_TESTNET=1 (and optionally BINANCE_TESTNET_API_KEY/SECRET)      -> testnet
     - otherwise                                                             -> paper

Market data always comes from the public data-api endpoint (no keys) unless
DATA_SOURCE=xau (replay of the backtest XAUUSD cache for gold demos).
"""
import os, sys
from dataclasses import dataclass, field
from typing import Optional, List

DEFAULT_TRAIL = [(0.6, 0.1), (0.75, 0.4), (0.9, 0.7)]


def _env_bool(name, default=False):
    v = os.environ.get(name)
    if v is None:
        return default
    return v.strip().lower() in ("1", "true", "yes", "on")


def _parse_ladder(s):
    """'0.6:0.1,0.75:0.4,0.9:0.7' -> [(0.6,0.1),...] sorted desc by progress"""
    out = []
    for tok in s.split(","):
        tok = tok.strip()
        if not tok:
            continue
        a, b = tok.split(":")
        out.append((float(a), float(b)))
    out.sort(key=lambda x: x[0])
    return out


@dataclass
class Config:
    mode: str = "auto"                # auto|paper|testnet|live
    venue: str = "spot"               # spot (data-api.binance.vision, SPOT orders) |
                                      # futures (fapi/fstream.binance.com, USDT-M perp orders)
    symbol: str = "PAXGUSDT"          # Binance symbol (PAXG = tokenized gold; XAUUSDT = gold TRADFI perp, futures only)
    data_source: str = "paxg"         # paxg (live Binance public) | xau (replay cache)
    feed: str = "ws"                  # ws (WebSocket push) | rest (REST polling)
    rr: float = 2.0
    skip_hour0: bool = True           # no entries triggered 00:00-01:00 UTC
    trail_on: bool = True
    trail_ladder: List[tuple] = field(default_factory=lambda: list(DEFAULT_TRAIL))
    sl_min: float = 0.0005
    sl_max: float = 0.005
    atr_n: int = 14
    bins: int = 48
    day_open_hour: int = 22
    session_days: tuple = (6, 0, 1, 2, 3)
    position_usd: float = 100.0       # paper/mirror size in quote (USDT)
    dry_run: bool = True              # paper even in live/testnet unless disabled
    same_bar_exits: bool = True      # engine parity: exits may trigger on the entry bar itself
                                     # (False = live-realistic: manage from the next closed bar)
    poll_s: float = 5.0
    port: int = 8765
    host: str = "0.0.0.0"
    xau_csv: str = ""
    replay_speed: float = 1.0         # xau replay multiplier vs wall clock
    replay_start: Optional[str] = None
    paper_start_usdt: float = 10000.0
    paper_commission_pct: float = 0.0  # per-trade round-trip cost in % (0.02 typical w/ spread)
    log_file: str = ""
    api_key: str = ""
    api_secret: str = ""
    testnet: bool = False
    live_ack: bool = False
    exec_style: str = "level"         # level | close | market (paper fill convention)
    verbose: bool = True

    @property
    def resolved_mode(self) -> str:
        m = self.mode.lower()
        if m in ("paper", "testnet", "live"):
            return m
        # auto
        if self.api_key and self.api_secret and self.live_ack:
            return "live"
        if self.testnet:
            return "testnet"
        return "paper"

    @property
    def display_mode(self) -> str:
        m = self.resolved_mode
        if self.dry_run and m in ("live", "testnet"):
            return f"{m} (dry-run)"
        return m

    @property
    def tradable(self) -> bool:
        """Orders actually reach a real exchange."""
        return self.resolved_mode in ("live", "testnet") and not self.dry_run


def load_config(argv=None) -> Config:
    e = os.environ
    c = Config(
        mode=e.get("MODE", "auto"),
        venue=e.get("VENUE", "spot").lower(),
        symbol=e.get("SYMBOL", "PAXGUSDT").upper(),
        data_source=e.get("DATA_SOURCE", "paxg").lower(),
        feed=e.get("FEED", "ws").lower(),
        rr=float(e.get("RR", 2.0)),
        skip_hour0=_env_bool("SKIP_HOUR0", True),
        trail_on=_env_bool("TRAIL", True),
        trail_ladder=_parse_ladder(e.get("TRAIL_LADDER", "")),
        position_usd=float(e.get("POSITION_USD", 100.0)),
        dry_run=_env_bool("BINANCE_DRY_RUN", True),
        same_bar_exits=_env_bool("SAME_BAR", True),
        poll_s=float(e.get("POLL_S", 5.0)),
        port=int(e.get("PORT", 8765)),
        xau_csv=e.get("XAU_CSV", ""),
        replay_speed=float(e.get("REPLAY_SPEED", 1.0)),
        replay_start=e.get("REPLAY_START", None),
        paper_start_usdt=float(e.get("PAPER_START_USDT", 10000.0)),
        paper_commission_pct=float(e.get("PAPER_COMMISSION_PCT", 0.0)),
        log_file=e.get("LOG_FILE", ""),
        api_key=e.get("BINANCE_API_KEY", ""),
        api_secret=e.get("BINANCE_API_SECRET", ""),
        testnet=_env_bool("BINANCE_TESTNET", False) or bool(
            e.get("BINANCE_TESTNET_API_KEY") or e.get("BINANCE_TESTNET_API_SECRET")),
        live_ack=(e.get("BINANCE_LIVE_ACK", "").lower() == "yes"),
        exec_style=e.get("EXEC_STYLE", "level"),
        verbose=_env_bool("VERBOSE", True),
    )
    if not c.trail_ladder:
        c.trail_ladder = list(DEFAULT_TRAIL)
    if e.get("BINANCE_TESTNET_API_KEY"):
        c.api_key = e["BINANCE_TESTNET_API_KEY"]
    if e.get("BINANCE_TESTNET_API_SECRET"):
        c.api_secret = e["BINANCE_TESTNET_API_SECRET"]
    # CLI overrides: --mode --symbol --rr --port --skip-hour0 0/1 --trail 0/1
    if argv:
        a = argv
        for i, x in enumerate(a):
            if x == "--mode" and i + 1 < len(a):
                c.mode = a[i + 1]
            if x == "--symbol" and i + 1 < len(a):
                c.symbol = a[i + 1].upper()
            if x == "--rr" and i + 1 < len(a):
                c.rr = float(a[i + 1])
            if x == "--port" and i + 1 < len(a):
                c.port = int(a[i + 1])
            if x == "--skip-hour0" and i + 1 < len(a):
                c.skip_hour0 = a[i + 1] in ("1", "true")
            if x == "--trail" and i + 1 < len(a):
                c.trail_on = a[i + 1] in ("1", "true")
            if x == "--source" and i + 1 < len(a):
                c.data_source = a[i + 1].lower()
            if x == "--replay-start" and i + 1 < len(a):
                c.replay_start = a[i + 1]
            if x == "--replay-speed" and i + 1 < len(a):
                c.replay_speed = float(a[i + 1])
            if x == "--exec-style" and i + 1 < len(a):
                c.exec_style = a[i + 1]
    return c


def print_banner(c: Config):
    mode = c.resolved_mode
    icon = {"paper": "\U0001f9fe", "testnet": "\U0001f6e1", "live": "\U0001f534"}[mode]
    print("=" * 68)
    print(f"  {icon} FRVP PoC live trader   mode = {mode.upper()}"
          f"{'  (dry-run, no real orders)' if c.dry_run and mode != 'paper' else ''}")
    src = "REPLAY-XAU" if c.data_source == "xau" else "LIVE-BINANCE"
    print(f"     data      : {src}  symbol={c.symbol}  venue={c.venue}"
          + (f"  csv={c.xau_csv}" if c.data_source == "xau" else ""))
    if c.data_source == "paxg":
        md = ("Binance spot public data (data-api/data-stream.binance.vision)"
              if c.venue == "spot" else
              "Binance USDT-M perp public data (fapi/fstream.binance.com)")
        print(f"     market    : {md} (no keys)")
    else:
        print(f"     market    : XAUUSD replay cache")
    print(f"     settings  : RR={c.rr:g}  skip-00:01={c.skip_hour0}  trail={c.trail_on}"
          + (f" {[(p, a) for p, a in c.trail_ladder]}" if c.trail_on else ""))
    print(f"     position  : {c.position_usd:g} USDT notional   poll={c.poll_s}s   "
          f"http://{c.host}:{c.port}")
    if mode == "live":
        print("     \u26a0 LIVE REAL MONEY — ack ok, dry-run=" + str(c.dry_run)
              + ("   [USDT-M perp — leveraged!]" if c.venue == "futures" else ""))
    if mode == "testnet":
        tgt = ("testnet.binancefuture.com (USDT-M perp)" if c.venue == "futures"
               else "testnet.binance.vision (spot)")
        print(f"     orders -> {tgt}   dry-run=" + str(c.dry_run))
    if mode == "paper":
        print("     paper fills (no exchange orders). Set MODE/BINANCE_* env to go live.")
    print("=" * 68)
