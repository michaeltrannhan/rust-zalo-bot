//! Public config validation entrypoint.

use std::path::Path;

use crate::error::AppError;

use super::load::load_config;

/// Validate resolved configuration without starting the runtime.
pub fn validate_config(config_path: Option<&Path>) -> Result<(), AppError> {
    load_config(config_path)?;
    Ok(())
}
