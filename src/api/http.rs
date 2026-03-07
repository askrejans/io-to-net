use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::config::HttpApiConfig;
use crate::core::bridge_manager::BridgeManagerHandle;
use crate::core::metrics::GlobalMetrics;

#[derive(Clone)]
struct ApiState {
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<VecDeque<String>>>,
    bridge_handle: BridgeManagerHandle,
    token: Option<String>,
}

/// Start the axum HTTP admin API server.
pub async fn serve(
    cfg: HttpApiConfig,
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<VecDeque<String>>>,
    bridge_handle: BridgeManagerHandle,
) -> anyhow::Result<()> {
    let token = cfg.auth.map(|a| a.token);

    let state = ApiState { metrics, log_buffer, bridge_handle, token };

    let app = Router::new()
        .route("/health", get(health))
        .route("/bridges", get(list_bridges))
        .route("/bridges/{name}", get(get_bridge))
        .route("/bridges/{name}/restart", post(restart_bridge))
        .route("/connections", get(list_connections))
        .route("/logs", get(get_logs))
        .route("/reload-config", post(reload_config))
        .with_state(state);

    info!(listen = %cfg.listen, "HTTP admin API listening");
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ─────────────────────── Auth helper ───────────────────────

fn check_auth(state: &ApiState, headers: &HeaderMap) -> bool {
    let Some(ref expected) = state.token else { return true; };
    let auth = headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected_header = format!("Bearer {}", expected);
    // Constant-time comparison to prevent timing attacks
    subtle_eq(auth.as_bytes(), expected_header.as_bytes())
}

fn subtle_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "unauthorized"}))).into_response()
}

// ─────────────────────── Response types ───────────────────────

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    uptime_seconds: u64,
}

#[derive(Serialize)]
struct BridgeSummary {
    name: String,
    source_type: String,
    source: String,
    transport: String,
    listen: String,
    clients: usize,
    bytes_rx: u64,
    bytes_tx: u64,
    errors: u64,
    running: bool,
    protocol_hint: String,
    message_rate: f64,
    avg_frame_size: f64,
}

#[derive(Serialize)]
struct ConnectionSummary {
    client_ip: String,
    bridge: String,
    connected_seconds: u64,
    bytes_rx: u64,
    bytes_tx: u64,
}

#[derive(Serialize)]
struct LogsResponse {
    logs: Vec<String>,
}

#[derive(Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
}

// ─────────────────────── Handlers ───────────────────────

async fn health(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let uptime = state.metrics.read().uptime_seconds();
    Json(HealthResponse { status: "ok", uptime_seconds: uptime }).into_response()
}

async fn list_bridges(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let m = state.metrics.read();
    let bridges: Vec<BridgeSummary> = m.bridges.values().map(bridge_to_summary).collect();
    Json(serde_json::json!({ "bridges": bridges })).into_response()
}

async fn get_bridge(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let m = state.metrics.read();
    match m.bridges.get(&name) {
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "bridge not found"}))).into_response(),
        Some(bm) => Json(bridge_to_summary(bm)).into_response(),
    }
}

async fn restart_bridge(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let handles = state.bridge_handle.handles.lock();
    match handles.get(&name) {
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "bridge not found"}))).into_response(),
        Some(h) => {
            let _ = h.restart_tx.try_send(());
            Json(serde_json::json!({"status": "restarted"})).into_response()
        }
    }
}

async fn list_connections(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let mut m = state.metrics.write();
    // Refresh uptimes before serialising
    for cm in m.connections.values_mut() {
        cm.refresh_uptime();
    }
    let conns: Vec<ConnectionSummary> = m.connections.values().map(|c| ConnectionSummary {
        client_ip: c.client_ip.clone(),
        bridge: c.bridge_name.clone(),
        connected_seconds: c.connected_seconds,
        bytes_rx: c.bytes_rx,
        bytes_tx: c.bytes_tx,
    }).collect();
    Json(serde_json::json!({ "connections": conns })).into_response()
}

async fn get_logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    let limit = q.limit.unwrap_or(100);
    let buf = state.log_buffer.lock();
    let logs: Vec<String> = buf.iter().rev().take(limit).cloned().collect();
    Json(LogsResponse { logs }).into_response()
}

async fn reload_config(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if !check_auth(&state, &headers) { return unauthorized(); }
    // Signal via SIGHUP on Unix; on other platforms, return not-implemented
    #[cfg(unix)]
    {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;
        let _ = signal::kill(Pid::this(), Signal::SIGHUP);
        Json(serde_json::json!({"status": "reloaded"})).into_response()
    }
    #[cfg(not(unix))]
    {
        (StatusCode::NOT_IMPLEMENTED, Json(serde_json::json!({"error": "config reload not supported on this platform"}))).into_response()
    }
}

