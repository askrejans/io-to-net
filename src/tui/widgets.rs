use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Table, Tabs, Wrap,
};
use ratatui::Frame;

use crate::core::metrics::GlobalMetrics;
use crate::tui::app::Tab;

/// Main draw function — renders the full TUI layout.
pub fn draw(frame: &mut Frame, metrics: &GlobalMetrics, logs: &[String], tab: Tab) {
    let size = frame.area();

    // Split: header (3 lines) + tabs bar (3 lines) + main content + log panel
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Length(3),  // tabs
            Constraint::Min(10),    // main content
            Constraint::Length(8),  // log panel
        ])
        .split(size);

    draw_header(frame, metrics, chunks[0]);
    draw_tabs(frame, tab, chunks[1]);

    match tab {
        Tab::Ports => draw_ports_tab(frame, metrics, chunks[2]),
        Tab::Connections => draw_connections_tab(frame, metrics, chunks[2]),
        Tab::Stats => draw_stats_tab(frame, metrics, chunks[2]),
        Tab::Logs => draw_logs_tab(frame, logs, chunks[2]),
    }

    draw_log_panel(frame, logs, chunks[3]);
}

fn draw_header(frame: &mut Frame, metrics: &GlobalMetrics, area: Rect) {
    let total_rx = format_bytes(metrics.total_rx_bytes());
    let total_tx = format_bytes(metrics.total_tx_bytes());
    let rx_rate = format_rate(metrics.total_rx_rate());
    let uptime = format_uptime(metrics.uptime_seconds());

    let text = Line::from(vec![
        Span::styled(
            format!(
                " io-to-net v{}  |  bridges: {}  |  clients: {}  |  RX: {} ({})  TX: {}  |  up: {} ",
                env!("CARGO_PKG_VERSION"),
                metrics.bridges.len(),
                metrics.total_clients(),
                total_rx,
                rx_rate,
                total_tx,
                uptime,
            ),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
    ]);

    let header = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
    frame.render_widget(header, area);
}

fn draw_tabs(frame: &mut Frame, current: Tab, area: Rect) {
    let titles: Vec<Line> = vec![
        Line::from(" Ports [1] "),
        Line::from(" Connections [2] "),
        Line::from(" Stats [3] "),
        Line::from(" Logs [4] "),
    ];

    let tabs = Tabs::new(titles)
        .select(current.index())
        .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)))
        .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .divider("|");

    frame.render_widget(tabs, area);
}

fn draw_ports_tab(frame: &mut Frame, metrics: &GlobalMetrics, area: Rect) {
    let header_cells = ["Name", "Source", "Listen", "Clients", "RX", "Rate", "Protocol", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = metrics.bridges.values().map(|bm| {
        let status = if bm.running {
            Span::styled("● running", Style::default().fg(Color::Green))
        } else {
            Span::styled("○ stopped", Style::default().fg(Color::Red))
        };
        let hint = bm.protocol_hint.to_string();
        let hint_style = match hint.as_str() {
            "unknown" => Style::default().fg(Color::DarkGray),
            _ => Style::default().fg(Color::Cyan),
        };
        Row::new(vec![
            Cell::from(bm.name.clone()),
            Cell::from(truncate(&bm.source_description, 24)),
            Cell::from(bm.transport_listen.clone()),
            Cell::from(bm.clients_connected.to_string()),
            Cell::from(format_bytes(bm.bytes_rx_total)),
            Cell::from(format_rate(bm.rx_rate_bps)),
            Cell::from(Span::styled(hint, hint_style)),
            Cell::from(Line::from(status)),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Length(26),
            Constraint::Length(20),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(16),
            Constraint::Min(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Bridges / Ports ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(table, area);
}

fn draw_connections_tab(frame: &mut Frame, metrics: &GlobalMetrics, area: Rect) {
    let header_cells = ["Client IP", "Bridge", "Connected", "Bytes In", "Bytes Out"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows: Vec<Row> = metrics.connections.values().map(|c| {
        Row::new(vec![
            Cell::from(c.client_ip.clone()),
            Cell::from(c.bridge_name.clone()),
            Cell::from(format!("{}s", c.connected_seconds)),
            Cell::from(format_bytes(c.bytes_rx)),
            Cell::from(format_bytes(c.bytes_tx)),
        ])
    }).collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(22),
            Constraint::Length(16),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(12),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Active Connections ")
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(table, area);
}

fn draw_stats_tab(frame: &mut Frame, metrics: &GlobalMetrics, area: Rect) {
    let running = metrics.bridges.values().filter(|b| b.running).count();
    let total_errors: u64 = metrics.bridges.values().map(|b| b.errors).sum();

    let text = vec![
        Line::from(vec![
            Span::styled("  Active bridges:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(running.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Total clients:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(metrics.total_clients().to_string(), Style::default().fg(Color::Cyan)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Total RX:        ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_bytes(metrics.total_rx_bytes()), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  Total TX:        ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_bytes(metrics.total_tx_bytes()), Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("  RX rate:         ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_rate(metrics.total_rx_rate()), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Total errors:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                total_errors.to_string(),
                if total_errors > 0 {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default().fg(Color::Green)
                },
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Uptime:          ", Style::default().fg(Color::DarkGray)),
            Span::styled(format_uptime(metrics.uptime_seconds()), Style::default().fg(Color::White)),
        ]),
    ];

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Global Statistics ")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, area);
}

fn draw_logs_tab(frame: &mut Frame, logs: &[String], area: Rect) {
    let lines: Vec<Line> = logs.iter().map(|l| Line::from(l.as_str())).collect();
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Live Logs (newest first) ")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(para, area);
}

fn draw_log_panel(frame: &mut Frame, logs: &[String], area: Rect) {
    let last_n: Vec<Line> = logs.iter().take(6).map(|l| {
        Line::from(Span::styled(l.as_str(), Style::default().fg(Color::DarkGray)))
    }).collect();

    let para = Paragraph::new(last_n)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Logs  [q] quit  [Tab] switch tab ")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: true });

    frame.render_widget(para, area);
}

// ─────────────────────── Formatting helpers ───────────────────────

fn format_bytes(n: u64) -> String {
    if n >= 1_073_741_824 {
        format!("{:.1} GB", n as f64 / 1_073_741_824.0)
    } else if n >= 1_048_576 {
        format!("{:.1} MB", n as f64 / 1_048_576.0)
    } else if n >= 1024 {
        format!("{:.1} kB", n as f64 / 1024.0)
    } else {
        format!("{} B", n)
    }
}

fn format_rate(bps: f64) -> String {
    if bps >= 1_048_576.0 {
        format!("{:.1} MB/s", bps / 1_048_576.0)
    } else if bps >= 1024.0 {
        format!("{:.1} kB/s", bps / 1024.0)
    } else {
        format!("{:.0} B/s", bps)
    }
}

fn format_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{}d {:02}:{:02}:{:02}", d, h, m, s)
    } else {
        format!("{:02}:{:02}:{:02}", h, m, s)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
