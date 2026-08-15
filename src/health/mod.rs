//! Readiness state for health endpoints.

use std::sync::atomic::{AtomicBool, Ordering};

/// Tracks whether the process is ready to serve traffic.
#[derive(Debug)]
pub struct ReadinessState {
    config_valid: AtomicBool,
    db_reachable: AtomicBool,
    migrations_current: AtomicBool,
    shutting_down: AtomicBool,
}

impl ReadinessState {
    pub fn new_ready() -> Self {
        Self {
            config_valid: AtomicBool::new(true),
            db_reachable: AtomicBool::new(false),
            migrations_current: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        }
    }

    pub fn set_db_reachable(&self, reachable: bool) {
        self.db_reachable.store(reachable, Ordering::SeqCst);
    }

    pub fn set_migrations_current(&self, current: bool) {
        self.migrations_current.store(current, Ordering::SeqCst);
    }

    pub fn mark_shutting_down(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }

    pub fn is_ready(&self) -> bool {
        self.config_valid.load(Ordering::SeqCst)
            && self.db_reachable.load(Ordering::SeqCst)
            && self.migrations_current.load(Ordering::SeqCst)
            && !self.shutting_down.load(Ordering::SeqCst)
    }

    pub fn is_live(&self) -> bool {
        !self.shutting_down.load(Ordering::SeqCst)
    }
}
