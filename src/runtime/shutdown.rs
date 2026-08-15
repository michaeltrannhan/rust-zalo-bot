//! Graceful shutdown on SIGTERM with readiness flip.

use std::sync::Arc;

use tokio::sync::watch;
use tracing::info;

use crate::health::ReadinessState;

const SHUTDOWN_DEADLINE_SECS: u64 = 30;

/// Signals shutdown to supervised tasks.
pub struct ShutdownSignal {
    shutdown_tx: watch::Sender<bool>,
    readiness: Arc<ReadinessState>,
}

impl ShutdownSignal {
    pub fn new(shutdown_tx: watch::Sender<bool>, readiness: Arc<ReadinessState>) -> Self {
        Self {
            shutdown_tx,
            readiness,
        }
    }

    pub async fn listen(self) {
        let sigterm = async {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut stream = signal(SignalKind::terminate()).expect("register SIGTERM");
                stream.recv().await;
            }
            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c().await.ok();
            }
        };

        let ctrl_c = async {
            tokio::signal::ctrl_c().await.ok();
        };

        tokio::select! {
            _ = sigterm => info!("received SIGTERM"),
            _ = ctrl_c => info!("received Ctrl+C"),
        }

        info!("marking readiness false before shutdown");
        self.readiness.mark_shutting_down();
        let _ = self.shutdown_tx.send(true);

        tokio::time::sleep(std::time::Duration::from_secs(SHUTDOWN_DEADLINE_SECS)).await;
        info!("shutdown deadline elapsed");
    }
}
