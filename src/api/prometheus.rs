use std::sync::Arc;

use axum::routing::get;
use axum::{extract::State, response::IntoResponse, Router};
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use prometheus::{
    register_gauge, register_int_counter_vec, register_int_gauge_vec, Encoder, Gauge,
    IntCounterVec, IntGaugeVec, TextEncoder,
};
use tracing::info;

use crate::config::MetricsConfig;
use crate::core::metrics::GlobalMetrics;

// ─────────────────────── Metric definitions ───────────────────────

static BRIDGES_TOTAL: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "io_to_net_bridges_total",
        "Number of configured bridges",
        &["name", "source", "listen"]
    )
    .unwrap()
});

static CLIENTS_TOTAL: Lazy<IntGaugeVec> = Lazy::new(|| {
    register_int_gauge_vec!(
        "io_to_net_clients_total",
        "Current number of connected clients per bridge",
        &["bridge"]
    )
    .unwrap()
});

static BYTES_RX_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "io_to_net_bytes_rx_total",
        "Total bytes received from hardware per bridge",
        &["bridge"]
    )
    .unwrap()
});

static BYTES_TX_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "io_to_net_bytes_tx_total",
        "Total bytes sent to hardware per bridge",
        &["bridge"]
    )
    .unwrap()
});

static ERRORS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "io_to_net_errors_total",
        "Total errors per bridge",
        &["bridge"]
    )
    .unwrap()
});

static UPTIME_SECONDS: Lazy<Gauge> = Lazy::new(|| {
    register_gauge!("io_to_net_uptime_seconds", "Service uptime in seconds").unwrap()
});

/// Update Prometheus metrics from the current global state snapshot.
pub fn update_metrics(metrics: &GlobalMetrics) {
    UPTIME_SECONDS.set(metrics.uptime_seconds() as f64);

    for bm in metrics.bridges.values() {
        let labels = &[bm.name.as_str(), bm.source_description.as_str(), bm.transport_listen.as_str()];
        BRIDGES_TOTAL.with_label_values(labels).set(1);
        CLIENTS_TOTAL.with_label_values(&[&bm.name]).set(bm.clients_connected as i64);

        // Counters can only increase; track delta manually
        // (prometheus crate counters are monotonic fine — we inc by delta)
        // We just re-set abs values via a workaround: use _total as gauge-like
        // For simplicity we use the counter's get() and inc_by(delta)
        let prev_rx = BYTES_RX_TOTAL.with_label_values(&[&bm.name]).get();
        if bm.bytes_rx_total > prev_rx {
            BYTES_RX_TOTAL.with_label_values(&[&bm.name]).inc_by(bm.bytes_rx_total - prev_rx);
        }
        let prev_tx = BYTES_TX_TOTAL.with_label_values(&[&bm.name]).get();
        if bm.bytes_tx_total > prev_tx {
            BYTES_TX_TOTAL.with_label_values(&[&bm.name]).inc_by(bm.bytes_tx_total - prev_tx);
        }
        let prev_err = ERRORS_TOTAL.with_label_values(&[&bm.name]).get();
        if bm.errors > prev_err {
            ERRORS_TOTAL.with_label_values(&[&bm.name]).inc_by(bm.errors - prev_err);
        }
    }
}

/// Serve Prometheus metrics endpoint on its own port.
pub async fn serve(
    cfg: MetricsConfig,
    metrics: Arc<RwLock<GlobalMetrics>>,
) -> anyhow::Result<()> {
    let state = metrics;
    let app = Router::new()
        .route("/metrics", get(metrics_handler))
        .with_state(state);

    info!(listen = %cfg.listen, "Prometheus metrics endpoint listening");
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn metrics_handler(State(metrics): State<Arc<RwLock<GlobalMetrics>>>) -> impl IntoResponse {
    {
        let m = metrics.read();
        update_metrics(&m);
    }

    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buf = Vec::new();
    encoder.encode(&metric_families, &mut buf).unwrap_or(());
    (
        [(axum::http::header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        buf,
    )
}
