#!/usr/bin/env python3
"""FRVP PoC live trader — run loop.

    python3 -m live.run                      # paper mode on live PAXGUSDT data
    python3 -m live.run --source xau         # paper mode on XAUUSD replay feed
    MODE=testnet BINANCE_TESTNET=1 BINANCE_DRY_RUN=0 BINANCE_API_KEY=.. \
        BINANCE_API_SECRET=.. python3 -m live.run     # Binance testnet (from your machine)
    MODE=live BINANCE_LIVE_ACK=yes BINANCE_DRY_RUN=0 BINANCE_API_KEY=.. \
        BINANCE_API_SECRET=.. python3 -m live.run     # live spot (use with care)

Dashboard: http://localhost:PORT/   (SSE event stream + JSON endpoints)
"""
import os, sys, time, json, threading

HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))

from config import load_config, print_banner
from strategy import Strategy
from datafeed import BinanceFeed, BinanceWSFeed, HAVE_WS, ReplayFeed
from broker import make_broker
import server


def main():
    cfg = load_config(sys.argv[1:])
    print_banner(cfg)

    if cfg.data_source == "xau":
        csvp = cfg.xau_csv or os.path.join(os.path.dirname(HERE), "cache", "XAUUSD_15m.csv")
        if not os.path.exists(csvp):
            print(f"XAU csv not found at {csvp}. Generate it (fetch_duka.py) or use default PAXG source.")
            sys.exit(1)
        feed = ReplayFeed(csvp, speed=cfg.replay_speed, start=cfg.replay_start)
        mode_label = f"XAUUSD replay ({cfg.replay_speed:.1f}x, {feed.remaining()} bars left)"
    else:
        use_ws = cfg.feed == "ws" and HAVE_WS
        if use_ws:
            feed = BinanceWSFeed(symbol=cfg.symbol)
            feed.bootstrap()
            feed.start()
            mode_label = f"{cfg.symbol} live via Binance WS stream (15m, push)"
        else:
            feed = BinanceFeed(symbol=cfg.symbol)
            feed.bootstrap()
            mode_label = f"{cfg.symbol} live via Binance data-api (15m, poll)"
            if cfg.feed == "ws":
                print("[!] websocket-client not installed — FEED=rest fallback "
                      "(pip install websocket-client for push)", flush=True)

    broker = make_broker(cfg)

    def emit(ev):
        server.hub.publish(ev["type"], ev)
        if ev["type"] == "open":
            broker.on_open(ev)
        elif ev["type"] == "close":
            broker.on_close(ev)
        elif ev["type"] == "trail":
            broker.on_trail(ev)

    strat = Strategy(cfg, emit=emit)
    if cfg.data_source == "paxg":
        strat.feed_history(feed.bars, quiet=True)     # ~10 days closed bars
    else:
        strat.feed_history(feed.warm_bars(), quiet=True)
        feed.start_pacing()
    # paper ledger should reflect the warmed trade history too
    pp = getattr(broker, "paper", broker)
    for t in strat.trades:
        pp.apply_trade(t)

    srv = server.serve(cfg)
    print(f"[*] dashboard -> http://{cfg.host}:{cfg.port}/   ({mode_label})")
    print("[*] ctrl-c to stop. All fills are paper unless MODE=testnet/live & dry-run=0.\n")

    last_state_ts = 0
    try:
        while True:
            time.sleep(max(0.5, min(cfg.poll_s, 10)))
            new_bars, last_price = feed.poll()
            for b in new_bars:
                for ev in strat.on_bar(b):
                    pass        # already routed via emit
            # periodic full snapshot ~ every poll
            st = strat.state()
            st["mode"] = cfg.display_mode
            st["symbol"] = cfg.symbol
            st["source"] = mode_label
            st["last_price"] = last_price if last_price is not None else (
                st.get("last_bar", {}).get("c"))
            st["balance"] = broker.balance()
            pp = getattr(broker, "paper", broker)   # paper ledger (any broker)
            st["equity_curve"] = pp.equity_curve[-400:]
            st["recent_bars"] = [dict(ts=b.ts, o=b.o, h=b.h, l=b.l, c=b.c)
                                 for b in strat.bars[-240:]]
            st["last_trades"] = strat.trades[-40:]
            st["broker_status"] = getattr(broker, "status", [])[-20:]
            st["config"] = dict(rr=cfg.rr, skip_hour0=cfg.skip_hour0,
                                trail_on=cfg.trail_on, ladder=cfg.trail_ladder,
                                symbol=cfg.symbol, source=cfg.data_source)
            server.hub.publish("state", st)
    except KeyboardInterrupt:
        print("\n[*] stopping...")
        if strat.pos is not None:
            print("    force-flat (paper):", strat.force_flat())
    finally:
        srv.shutdown()


if __name__ == "__main__":
    main()
