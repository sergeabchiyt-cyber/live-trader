//! Embedded dashboard — the single-file terminal UI (no CDN, fully
//! self-contained). Source of truth: rust/dashboard/dashboard.html.

pub const DASHBOARD_HTML: &str = include_str!("../dashboard/dashboard.html");
