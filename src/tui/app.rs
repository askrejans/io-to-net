//! Interactive terminal UI. Runs on a blocking thread (terminal input is
//! blocking) and reads the shared registry; actions go through the manager.

use std::sync::Arc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use super::widgets;
use crate::core::manager::Control;
use crate::core::stats::{BridgeSnapshot, Registry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Bridges,
    Clients,
    Monitor,
    Logs,
}

pub const TABS: [Tab; 4] = [Tab::Bridges, Tab::Clients, Tab::Monitor, Tab::Logs];

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Bridges => "Bridges",
            Tab::Clients => "Clients",
            Tab::Monitor => "Monitor",
            Tab::Logs => "Logs",
        }
    }
    fn index(self) -> usize {
        TABS.iter().position(|t| *t == self).unwrap_or(0)
    }
    fn step(self, d: isize) -> Self {
        TABS[(self.index() as isize + d).rem_euclid(TABS.len() as isize) as usize]
    }
}

pub struct App {
    pub tab: Tab,
    pub selected: usize,
    pub bridges: Vec<BridgeSnapshot>,
    pub monitor: Vec<crate::core::stats::Chunk>,
    pub logs: Vec<String>,
    pub notice: String,
    pub uptime_s: u64,
    pub log_scroll: usize,
}

impl App {
    pub fn selected_name(&self) -> Option<&str> {
        self.bridges.get(self.selected).map(|b| b.name.as_str())
    }
}

pub fn run(reg: Arc<Registry>, control: Control, rt: tokio::runtime::Handle) -> anyhow::Result<()> {
    let mut terminal = ratatui::try_init()?;
    let (notice_tx, notice_rx) = std::sync::mpsc::channel::<String>();
    let mut app = App {
        tab: Tab::Bridges,
        selected: 0,
        bridges: Vec::new(),
        monitor: Vec::new(),
        logs: Vec::new(),
        notice: "↑↓ select  r restart  R reload config  Tab/1-4 view  q quit".into(),
        uptime_s: 0,
        log_scroll: 0,
    };
    let result = (|| -> anyhow::Result<()> {
        loop {
            app.bridges = reg.bridges().iter().map(|b| b.snapshot()).collect();
            app.selected = app.selected.min(app.bridges.len().saturating_sub(1));
            app.monitor = app
                .selected_name()
                .and_then(|n| reg.bridge(n))
                .map(|b| b.monitor_since(0))
                .unwrap_or_default();
            app.logs = reg.logs.tail(500);
            app.uptime_s = reg.uptime_s();
            while let Ok(n) = notice_rx.try_recv() {
                app.notice = n;
            }
            terminal.draw(|f| widgets::draw(f, &app))?;

            if !event::poll(Duration::from_millis(250))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Tab | KeyCode::Right => app.tab = app.tab.step(1),
                KeyCode::BackTab | KeyCode::Left => app.tab = app.tab.step(-1),
                KeyCode::Char(c @ '1'..='4') => app.tab = TABS[(c as u8 - b'1') as usize],
                KeyCode::Up if app.tab == Tab::Logs => app.log_scroll += 1,
                KeyCode::Down if app.tab == Tab::Logs => {
                    app.log_scroll = app.log_scroll.saturating_sub(1)
                }
                KeyCode::Up => app.selected = app.selected.saturating_sub(1),
                KeyCode::Down => {
                    app.selected = (app.selected + 1).min(app.bridges.len().saturating_sub(1))
                }
                KeyCode::Char('r') => {
                    if let Some(b) = app.selected_name().and_then(|n| reg.bridge(n)) {
                        b.restart.notify_one();
                        app.notice = format!("restarting {}", b.name);
                    }
                }
                KeyCode::Char('R') => {
                    app.notice = "reloading configuration…".into();
                    let (control, tx) = (control.clone(), notice_tx.clone());
                    rt.spawn(async move {
                        let msg = match control.reload().await {
                            Ok(s) => format!(
                                "reloaded: {} started, {} restarted, {} stopped, {} unchanged",
                                s.started.len(),
                                s.restarted.len(),
                                s.stopped.len(),
                                s.unchanged.len()
                            ),
                            Err(e) => format!("reload failed: {e}"),
                        };
                        let _ = tx.send(msg);
                    });
                }
                _ => {}
            }
        }
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_wrap() {
        assert_eq!(Tab::Bridges.step(-1), Tab::Logs);
        assert_eq!(Tab::Logs.step(1), Tab::Bridges);
        assert_eq!(Tab::Clients.step(1), Tab::Monitor);
    }
}
