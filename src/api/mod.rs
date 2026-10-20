//! HTTP API: `/health`, `/metrics`, `/api/v1/*` (bearer token when set),
//! a server-sent event stream and the embedded web dashboard.
//!
//! State-changing requests (restart, reload) must carry
//! `X-Requested-With`, which a cross-site form or image cannot send without
//! a CORS preflight, so a web page cannot poke the loopback API.

pub mod metrics;
pub mod web;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::config::HttpApiConfig;
use crate::core::manager::Control;
use crate::core::stats::{Direction, Registry, Status};

#[derive(Clone)]
struct Ctx {
    reg: Arc<Registry>,
    control: Option<Control>,
    token: Option<Arc<str>>,
}

pub struct Options {
    pub token: Option<String>,
    pub ui: bool,
    pub cors_origin: Option<String>,
}

impl From<&HttpApiConfig> for Options {
    fn from(c: &HttpApiConfig) -> Self {
        Options {
            token: c.auth.as_ref().map(|a| a.token.clone()),
            ui: c.ui,
            cors_origin: c.cors_origin.clone(),
        }
    }
}

pub fn router(reg: Arc<Registry>, control: Option<Control>, opts: Options) -> Router {
    let ctx = Ctx {
        reg,
        control,
        token: opts.token.filter(|t| !t.is_empty()).map(Arc::from),
    };
    let api = Router::new()
        .route("/api/v1/status", get(status))
        .route("/api/v1/bridges", get(bridges))
        .route("/api/v1/bridges/{name}", get(bridge))
        .route("/api/v1/bridges/{name}/monitor", get(monitor))
        .route("/api/v1/bridges/{name}/restart", post(restart))
        .route("/api/v1/reload", post(reload))
        .route("/api/v1/logs", get(logs))
        .route("/api/v1/config", get(config))
        .route("/api/v1/ports", get(ports))
        .route("/api/v1/stream", get(stream))
        .route_layer(middleware::from_fn_with_state(ctx.clone(), guard));
    let mut app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(prometheus))
        .merge(api);
    if opts.ui {
        app = app.merge(web::router());
    }
    let cors = opts
        .cors_origin
        .and_then(|o| HeaderValue::from_str(&o).ok());
    app.layer(middleware::from_fn(move |req: Request, next: Next| {
        let cors = cors.clone();
        async move {
            let mut res = next.run(req).await;
            let h = res.headers_mut();
            h.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            if let Some(origin) = cors {
                h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
                h.insert(
                    header::ACCESS_CONTROL_ALLOW_HEADERS,
                    HeaderValue::from_static("authorization, x-requested-with"),
                );
            }
            res
        }
    }))
    .with_state(ctx)
}

pub async fn serve(
    cfg: HttpApiConfig,
    reg: Arc<Registry>,
    control: Control,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(&cfg.listen)
        .await
        .with_context(|| format!("HTTP API cannot listen on {}", cfg.listen))?;
    let addr = listener.local_addr()?;
    if cfg.auth.is_none() && !addr.ip().is_loopback() {
        warn!(
            "HTTP API on {addr} has no token; anyone on the network can read it and restart bridges"
        );
    }
    info!(
        "HTTP API on http://{addr}{}",
        if cfg.ui { " (dashboard at /)" } else { "" }
    );
    axum::serve(listener, router(reg, Some(control), Options::from(&cfg)))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// Separate Prometheus-only listener.
pub async fn serve_metrics(
    listen: String,
    reg: Arc<Registry>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("metrics cannot listen on {listen}"))?;
    info!(
        "Prometheus metrics on http://{}/metrics",
        listener.local_addr()?
    );
    let ctx = Ctx {
        reg,
        control: None,
        token: None,
    };
    let app = Router::new()
        .route("/metrics", get(prometheus))
        .with_state(ctx);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

async fn guard(State(ctx): State<Ctx>, headers: HeaderMap, req: Request, next: Next) -> Response {
    if let Some(expected) = &ctx.token {
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if !ct_eq(presented.as_bytes(), expected.as_bytes()) {
            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
                Json(json!({"error": "unauthorized"})),
            )
                .into_response();
        }
    }
    if req.method() == Method::POST && !headers.contains_key("x-requested-with") {
        return error(
            StatusCode::FORBIDDEN,
            "POST requests need an X-Requested-With header",
        );
    }
    next.run(req).await
}

fn status_doc(reg: &Registry) -> serde_json::Value {
    let bridges: Vec<_> = reg.bridges().iter().map(|b| b.snapshot()).collect();
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_s": reg.uptime_s(),
        "config_path": reg.config_path.lock().unwrap().clone(),
        "bridges": bridges,
    })
}

async fn health(State(ctx): State<Ctx>) -> Response {
    let bridges = ctx.reg.bridges();
    let open = bridges
        .iter()
        .filter(|b| b.status() == Status::Open)
        .count();
    let ok = open == bridges.len();
    let body = Json(json!({
        "status": if ok { "ok" } else { "degraded" },
        "version": env!("CARGO_PKG_VERSION"),
        "bridges": bridges.len(),
        "open": open,
        "down": bridges.iter().filter(|b| b.status() != Status::Open).map(|b| b.name.clone()).collect::<Vec<_>>(),
    }));
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        body,
    )
        .into_response()
}

async fn prometheus(State(ctx): State<Ctx>) -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        metrics::render(&ctx.reg),
    )
        .into_response()
}

async fn status(State(ctx): State<Ctx>) -> Response {
    Json(status_doc(&ctx.reg)).into_response()
}

