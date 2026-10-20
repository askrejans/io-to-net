use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs, Wrap};

use super::app::{App, TABS, Tab};
use crate::core::stats::{BridgeSnapshot, Direction, Status};

const DIM: Style = Style::new().fg(Color::DarkGray);

pub fn draw(f: &mut Frame, app: &App) {
    let [header, tabs, main, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    let titles = TABS
        .iter()
        .enumerate()
        .map(|(i, t)| format!(" {} {} ", i + 1, t.title()));
    f.render_widget(
        Tabs::new(titles)
            .select(TABS.iter().position(|t| *t == app.tab))
            .highlight_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .divider("│"),
        tabs,
    );
    match app.tab {
        Tab::Bridges => bridges(f, app, main),
        Tab::Clients => clients(f, app, main),
        Tab::Monitor => monitor(f, app, main),
        Tab::Logs => logs(f, app, main),
    }
    f.render_widget(Paragraph::new(Span::styled(app.notice.as_str(), DIM)), foot);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let open = app
        .bridges
        .iter()
        .filter(|b| b.status == Status::Open)
        .count();
    let clients: usize = app.bridges.iter().map(|b| b.clients.len()).sum();
    let rx: f64 = app.bridges.iter().map(|b| b.rx_bps).sum();
    let line = Line::from(vec![
        Span::styled(
            format!(" io-to-net {} ", env!("CARGO_PKG_VERSION")),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " bridges {open}/{} open · clients {clients} · RX {} · up {}",
            app.bridges.len(),
            rate(rx),
            uptime(app.uptime_s)
        )),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn status_style(s: Status) -> Style {
    match s {
        Status::Open => Style::new().fg(Color::Green),
        Status::Opening => Style::new().fg(Color::Yellow),
        Status::Waiting => Style::new().fg(Color::Red),
        Status::Stopped => DIM,
    }
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::Open => "open",
        Status::Opening => "opening",
        Status::Waiting => "waiting",
        Status::Stopped => "stopped",
    }
}

fn bridges(f: &mut Frame, app: &App, area: Rect) {
    let [table, detail] = Layout::vertical([Constraint::Min(4), Constraint::Length(9)]).areas(area);
    let rows = app.bridges.iter().enumerate().map(|(i, b)| {
        let style = if i == app.selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        Row::new(vec![
            Cell::from(b.name.clone()),
            Cell::from(status_name(b.status)).style(status_style(b.status)),
            Cell::from(format!("{} {}", b.transport, b.listen)),
            Cell::from(b.clients.len().to_string()),
            Cell::from(rate(b.rx_bps)),
            Cell::from(rate(b.tx_bps)),
            Cell::from(format!("{:.1}/s", b.frame_rate)),
            Cell::from(b.protocol),
        ])
        .style(style)
    });
    let header = Row::new([
        "Bridge", "Status", "Listen", "Clients", "RX", "TX", "Rate", "Protocol",
    ])
    .style(DIM);
    let widths = [
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Min(18),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(9),
        Constraint::Length(14),
    ];
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(Block::new().borders(Borders::TOP).border_style(DIM)),
        table,
    );
    if let Some(b) = app.bridges.get(app.selected) {
        f.render_widget(
            Paragraph::new(detail_lines(b)).block(
                Block::new()
                    .borders(Borders::TOP)
                    .border_style(DIM)
                    .title(" Selected "),
            ),
            detail,
        );
    } else {
        f.render_widget(
            Paragraph::new("No bridges configured. Add [[bridges]] to the config and press R."),
            detail,
        );
    }
}

fn detail_lines(b: &BridgeSnapshot) -> Vec<Line<'static>> {
    let kv =
        |k: &str, v: String| Line::from(vec![Span::styled(format!("{k:<12}"), DIM), Span::raw(v)]);
    let mut flags = vec![format!("{:?}", b.mode).to_lowercase()];
    if b.read_only {
        flags.push("read-only".into());
    }
    if b.auth {
        flags.push("token".into());
    }
    vec![
        kv("Source", b.source.clone()),
        kv(
            status_name(b.status),
            format!("{} ({} s)", b.detail, b.status_for_s),
        ),
        kv(
            "Traffic",
            format!(
                "RX {} in {} reads · TX {} in {} writes",
                bytes(b.rx_bytes),
                b.rx_chunks,
                bytes(b.tx_bytes),
                b.tx_chunks
            ),
        ),
        kv(
            "Frames",
            format!(
                "{} · {:.1}/s · avg {:.0} B",
                b.protocol, b.frame_rate, b.avg_frame_bytes
            ),
        ),
        kv(
            "Source",
            format!(
                "{} opens · {} failed · {} lost",
                b.opens, b.open_failures, b.source_errors
            ),
        ),
        kv(
            "Clients",
            format!(
                "{} now · {} total · {} refused · {} lagged · {} replaced · {} bad token · {} writes dropped",
                b.clients.len(),
                b.clients_total,
                b.refused,
                b.lagged,
                b.replaced,
                b.auth_failures,
                b.tx_dropped
            ),
        ),
        kv("Mode", flags.join(", ")),
    ]
}