fn bridge_to_summary(bm: &crate::core::metrics::BridgeMetrics) -> BridgeSummary {
    let (source_type, source) = parse_source(&bm.source_description);
    BridgeSummary {
        name: bm.name.clone(),
        source_type,
        source,
        transport: "tcp".to_string(),
        listen: bm.transport_listen.clone(),
        clients: bm.clients_connected,
        bytes_rx: bm.bytes_rx_total,
        bytes_tx: bm.bytes_tx_total,
        errors: bm.errors,
        running: bm.running,
        protocol_hint: bm.protocol_hint.to_string(),
        message_rate: bm.message_rate,
        avg_frame_size: bm.avg_frame_size,
    }
}

fn parse_source(desc: &str) -> (String, String) {
    if let Some(rest) = desc.strip_prefix("serial://") {
        return ("serial".to_string(), rest.to_string());
    }
    if let Some(rest) = desc.strip_prefix("bluetooth://") {
        return ("bluetooth".to_string(), rest.to_string());
    }
    if let Some(rest) = desc.strip_prefix("i2c://") {
        return ("i2c".to_string(), rest.to_string());
    }
    if let Some(rest) = desc.strip_prefix("canbus://") {
        return ("canbus".to_string(), rest.to_string());
    }
    ("unknown".to_string(), desc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── subtle_eq ───────────────────────────────────────────────────────────────

    #[test]
    fn subtle_eq_identical() {
        assert!(subtle_eq(b"hello", b"hello"));
    }

    #[test]
    fn subtle_eq_different_bytes() {
        assert!(!subtle_eq(b"hello", b"world"));
    }

    #[test]
    fn subtle_eq_different_lengths() {
        assert!(!subtle_eq(b"short", b"longer"));
    }

    #[test]
    fn subtle_eq_empty() {
        assert!(subtle_eq(b"", b""));
    }

    #[test]
    fn subtle_eq_off_by_one_byte() {
        assert!(!subtle_eq(b"token_abc", b"token_abd"));
    }

    // ── check_auth ──────────────────────────────────────────────────────────────

    fn make_state(token: Option<&str>) -> ApiState {
        let metrics = Arc::new(RwLock::new(crate::core::metrics::GlobalMetrics::new()));
        let log_buffer = Arc::new(parking_lot::Mutex::new(VecDeque::new()));
        let handles = Arc::new(parking_lot::Mutex::new(std::collections::HashMap::new()));
        ApiState {
            metrics,
            log_buffer,
            bridge_handle: crate::core::bridge_manager::BridgeManagerHandle { handles },
            token: token.map(|s| s.to_string()),
        }
    }

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            "Authorization",
            format!("Bearer {}", token).parse().unwrap(),
        );
        h
    }

    #[test]
    fn no_token_configured_always_allows() {
        let state = make_state(None);
        assert!(check_auth(&state, &HeaderMap::new()));
    }

    #[test]
    fn correct_bearer_token_allowed() {
        let state = make_state(Some("mysecret"));
        let headers = headers_with_bearer("mysecret");
        assert!(check_auth(&state, &headers));
    }

    #[test]
    fn wrong_bearer_token_rejected() {
        let state = make_state(Some("mysecret"));
        let headers = headers_with_bearer("wrongtoken");
        assert!(!check_auth(&state, &headers));
    }

    #[test]
    fn missing_authorization_header_rejected_when_token_required() {
        let state = make_state(Some("mysecret"));
        assert!(!check_auth(&state, &HeaderMap::new()));
    }

    #[test]
    fn non_bearer_scheme_rejected() {
        let state = make_state(Some("mysecret"));
        let mut h = HeaderMap::new();
        h.insert("Authorization", "Basic mysecret".parse().unwrap());
        assert!(!check_auth(&state, &h));
    }

    // ── parse_source ────────────────────────────────────────────────────────────

    #[test]
    fn parse_source_serial() {
        let (t, v) = parse_source("serial:///dev/ttyUSB0");
        assert_eq!(t, "serial");
        assert_eq!(v, "/dev/ttyUSB0");
    }

    #[test]
    fn parse_source_bluetooth() {
        let (t, v) = parse_source("bluetooth://AA:BB:CC:DD:EE:FF");
        assert_eq!(t, "bluetooth");
        assert_eq!(v, "AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn parse_source_i2c() {
        let (t, v) = parse_source("i2c:///dev/i2c-1");
        assert_eq!(t, "i2c");
        assert_eq!(v, "/dev/i2c-1");
    }

    #[test]
    fn parse_source_canbus() {
        let (t, v) = parse_source("canbus://can0");
        assert_eq!(t, "canbus");
        assert_eq!(v, "can0");
    }

    #[test]
    fn parse_source_unknown() {
        let (t, v) = parse_source("pty://(not opened)");
        assert_eq!(t, "unknown");
        assert_eq!(v, "pty://(not opened)");
    }
}
