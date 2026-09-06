//! Hub — fan-out of state snapshots, strategy events and ticks to SSE queues
//! and WebSocket clients. Dead subscribers are pruned on send failure (the
//! bounded queues make slow/dead clients drop out instead of blocking).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::Mutex;

/// Messages pushed to a WebSocket writer thread.
#[derive(Debug)]
#[allow(dead_code)] // Ping/Pong/Close are part of the writer protocol
pub enum FrameOut {
    Text(String),
    Ping,
    Pong(Vec<u8>),
    Close(Vec<u8>),
}

struct Inner {
    next_id: u64,
    sse: Vec<(u64, SyncSender<String>)>,
    ws: Vec<(u64, SyncSender<FrameOut>)>,
    snapshot: HashMap<String, Value>,
}

pub struct Hub {
    inner: Mutex<Inner>,
    /// cached serialization of the latest `state` data (served by /api/state
    /// without re-serializing)
    state_str: Mutex<Option<String>>,
}

impl Hub {
    pub fn new() -> Hub {
        Hub {
            inner: Mutex::new(Inner {
                next_id: 0,
                sse: Vec::new(),
                ws: Vec::new(),
                snapshot: HashMap::new(),
            }),
            state_str: Mutex::new(None),
        }
    }

    pub fn publish(&self, kind: &str, data: Value) {
        let envelope = json!({"type": kind, "ts": crate::engine::now_ts_f64(), "data": data});
        let msg = serde_json::to_string(&envelope).unwrap_or_default();
        if kind == "state" {
            if let Ok(mut c) = self.state_str.lock() {
                *c = Some(serde_json::to_string(&envelope["data"]).unwrap_or_default());
            }
        }
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.snapshot.insert(kind.to_string(), envelope["data"].clone());
        let mut dead: Vec<u64> = Vec::new();
        for (id, tx) in &inner.sse {
            if tx.try_send(format!("data: {msg}\n\n")).is_err() {
                dead.push(*id);
            }
        }
        for (id, tx) in &inner.ws {
            if tx.try_send(FrameOut::Text(msg.clone())).is_err() {
                dead.push(*id);
            }
        }
        if !dead.is_empty() {
            inner.sse.retain(|(id, _)| !dead.contains(id));
            inner.ws.retain(|(id, _)| !dead.contains(id));
        }
    }

    pub fn subscribe_sse(&self) -> (u64, Receiver<String>) {
        let (tx, rx) = sync_channel::<String>(200);
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_id += 1;
        let id = inner.next_id;
        inner.sse.push((id, tx));
        (id, rx)
    }

    pub fn subscribe_ws(&self) -> (u64, Receiver<FrameOut>) {
        let (tx, rx) = sync_channel::<FrameOut>(400);
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.next_id += 1;
        let id = inner.next_id;
        inner.ws.push((id, tx));
        (id, rx)
    }

    pub fn unsubscribe(&self, id: u64) {
        let mut inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        inner.sse.retain(|(i, _)| *i != id);
        inner.ws.retain(|(i, _)| *i != id);
    }

    pub fn state_str(&self) -> Option<String> {
        self.state_str.lock().ok().and_then(|g| g.clone())
    }

    pub fn snapshot_json(&self) -> String {
        let inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        serde_json::to_string(&inner.snapshot).unwrap_or_else(|_| "{}".into())
    }
}