fn clients(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.bridges.iter().flat_map(|b| {
        b.clients.iter().map(move |c| {
            Row::new(vec![
                b.name.clone(),
                c.addr.clone(),
                uptime(c.connected_s),
                bytes(c.sent_bytes),
                bytes(c.received_bytes),
            ])
        })
    });
    let widths = [
        Constraint::Length(14),
        Constraint::Min(22),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
    ];
    let header = Row::new(["Bridge", "Client", "Connected", "Sent", "Received"]).style(DIM);
    f.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(Block::new().borders(Borders::TOP).border_style(DIM)),
        area,
    );
}

fn monitor(f: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " {} · newest last · ↑↓ bridge ",
        app.selected_name().unwrap_or("—")
    );
    let height = area.height.saturating_sub(1) as usize;
    let lines: Vec<Line> = app
        .monitor
        .iter()
        .rev()
        .take(height)
        .rev()
        .map(|c| {
            let (arrow, colour) = if c.dir == Direction::Rx {
                ("◀ RX", Color::Cyan)
            } else {
                ("▶ TX", Color::Yellow)
            };
            let shown = &c.bytes[..c.bytes.len().min(32)];
            let hex: String = shown.iter().map(|b| format!("{b:02x} ")).collect();
            let ascii: String = shown
                .iter()
                .map(|&b| {
                    if (0x20..0x7f).contains(&b) {
                        b as char
                    } else {
                        '·'
                    }
                })
                .collect();
            let more = if c.bytes.len() > 32 { "…" } else { "" };
            Line::from(vec![
                Span::styled(
                    format!("{arrow} {:>4} ", c.bytes.len()),
                    Style::new().fg(colour),
                ),
                Span::raw(format!("{hex:<96}")),
                Span::styled(format!("{ascii}{more}"), DIM),
            ])
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(DIM)
                .title(title),
        ),
        area,
    );
}

fn logs(f: &mut Frame, app: &App, area: Rect) {
    let height = area.height.saturating_sub(1) as usize;
    let end = app.logs.len().saturating_sub(app.log_scroll);
    let start = end.saturating_sub(height);
    let lines: Vec<Line> = app.logs[start..end]
        .iter()
        .map(|l| {
            let style = if l.contains("ERROR") {
                Style::new().fg(Color::Red)
            } else if l.contains("WARN") {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new()
            };
            Line::styled(l.clone(), style)
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(DIM)
                .title(" ↑↓ scroll "),
        ),
        area,
    );
}

pub fn bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KiB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

pub fn rate(bps: f64) -> String {
    format!("{}/s", bytes(bps as u64))
}

pub fn uptime(s: u64) -> String {
    match s {
        s if s >= 86400 => format!("{}d {}h", s / 86400, s % 86400 / 3600),
        s if s >= 3600 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m {}s", s / 60, s % 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(rate(6500.0), "6.3 KiB/s");
        assert_eq!(uptime(59), "59s");
        assert_eq!(uptime(3661), "1h 1m");
        assert_eq!(uptime(90000), "1d 1h");
    }

    #[test]
    fn renders_without_panicking() {
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut t = ratatui::Terminal::new(backend).unwrap();
        for tab in TABS {
            let app = App {
                tab,
                selected: 0,
                bridges: Vec::new(),
                monitor: Vec::new(),
                logs: vec!["INFO x".into()],
                notice: String::new(),
                uptime_s: 5,
                log_scroll: 0,
            };
            t.draw(|f| draw(f, &app)).unwrap();
        }
    }
}
