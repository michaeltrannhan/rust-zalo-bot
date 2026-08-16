//! Configuration loading, validation, and source attribution.

mod load;
mod types;
mod validate;

pub use load::{
    ConfigSource, ExtractionBackend, ResolvedAiProfile, ResolvedConfig, ResolvedValue,
    StorageBackend, load_config,
};
pub use types::Config;
pub use validate::validate_config;
