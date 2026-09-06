//! Threaded HTTP server: dashboard (embedded HTML), JSON endpoints, SSE push
//! and the WebSocket upgrade — all std-only, one thread per connection with
//! keep-alive. Endpoints:
//!   GET /            dashboard
//!   GET /api/state   latest strategy state snapshot (cached serialization)
//!   GET /api/snapshot all hub snapshots (state/tick/last events)
//!   GET /api/events  SSE stream (legacy compatibility)
//!   GET /api/ws      RFC 6455 WebSocket push (state + events + ~1s ticks)
//!   GET /api/health  liveness + feed health + uptime (for uptime pingers)
//!   GET /api/ping    "pong"

use crate::config::Config;
use crate::dashboard::DASHBOARD_HTML;
use crate::hub::{FrameOut, Hub};
use crate::ws;
use serde_json::json;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const ENGINE_VERSION: &str = "rust-2.0.0";

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

pub fn serve(cfg: &Config, hub: Arc<Hub>) -> std::io::Result<()> {
    let _ = START.set(Instant::now());
    let addr = format!("{}:{}", cfg.host, cfg.port);
    let listener = TcpListener::bind(&addr)?;
    println!("[http] listening on http://{}", addr);
    let hub_c = hub.clone();
    std::thread::Builder::new()
        .name("http-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        if ACTIVE.load(Ordering::Relaxed) > 200 {
                            let mut s = s;
                            let _ = s.write_all(
                                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                            continue;
                        }
                        let hub = hub_c.clone();
                        ACTIVE.fetch_add(1, Ordering::Relaxed);
                        let _ = std::thread::Builder::new()
                            .name("conn".into())
                            .spawn(move || {
                                handle_conn(s, hub);
                                ACTIVE.fetch_sub(1, Ordering::Relaxed);
                            });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        })?;
    Ok(())
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>, // lowercase keys
    keep_alive: bool,
}

fn read_line(r: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    let n = r.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    if line.len() > 16_384 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "header too large"));
    }
    while line.ends_with('\n') || line.ends_with('\r') {
        line.pop();
    }
    Ok(Some(line))
}

fn parse_request(r: &mut impl BufRead) -> std::io::Result<Option<Request>> {
    let first = match read_line(r)? {
        Some(l) if !l.is_empty() => l,
        _ => return Ok(None),
    };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_uppercase();
    let target = parts.next().unwrap_or("/").to_string();
    let version = parts.next().unwrap_or("HTTP/1.1").to_string();
    if method.is_empty() {
        return Ok(None);
    }
    let mut headers = HashMap::new();
    loop {
        let line = match read_line(r)? {
            Some(l) => l,
            None => return Ok(None),
        };
        if line.is_empty() {
            break;
        }
        if let Some(idx) = line.find(':') {
            let k = line[..idx].trim().to_lowercase();
            let v = line[idx + 1..].trim().to_string();
            headers.insert(k, v);
        }
    }
    // drain any body (we accept none)
    if let Some(cl) = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()) {
        if cl > 1_048_576 {
            return Ok(None);
        }
        let mut sink = vec![0u8; cl];
        r.read_exact(&mut sink)?;
    }
    let path = target.split('?').next().unwrap_or("/").to_string();
    let conn = headers
        .get("connection")
        .map(|v| v.to_lowercase())
        .unwrap_or_default();
    let keep_alive = if version == "HTTP/1.0" {
        conn.contains("keep-alive")
    } else {
        !conn.contains("close")
    };
    Ok(Some(Request { method, path, headers, keep_alive }))
}

fn respond(
    w: &mut impl Write,
    status: u16,
    reason: &str,
    ctype: &str,
    body: &[u8],
    head_only: bool,
) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\r\n",
        body.len()
    );
    w.write_all(resp.as_bytes())?;
    if !head_only {
        w.write_all(body)?;
    }
    w.flush()
}

