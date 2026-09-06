"""Tiny HTTP server: dashboard (inline HTML) + JSON/SSE/WebSocket endpoints.

/api/ws    — RFC 6455 WebSocket push (stdlib only): one connection carries
             state snapshots, strategy events and ~1s price ticks. Same
             {type, ts, data} JSON message shape as the SSE stream.
/api/events — SSE push (kept for compatibility / simple clients).
/api/state  — REST snapshot (fallback + initial load).

A lightweight emitter avoids third-party server dependencies: the WS layer is
~70 lines of framing (sha1+base64 handshake, text frames server->client,
ping/pong + close handling server-side).
"""
import json, time, queue, threading, base64, hashlib, struct
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

try:
    from .dashboard_html import DASHBOARD_HTML as DASH
except ImportError:
    try:
        from live.dashboard_html import DASHBOARD_HTML as DASH
    except ImportError:
        DASH = None

WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


class _WSClient:
    """One accepted WebSocket connection (server->client text push; the client
    side only needs ping/pong/close handling, so the read loop is minimal)."""

    def __init__(self, sock, wfile):
        self.sock = sock
        self.wfile = wfile
        self.lock = threading.Lock()

    def _frame(self, opcode, payload: bytes):
        n = len(payload)
        if n < 126:
            hdr = struct.pack("!BB", 0x80 | opcode, n)
        elif n < 65536:
            hdr = struct.pack("!BBH", 0x80 | opcode, 126, n)
        else:
            hdr = struct.pack("!BBQ", 0x80 | opcode, 127, n)
        with self.lock:
            self.wfile.write(hdr + payload)
            self.wfile.flush()

    def send_text(self, msg: str):
        self._frame(0x1, msg.encode("utf-8"))

    def read_loop(self, rfile):
        """Block on client frames: answer pings, echo close, ignore the rest."""
        try:
            self.sock.settimeout(600)                  # drop half-dead peers
            while True:
                hdr = rfile.read(2)
                if not hdr or len(hdr) < 2:
                    break
                b1, b2 = hdr[0], hdr[1]
                opcode = b1 & 0x0F
                masked, ln = b2 & 0x80, b2 & 0x7F
                if ln == 126:
                    ln = struct.unpack("!H", rfile.read(2))[0]
                elif ln == 127:
                    ln = struct.unpack("!Q", rfile.read(8))[0]
                mask = rfile.read(4) if masked else None
                payload = rfile.read(ln) if ln else b""
                if mask and len(mask) == 4:
                    payload = bytes(c ^ mask[i % 4] for i, c in enumerate(payload))
                if opcode == 0x8:                      # close
                    try:
                        self._frame(0x8, payload[:2])
                    except Exception:
                        pass
                    break
                if opcode == 0x9:                      # ping -> pong
                    self._frame(0xA, payload)
        except Exception:
            pass


class Hub:
    def __init__(self):
        self.subs = []          # SSE queues
        self.ws = set()         # WebSocket clients
        self.lock = threading.Lock()
        self.snapshot = {}

    def publish(self, kind, data):
        ev = dict(type=kind, ts=time.time(), data=data)
        self.snapshot[kind] = data
        with self.lock:
            dead = []
            for q in self.subs:
                try:
                    q.put(ev, block=False)
                except queue.Full:
                    dead.append(q)
            for q in dead:
                self.subs.remove(q)
        msg = json.dumps(ev, default=str)
        with self.lock:
            dead = []
            for c in self.ws:
                try:
                    c.send_text(msg)
                except Exception:
                    dead.append(c)
            for c in dead:
                self.ws.discard(c)

    def subscribe(self, q):
        with self.lock:
            self.subs.append(q)

    def unsubscribe(self, q):
        with self.lock:
            if q in self.subs:
                self.subs.remove(q)

    def add_ws(self, client):
        with self.lock:
            self.ws.add(client)

    def remove_ws(self, client):
        with self.lock:
            self.ws.discard(client)


hub = Hub()


def make_handler():
    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):
            pass

        def _send(self, code, ctype, body: bytes):
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            u = urlparse(self.path)
            p = u.path
            if p in ("/", "/index.html", "/dashboard"):
                if DASH:
                    self._send(200, "text/html; charset=utf-8", DASH.encode())
                else:
                    self._send(503, "text/plain", b"dashboard not ready")
                return
            if p == "/api/state":
                s = hub.snapshot.get("state")
                if s is not None:
                    self._send(200, "application/json", json.dumps(s).encode())
                else:
                    self._send(200, "application/json", b"{}")
                return
            if p == "/api/snapshot":
                body = json.dumps(hub.snapshot, default=str).encode()
                self._send(200, "application/json", body)
                return
            if p == "/api/events":
                q = queue.Queue(maxsize=200)
                hub.subscribe(q)
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-store")
                self.send_header("Connection", "keep-alive")
                self.end_headers()
                try:
                    while True:
                        try:
                            ev = q.get(timeout=8)
                        except queue.Empty:
                            self.wfile.write(b": keepalive\n\n")
                            self.wfile.flush()
                            continue
                        try:
                            self.wfile.write(f"data: {json.dumps(ev, default=str)}\n\n".encode())
                            self.wfile.flush()
                        except Exception:
                            break
                except Exception:
                    pass
                finally:
                    hub.unsubscribe(q)
                return
            if p == "/api/ws":
                key = self.headers.get("Sec-WebSocket-Key")
                if not key or "websocket" not in self.headers.get("Upgrade", "").lower():
                    self._send(400, "text/plain", b"expected websocket upgrade")
                    return
                accept = base64.b64encode(
                    hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
                self.send_response(101, "Switching Protocols")
                self.send_header("Upgrade", "websocket")
                self.send_header("Connection", "Upgrade")
                self.send_header("Sec-WebSocket-Accept", accept)
                self.end_headers()
                client = _WSClient(self.connection, self.wfile)
                hub.add_ws(client)
                try:
                    client.read_loop(self.rfile)        # blocks until close
                finally:
                    hub.remove_ws(client)
                return
            self._send(404, "text/plain", b"not found")

    return H


def serve(cfg):
    srv = ThreadingHTTPServer((cfg.host, cfg.port), make_handler())
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    return srv