async fn bridges(State(ctx): State<Ctx>) -> Response {
    Json(
        ctx.reg
            .bridges()
            .iter()
            .map(|b| b.snapshot())
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn bridge(State(ctx): State<Ctx>, Path(name): Path<String>) -> Response {
    match ctx.reg.bridge(&name) {
        Some(b) => Json(b.snapshot()).into_response(),
        None => error(StatusCode::NOT_FOUND, format!("no bridge named {name}")),
    }
}

#[derive(Deserialize)]
struct MonitorQuery {
    #[serde(default)]
    after: u64,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn monitor(
    State(ctx): State<Ctx>,
    Path(name): Path<String>,
    Query(q): Query<MonitorQuery>,
) -> Response {
    let Some(b) = ctx.reg.bridge(&name) else {
        return error(StatusCode::NOT_FOUND, format!("no bridge named {name}"));
    };
    let chunks: Vec<_> = b
        .monitor_since(q.after)
        .into_iter()
        .map(|c| json!({ "seq": c.seq, "ms": c.ms, "dir": if c.dir == Direction::Rx { "rx" } else { "tx" }, "len": c.len, "hex": hex(&c.bytes) }))
        .collect();
    Json(json!({ "bridge": name, "chunks": chunks })).into_response()
}

async fn restart(State(ctx): State<Ctx>, Path(name): Path<String>) -> Response {
    match ctx.reg.bridge(&name) {
        Some(b) => {
            info!(bridge = %name, "restart requested over the API");
            b.restart.notify_one();
            (StatusCode::ACCEPTED, Json(json!({ "restarting": name }))).into_response()
        }
        None => error(StatusCode::NOT_FOUND, format!("no bridge named {name}")),
    }
}

async fn reload(State(ctx): State<Ctx>) -> Response {
    let Some(control) = &ctx.control else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "reload unavailable");
    };
    match control.reload().await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => error(StatusCode::UNPROCESSABLE_ENTITY, e),
    }
}

#[derive(Deserialize)]
struct LogQuery {
    n: Option<usize>,
}

async fn logs(State(ctx): State<Ctx>, Query(q): Query<LogQuery>) -> Response {
    Json(json!({ "lines": ctx.reg.logs.tail(q.n.unwrap_or(200).min(1000)) })).into_response()
}

async fn config(State(ctx): State<Ctx>) -> Response {
    Json(ctx.reg.config.lock().unwrap().clone()).into_response()
}

async fn ports() -> Response {
    match tokio::task::spawn_blocking(crate::scan::serial::scan_serial).await {
        Ok(p) => Json(p).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn stream(
    State(ctx): State<Ctx>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let events = futures_util::stream::unfold((ctx.reg, true), |(reg, first)| async move {
        if !first {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let e = Event::default()
            .event("status")
            .json_data(status_doc(&reg))
            .unwrap_or_default();
        Some((Ok(e), (reg, false)))
    });
    Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::BridgeStats;
    use axum::body::Body;
    use tower::ServiceExt;

    fn app(token: Option<&str>) -> (Router, Arc<Registry>) {
        let reg = Registry::new();
        let cfg: crate::config::BridgeConfig = toml::from_str(
            "name = \"ecu\"\n[source]\ntype = \"serial\"\nport = \"/dev/ttyUSB0\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        let b = Arc::new(BridgeStats::new(&cfg));
        b.record(Direction::Rx, &bytes::Bytes::from_static(b"\x41\x00"));
        reg.set_bridges(vec![b]);
        (
            router(
                reg.clone(),
                None,
                Options {
                    token: token.map(str::to_string),
                    ui: true,
                    cors_origin: None,
                },
            ),
            reg,
        )
    }

    async fn call(
        app: &Router,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let code = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (code, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn health_and_metrics_are_open_api_needs_token() {
        let (app, _) = app(Some("t0ken"));
        let (code, body) = call(&app, "GET", "/health", &[]).await;
        assert_eq!(code, StatusCode::SERVICE_UNAVAILABLE, "source not open yet");
        assert!(body.contains("\"degraded\""));
        assert_eq!(call(&app, "GET", "/metrics", &[]).await.0, StatusCode::OK);
        assert_eq!(
            call(&app, "GET", "/api/v1/status", &[]).await.0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(
                &app,
                "GET",
                "/api/v1/status",
                &[("authorization", "Bearer wrong")]
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let (code, body) = call(
            &app,
            "GET",
            "/api/v1/status",
            &[("authorization", "Bearer t0ken")],
        )
        .await;
        assert_eq!(code, StatusCode::OK);
        assert!(body.contains("\"name\":\"ecu\""));
    }

    #[tokio::test]
    async fn monitor_returns_hex_chunks() {
        let (app, _) = app(None);
        tokio::time::sleep(Duration::from_millis(10)).await; // let the chunk settle
        let (code, body) = call(&app, "GET", "/api/v1/bridges/ecu/monitor?after=0", &[]).await;
        assert_eq!(code, StatusCode::OK);
        assert!(body.contains("\"hex\":\"4100\""), "{body}");
        assert_eq!(
            call(&app, "GET", "/api/v1/bridges/nope/monitor", &[])
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn posts_need_x_requested_with() {
        let (app, reg) = app(None);
        assert_eq!(
            call(&app, "POST", "/api/v1/bridges/ecu/restart", &[])
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let (code, _) = call(
            &app,
            "POST",
            "/api/v1/bridges/ecu/restart",
            &[("x-requested-with", "io-to-net")],
        )
        .await;
        assert_eq!(code, StatusCode::ACCEPTED);
        // the restart request is stored until the bridge loop picks it up
        tokio::time::timeout(
            Duration::from_millis(100),
            reg.bridge("ecu").unwrap().restart.notified(),
        )
        .await
        .unwrap();
        assert_eq!(
            call(&app, "POST", "/api/v1/reload", &[("x-requested-with", "x")])
                .await
                .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn security_headers_on_every_response() {
        let (app, _) = app(None);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/bridges")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    }
}
