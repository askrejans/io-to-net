//! Runs the configured bridges and applies configuration reloads: bridges
//! whose configuration is unchanged keep running (with their clients),
//! changed ones restart, removed ones stop, new ones start.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use super::bridge;
use super::stats::{BridgeStats, Registry};
use crate::config::BridgeConfig;

struct Running {
    key: String,
    stats: Arc<BridgeStats>,
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ReloadSummary {
    pub started: Vec<String>,
    pub restarted: Vec<String>,
    pub stopped: Vec<String>,
    pub unchanged: Vec<String>,
}

pub struct Manager {
    registry: Arc<Registry>,
    running: Vec<Running>,
}

fn key(cfg: &BridgeConfig) -> String {
    serde_json::to_string(cfg).unwrap_or_default()
}

impl Manager {
    pub fn new(registry: Arc<Registry>) -> Self {
        Manager {
            registry,
            running: Vec::new(),
        }
    }

    async fn stop(r: Running) {
        let _ = r.stop.send(true);
        if tokio::time::timeout(Duration::from_secs(5), r.handle)
            .await
            .is_err()
        {
            warn!(bridge = %r.stats.name, "bridge did not stop in time");
        }
    }

    pub async fn apply(&mut self, bridges: &[BridgeConfig]) -> ReloadSummary {
        let mut summary = ReloadSummary::default();
        let mut keep = Vec::new();
        // Stop first so a changed bridge frees its listen address.
        for r in std::mem::take(&mut self.running) {
            match bridges.iter().find(|b| b.name == r.stats.name) {
                Some(b) if key(b) == r.key => keep.push(r),
                Some(_) => {
                    summary.restarted.push(r.stats.name.clone());
                    Self::stop(r).await;
                }
                None => {
                    summary.stopped.push(r.stats.name.clone());
                    Self::stop(r).await;
                }
            }
        }
        for b in bridges {
            if let Some(i) = keep.iter().position(|r| r.stats.name == b.name) {
                summary.unchanged.push(b.name.clone());
                self.running.push(keep.swap_remove(i));
                continue;
            }
            if !summary.restarted.contains(&b.name) {
                summary.started.push(b.name.clone());
            }
            let stats = Arc::new(BridgeStats::new(b));
            let (stop, stop_rx) = watch::channel(false);
            let handle = tokio::spawn(bridge::run(b.clone(), stats.clone(), stop_rx));
            self.running.push(Running {
                key: key(b),
                stats,
                stop,
                handle,
            });
        }
        self.registry
            .set_bridges(self.running.iter().map(|r| r.stats.clone()).collect());
        summary
    }

    pub async fn shutdown(&mut self) {
        self.apply(&[]).await;
    }
}

/// Requests to the manager task.
pub enum Command {
    Reload(oneshot::Sender<Result<ReloadSummary, String>>),
}

#[derive(Clone)]
pub struct Control(mpsc::Sender<Command>);

impl Control {
    pub async fn reload(&self) -> Result<ReloadSummary, String> {
        let (tx, rx) = oneshot::channel();
        self.0
            .send(Command::Reload(tx))
            .await
            .map_err(|_| "shutting down".to_string())?;
        rx.await.map_err(|_| "shutting down".to_string())?
    }
}

/// Run the manager until `shutdown` resolves. Reloads re-read `config_path`.
pub fn spawn(
    mut manager: Manager,
    config_path: PathBuf,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> (Control, JoinHandle<()>) {
    let (tx, mut rx) = mpsc::channel::<Command>(8);
    let handle = tokio::spawn(async move {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                Some(cmd) = rx.recv() => match cmd {
                    Command::Reload(reply) => {
                        let result = match crate::config::loader::load_config(&config_path) {
                            Ok(cfg) => {
                                let s = manager.apply(&cfg.bridges).await;
                                *manager.registry.config.lock().unwrap() = crate::config::masked(&cfg);
                                info!(started = ?s.started, restarted = ?s.restarted, stopped = ?s.stopped, "configuration reloaded");
                                Ok(s)
                            }
                            Err(e) => {
                                let msg = format!("{e:#}");
                                warn!(error = %msg, "reload rejected, keeping the running configuration");
                                Err(msg)
                            }
                        };
                        let _ = reply.send(result);
                    }
                },
            }
        }
        manager.shutdown().await;
    });
    (Control(tx), handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(name: &str, port: u16, extra: &str) -> BridgeConfig {
        toml::from_str(&format!(
            "name = \"{name}\"\n{extra}\n[source]\ntype = \"tcp_client\"\naddress = \"127.0.0.1:1\"\n[transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:{port}\"\n"
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn reload_diffs_bridges() {
        let reg = Registry::new();
        let mut m = Manager::new(reg.clone());
        let s = m.apply(&[bridge("a", 0, ""), bridge("b", 0, "")]).await;
        assert_eq!(s.started, vec!["a", "b"]);
        let a_before = reg.bridge("a").unwrap();

        let s = m
            .apply(&[
                bridge("a", 0, ""),
                bridge("b", 0, "read_only = true"),
                bridge("c", 0, ""),
            ])
            .await;
        assert_eq!(
            s,
            ReloadSummary {
                started: vec!["c".into()],
                restarted: vec!["b".into()],
                stopped: vec![],
                unchanged: vec!["a".into()],
            }
        );
        assert!(
            Arc::ptr_eq(&a_before, &reg.bridge("a").unwrap()),
            "unchanged bridge keeps running"
        );
        assert!(reg.bridge("b").unwrap().read_only);

        let s = m.apply(&[bridge("c", 0, "")]).await;
        assert_eq!(s.stopped, vec!["a", "b"]);
        assert_eq!(reg.bridges().len(), 1);
        m.shutdown().await;
        assert!(reg.bridges().is_empty());
    }

    #[tokio::test]
    async fn reload_from_file_and_rejects_bad_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[[bridges]]\nname = \"a\"\n[bridges.source]\ntype = \"tcp_client\"\naddress = \"127.0.0.1:1\"\n[bridges.transport]\ntype = \"tcp\"\nlisten = \"127.0.0.1:0\"\n").unwrap();
        let reg = Registry::new();
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let (ctl, h) = spawn(Manager::new(reg.clone()), path.clone(), async {
            let _ = stop_rx.await;
        });
        assert_eq!(ctl.reload().await.unwrap().started, vec!["a"]);
        std::fs::write(&path, "bridges = 3").unwrap();
        assert!(ctl.reload().await.is_err());
        assert_eq!(reg.bridges().len(), 1, "bad config leaves bridges running");
        stop_tx.send(()).unwrap();
        h.await.unwrap();
        assert!(reg.bridges().is_empty());
    }
}
