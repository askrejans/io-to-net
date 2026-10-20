//! Prometheus text exposition, rendered from the registry on each scrape.

use std::fmt::Write;

use crate::core::stats::{Registry, Status};

fn esc(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub fn render(reg: &Registry) -> String {
    let bridges: Vec<_> = reg.bridges().iter().map(|b| b.snapshot()).collect();
    let mut o = String::with_capacity(4096);
    let _ = writeln!(
        o,
        "# HELP io_to_net_info Build information.\n# TYPE io_to_net_info gauge"
    );
    let _ = writeln!(
        o,
        "io_to_net_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(
        o,
        "# HELP io_to_net_uptime_seconds Process uptime.\n# TYPE io_to_net_uptime_seconds gauge"
    );
    let _ = writeln!(o, "io_to_net_uptime_seconds {}", reg.uptime_s());

    let _ = writeln!(
        o,
        "# HELP io_to_net_bridge_info Bridge wiring.\n# TYPE io_to_net_bridge_info gauge"
    );
    for b in &bridges {
        let _ = writeln!(
            o,
            "io_to_net_bridge_info{{bridge=\"{}\",source=\"{}\",source_kind=\"{}\",transport=\"{}\",listen=\"{}\"}} 1",
            esc(&b.name),
            esc(&b.source),
            b.source_kind,
            b.transport,
            esc(&b.listen)
        );
    }

    type Get = fn(&crate::core::stats::BridgeSnapshot) -> f64;
    let series: &[(&str, &str, &str, Get)] = &[
        (
            "bridge_source_open",
            "gauge",
            "1 while the source is open.",
            |b| (b.status == Status::Open) as u8 as f64,
        ),
        ("bridge_clients", "gauge", "Connected clients.", |b| {
            b.clients.len() as f64
        }),
        (
            "bridge_rx_bytes_total",
            "counter",
            "Bytes read from the device.",
            |b| b.rx_bytes as f64,
        ),
        (
            "bridge_tx_bytes_total",
            "counter",
            "Bytes written to the device.",
            |b| b.tx_bytes as f64,
        ),
        (
            "bridge_rx_chunks_total",
            "counter",
            "Reads from the device.",
            |b| b.rx_chunks as f64,
        ),
        (
            "bridge_tx_chunks_total",
            "counter",
            "Client writes forwarded to the device.",
            |b| b.tx_chunks as f64,
        ),
        (
            "bridge_tx_dropped_total",
            "counter",
            "Client writes dropped (source down or queue full).",
            |b| b.tx_dropped as f64,
        ),
        (
            "bridge_source_opens_total",
            "counter",
            "Successful source opens.",
            |b| b.opens as f64,
        ),
        (
            "bridge_source_open_failures_total",
            "counter",
            "Failed source opens.",
            |b| b.open_failures as f64,
        ),
        (
            "bridge_source_errors_total",
            "counter",
            "Open sources that failed or closed.",
            |b| b.source_errors as f64,
        ),
        (
            "bridge_clients_connected_total",
            "counter",
            "Clients admitted.",
            |b| b.clients_total as f64,
        ),
        (
            "bridge_clients_refused_total",
            "counter",
            "Clients refused by max_clients.",
            |b| b.refused as f64,
        ),
        (
            "bridge_clients_lagged_total",
            "counter",
            "Clients disconnected for falling behind.",
            |b| b.lagged as f64,
        ),
        (
            "bridge_clients_replaced_total",
            "counter",
            "Clients replaced in exclusive mode.",
            |b| b.replaced as f64,
        ),
        (
            "bridge_auth_failures_total",
            "counter",
            "Rejected client tokens.",
            |b| b.auth_failures as f64,
        ),
        (
            "bridge_rx_bytes_per_second",
            "gauge",
            "Device read rate.",
            |b| b.rx_bps,
        ),
        (
            "bridge_frame_rate",
            "gauge",
            "Lines (line protocols) or reads per second.",
            |b| b.frame_rate,
        ),
    ];
    for (name, kind, help, get) in series {
        let _ = writeln!(
            o,
            "# HELP io_to_net_{name} {help}\n# TYPE io_to_net_{name} {kind}"
        );
        for b in &bridges {
            let _ = writeln!(
                o,
                "io_to_net_{name}{{bridge=\"{}\"}} {}",
                esc(&b.name),
                get(b)
            );
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stats::{BridgeStats, Counters};
    use std::sync::Arc;

    #[test]
    fn renders_valid_exposition() {
        let cfg: crate::config::BridgeConfig = toml::from_str(
            "name = \"ecu\"\n[source]\ntype = \"serial\"\nport = \"/dev/tty\\\"x\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:9000\"\n",
        )
        .unwrap();
        let reg = Registry::new();
        let b = Arc::new(BridgeStats::new(&cfg));
        Counters::add(&b.counters.rx_bytes, 1300);
        reg.set_bridges(vec![b]);
        let text = render(&reg);
        assert!(text.contains("io_to_net_bridge_rx_bytes_total{bridge=\"ecu\"} 1300"));
        assert!(
            text.contains("source=\"serial:///dev/tty\\\"x@115200\""),
            "{text}"
        );
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let value = line.rsplit(' ').next().unwrap();
            assert!(value.parse::<f64>().is_ok(), "bad sample line: {line}");
        }
    }
}