fn handle_conn(stream: TcpStream, hub: Arc<Hub>) {
    let _ = stream.set_nodelay(true);
    let mut reader = match stream.try_clone() {
        Ok(r) => BufReader::new(r),
        Err(_) => return,
    };
    let mut wstream = stream;
    loop {
        let _ = wstream.set_read_timeout(Some(Duration::from_secs(65)));
        let req = match parse_request(&mut reader) {
            Ok(Some(r)) => r,
            _ => return,
        };
        let head_only = req.method == "HEAD";
        let get = req.method == "GET" || head_only;
        let path = req.path.as_str();

        // WebSocket upgrade
        if path == "/api/ws" && get {
            let key = req.headers.get("sec-websocket-key").cloned();
            let upgrade = req
                .headers
                .get("upgrade")
                .map(|v| v.to_lowercase().contains("websocket"))
                .unwrap_or(false);
            match (key, upgrade) {
                (Some(key), true) => {
                    let rstream = match wstream.try_clone() {
                        Ok(s) => s,
                        Err(_) => return,
                    };
                    serve_ws(key, rstream, reader, hub);
                    return; // connection consumed
                }
                _ => {
                    let _ = respond(&mut wstream, 400, "Bad Request", "text/plain", b"expected websocket upgrade", false);
                    return;
                }
            }
        }

        if !get {
            let _ = respond(&mut wstream, 405, "Method Not Allowed", "text/plain", b"method not allowed", false);
            if !req.keep_alive {
                return;
            }
            continue;
        }

        match path {
            "/" | "/index.html" | "/dashboard" => {
                let _ = respond(&mut wstream, 200, "OK", "text/html; charset=utf-8", DASHBOARD_HTML.as_bytes(), head_only);
            }
            "/api/state" => {
                let body = hub.state_str().unwrap_or_else(|| "{}".into());
                let _ = respond(&mut wstream, 200, "OK", "application/json", body.as_bytes(), head_only);
            }
            "/api/snapshot" => {
                let body = hub.snapshot_json();
                let _ = respond(&mut wstream, 200, "OK", "application/json", body.as_bytes(), head_only);
            }
            "/api/health" => {
                let uptime = START.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
                let body = json!({
                    "status": "ok",
                    "engine": ENGINE_VERSION,
                    "uptime_s": uptime,
                })
                .to_string();
                let _ = respond(&mut wstream, 200, "OK", "application/json", body.as_bytes(), head_only);
            }
            "/api/ping" => {
                let _ = respond(&mut wstream, 200, "OK", "text/plain", b"pong", head_only);
            }
            "/api/events" => {
                serve_sse(&mut wstream, &hub);
                return; // SSE holds the connection until it breaks
            }
            "/favicon.ico" => {
                let _ = respond(&mut wstream, 204, "No Content", "image/x-icon", b"", true);
            }
            _ => {
                let _ = respond(&mut wstream, 404, "Not Found", "text/plain", b"not found", false);
            }
        }
        if !req.keep_alive {
            return;
        }
    }
}

fn serve_sse(w: &mut TcpStream, hub: &Arc<Hub>) {
    use std::sync::mpsc::RecvTimeoutError;
    let (id, rx) = hub.subscribe_sse();
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nConnection: keep-alive\r\n\r\n";
    if w.write_all(head.as_bytes()).and_then(|_| w.flush()).is_err() {
        hub.unsubscribe(id);
        return;
    }
    let _ = w.set_read_timeout(None);
    loop {
        match rx.recv_timeout(Duration::from_secs(8)) {
            Ok(data) => {
                if w.write_all(data.as_bytes()).and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if w.write_all(b": keepalive\n\n").and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    hub.unsubscribe(id);
}

fn serve_ws(key: String, rstream: TcpStream, reader: BufReader<TcpStream>, hub: Arc<Hub>) {
    use std::sync::mpsc::RecvTimeoutError;
    let accept = ws::accept_key(&key);
    let resp = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
    );
    let wshared = match rstream.try_clone() {
        Ok(s) => Arc::new(Mutex::new(s)),
        Err(_) => return,
    };
    {
        let mut w = match wshared.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if w.write_all(resp.as_bytes()).and_then(|_| w.flush()).is_err() {
            return;
        }
    }
    let (id, rx) = hub.subscribe_ws();
    // ---- writer thread: hub frames out, 30s keepalive ping ----
    let wshared_w = wshared.clone();
    let writer = std::thread::Builder::new()
        .name("ws-writer".into())
        .spawn(move || {
            loop {
                match rx.recv_timeout(Duration::from_secs(30)) {
                    Ok(FrameOut::Text(s)) => {
                        let mut w = match wshared_w.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        if ws::write_frame(&mut *w, 0x1, s.as_bytes()).is_err() {
                            break;
                        }
                    }
                    Ok(FrameOut::Ping) => {
                        let mut w = match wshared_w.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        if ws::write_frame(&mut *w, 0x9, b"").is_err() {
                            break;
                        }
                    }
                    Ok(FrameOut::Pong(p)) => {
                        let mut w = match wshared_w.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        if ws::write_frame(&mut *w, 0xA, &p).is_err() {
                            break;
                        }
                    }
                    Ok(FrameOut::Close(p)) => {
                        let mut w = match wshared_w.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        let _ = ws::write_frame(&mut *w, 0x8, &p);
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        let mut w = match wshared_w.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        if ws::write_frame(&mut *w, 0x9, b"keepalive").is_err() {
                            break;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    if writer.is_err() {
        hub.unsubscribe(id);
        return;
    }
    // ---- reader loop (this thread): answer pings, echo close ----
    let _ = rstream.set_read_timeout(Some(Duration::from_secs(600)));
    let mut r = reader;
    loop {
        match ws::read_frame(&mut r) {
            Ok(Some(ws::ClientFrame::Ping(p))) => {
                let mut w = match wshared.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                if ws::write_frame(&mut *w, 0xA, &p).is_err() {
                    break;
                }
            }
            Ok(Some(ws::ClientFrame::Close(p))) => {
                let mut w = match wshared.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                let _ = ws::write_frame(&mut *w, 0x8, &p);
                break;
            }
            Ok(Some(_)) => {} // client->server text/binary ignored
            Ok(None) => break,
            Err(_) => break,
        }
    }
    hub.unsubscribe(id); // drops the hub sender -> writer exits promptly
}
