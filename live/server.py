"""Tiny HTTP server: serves the dashboard (inline HTML) + JSON/SSE endpoints.
SSE pushes strategy events; a lightweight emitter avoids dependencies."""
import json, time, queue, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs

try:
    from .dashboard_html import DASHBOARD_HTML as DASH
except ImportError:
    try:
        from live.dashboard_html import DASHBOARD_HTML as DASH
    except ImportError:
        DASH = None


class Hub:
    def __init__(self):
        self.subs = []
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

    def subscribe(self, q):
        with self.lock:
            self.subs.append(q)

    def unsubscribe(self, q):
        with self.lock:
            if q in self.subs:
                self.subs.remove(q)


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
            self._send(404, "text/plain", b"not found")

    return H


def serve(cfg):
    srv = ThreadingHTTPServer((cfg.host, cfg.port), make_handler())
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()
    return srv
