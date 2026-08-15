//! zl-expense library — walking-skeleton core for the Zalo expense bot.

pub mod cli;
pub mod config;
pub mod conversation;
pub mod db;
pub mod error;
pub mod health;
pub mod http;
pub mod ingress;
pub mod provider;
pub mod runtime;

pub use cli::{Cli, execute};
pub use config::{Config, ConfigSource, ResolvedConfig};
pub use error::{AppError, ErrorClass, ExitCode};
pub use runtime::{Role, RuntimeOptions, run};
