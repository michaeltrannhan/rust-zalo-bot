//! Configuration loading, validation, and source attribution.

mod load;
mod types;
mod validate;

pub use load::{ConfigSource, ResolvedConfig, ResolvedValue, load_config};
pub use types::Config;
pub use validate::validate_config;
