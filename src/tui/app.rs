use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use parking_lot::RwLock;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::core::bridge_manager::BridgeManagerHandle;
use crate::core::metrics::GlobalMetrics;

use super::widgets::draw;

/// Which tab is currently visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Ports,
    Connections,
    Stats,
    Logs,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Tab::Ports => Tab::Connections,
            Tab::Connections => Tab::Stats,
            Tab::Stats => Tab::Logs,
            Tab::Logs => Tab::Ports,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Tab::Ports => Tab::Logs,
            Tab::Connections => Tab::Ports,
            Tab::Stats => Tab::Connections,
            Tab::Logs => Tab::Stats,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tab::Ports => 0,
            Tab::Connections => 1,
            Tab::Stats => 2,
            Tab::Logs => 3,
        }
    }
}

/// Run the interactive TUI until the user presses 'q'.
pub async fn run(
    metrics: Arc<RwLock<GlobalMetrics>>,
    log_buffer: Arc<parking_lot::Mutex<VecDeque<String>>>,
    _bridge_handle: BridgeManagerHandle,
) -> anyhow::Result<()> {
    // Install a custom panic hook so that any panic (in any thread) tears down
    // the terminal gracefully and writes the message to the log ring-buffer
    // instead of to stderr, preventing raw text from bleeding into the TUI.
    let ring_for_hook = log_buffer.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort terminal restore so the shell is usable after a crash.
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let _ = crossterm::ExecutableCommand::execute(
            &mut std::io::stderr(),
            crossterm::terminal::LeaveAlternateScreen,
        );

        let msg = format!("PANIC: {}", info);
        {
            let mut b = ring_for_hook.lock();
            if b.len() >= 1000 { b.pop_front(); }
            b.push_back(msg.clone());
        }
        // Also call the original hook so the message appears after the TUI
        // has left the alternate screen (visible in the normal terminal scroll-back).
        prev_hook(info);
    }));

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // Force ratatui to repaint every cell on first draw, covering any stale
    // content that may be present in the alternate screen buffer.
    terminal.clear()?;

    let mut current_tab = Tab::Ports;
    let tick = Duration::from_millis(100);

    loop {
        // Take a snapshot of metrics to avoid holding lock while rendering
        let metrics_snapshot = {
            let mut m = metrics.write();
            for bm in m.bridges.values_mut() {
                bm.update_rates();
            }
            m.clone()
        };

        let logs_snapshot = {
            let buf = log_buffer.lock();
            buf.iter().rev().take(200).cloned().collect::<Vec<_>>()
        };

        terminal.draw(|frame| {
            draw(frame, &metrics_snapshot, &logs_snapshot, current_tab);
        })?;

        // Poll for input events
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Char('Q') => break,
                        KeyCode::Tab | KeyCode::Right => { current_tab = current_tab.next(); }
                        KeyCode::BackTab | KeyCode::Left => { current_tab = current_tab.prev(); }
                        KeyCode::Char('1') => { current_tab = Tab::Ports; }
                        KeyCode::Char('2') => { current_tab = Tab::Connections; }
                        KeyCode::Char('3') => { current_tab = Tab::Stats; }
                        KeyCode::Char('4') => { current_tab = Tab::Logs; }
                        _ => {}
                    }
                }
            }
        }
    }

    // Restore terminal and panic hook
    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    // Reinstall the default panic hook now that the TUI is gone.
    std::panic::set_hook(Box::new(move |info| {
        eprintln!("{}", info);
    }));
    Ok(())
}

impl Clone for GlobalMetrics {
    fn clone(&self) -> Self {
        GlobalMetrics {
            bridges: self.bridges.clone(),
            connections: self.connections.clone(),
            start_time: self.start_time,
        }
    }
}
