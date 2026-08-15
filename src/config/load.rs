//! Configuration loading with env overrides and source attribution.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::AppError;

use super::types::Config;

/// Where a resolved configuration value originated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Default,
    File,
    Env,
}

/// A single resolved value with source attribution (never secret values).
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedValue {
    pub value: serde_json::Value,
    pub source: ConfigSource,
}

/// Fully resolved configuration ready for runtime use.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub listen_address: String,
    pub database_url: String,
    pub max_connections: u32,
    pub receipt_extraction: u32,
    pub outbound_delivery: u32,
    pub original_receipt_days: u32,
    pub credentials_directory: PathBuf,
    pub bot_token_credential: String,
    pub webhook_secret_credential: String,
    pub attribution: BTreeMap<String, ResolvedValue>,
}

impl ResolvedConfig {
    /// Show attribution map as JSON (redacted — no secret values).
    pub fn show_json(&self) -> String {
        let map: BTreeMap<&str, &ResolvedValue> = self
            .attribution
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .collect();
        serde_json::to_string_pretty(&map).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Load configuration from file path with env overrides for CI.
pub fn load_config(config_path: Option<&Path>) -> Result<ResolvedConfig, AppError> {
    let mut attribution = BTreeMap::new();
    let mut cfg = Config::default();

    if let Some(path) = config_path {
        let contents = fs::read_to_string(path)
            .map_err(|e| AppError::config(format!("failed to read config file: {}", e)))?;
        let file_cfg: Config = toml::from_str(&contents)
            .map_err(|e| AppError::config(format!("invalid config TOML: {}", e)))?;
        merge_config(&mut cfg, &file_cfg);
        record_file_sources(&mut attribution, &file_cfg);
    } else {
        record_default_sources(&mut attribution);
    }

    apply_env_overrides(&mut cfg, &mut attribution)?;

    validate_resolved(&cfg)?;

    let credentials_dir = PathBuf::from(&cfg.credentials.directory);
    let database_url = resolve_database_url(&cfg, &credentials_dir)?;

    let resolved = ResolvedConfig {
        listen_address: cfg.server.listen_address.clone(),
        database_url,
        max_connections: cfg.database.max_connections,
        receipt_extraction: cfg.concurrency.receipt_extraction,
        outbound_delivery: cfg.concurrency.outbound_delivery,
        original_receipt_days: cfg.retention.original_receipt_days,
        credentials_directory: credentials_dir,
        bot_token_credential: cfg.zalo.bot_token_credential.clone(),
        webhook_secret_credential: cfg.zalo.webhook_secret_credential.clone(),
        attribution,
    };

    Ok(resolved)
}

fn merge_config(target: &mut Config, source: &Config) {
    target.server = source.server.clone();
    target.database = source.database.clone();
    target.concurrency = source.concurrency.clone();
    target.retention = source.retention.clone();
    target.credentials = source.credentials.clone();
    target.zalo = source.zalo.clone();
}

fn record_default_sources(attribution: &mut BTreeMap<String, ResolvedValue>) {
    let defaults = Config::default();
    insert_attr(
        attribution,
        "server.listen_address",
        defaults.server.listen_address,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "database.url_credential",
        defaults.database.url_credential,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "database.max_connections",
        defaults.database.max_connections,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "concurrency.receipt_extraction",
        defaults.concurrency.receipt_extraction,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "concurrency.outbound_delivery",
        defaults.concurrency.outbound_delivery,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "retention.original_receipt_days",
        defaults.retention.original_receipt_days,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "credentials.directory",
        defaults.credentials.directory,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "zalo.bot_token_credential",
        defaults.zalo.bot_token_credential,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "zalo.webhook_secret_credential",
        defaults.zalo.webhook_secret_credential,
        ConfigSource::Default,
    );
}

fn record_file_sources(attribution: &mut BTreeMap<String, ResolvedValue>, cfg: &Config) {
    insert_attr(
        attribution,
        "server.listen_address",
        cfg.server.listen_address.clone(),
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "database.url_credential",
        cfg.database.url_credential.clone(),
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "database.max_connections",
        cfg.database.max_connections,
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "concurrency.receipt_extraction",
        cfg.concurrency.receipt_extraction,
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "concurrency.outbound_delivery",
        cfg.concurrency.outbound_delivery,
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "retention.original_receipt_days",
        cfg.retention.original_receipt_days,
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "credentials.directory",
        cfg.credentials.directory.clone(),
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "zalo.bot_token_credential",
        cfg.zalo.bot_token_credential.clone(),
        ConfigSource::File,
    );
    insert_attr(
        attribution,
        "zalo.webhook_secret_credential",
        cfg.zalo.webhook_secret_credential.clone(),
        ConfigSource::File,
    );
}

fn apply_env_overrides(
    cfg: &mut Config,
    attribution: &mut BTreeMap<String, ResolvedValue>,
) -> Result<(), AppError> {
    if let Ok(v) = env::var("ZL_EXPENSE_LISTEN_ADDRESS") {
        cfg.server.listen_address = v;
        insert_attr(
            attribution,
            "server.listen_address",
            cfg.server.listen_address.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_DATABASE_URL_CREDENTIAL") {
        cfg.database.url_credential = v;
        insert_attr(
            attribution,
            "database.url_credential",
            cfg.database.url_credential.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_DATABASE_MAX_CONNECTIONS") {
        cfg.database.max_connections = parse_env_u32("ZL_EXPENSE_DATABASE_MAX_CONNECTIONS", &v)?;
        insert_attr(
            attribution,
            "database.max_connections",
            cfg.database.max_connections,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_RECEIPT_EXTRACTION_CONCURRENCY") {
        cfg.concurrency.receipt_extraction =
            parse_env_u32("ZL_EXPENSE_RECEIPT_EXTRACTION_CONCURRENCY", &v)?;
        insert_attr(
            attribution,
            "concurrency.receipt_extraction",
            cfg.concurrency.receipt_extraction,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_OUTBOUND_DELIVERY_CONCURRENCY") {
        cfg.concurrency.outbound_delivery =
            parse_env_u32("ZL_EXPENSE_OUTBOUND_DELIVERY_CONCURRENCY", &v)?;
        insert_attr(
            attribution,
            "concurrency.outbound_delivery",
            cfg.concurrency.outbound_delivery,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_RETENTION_ORIGINAL_RECEIPT_DAYS") {
        cfg.retention.original_receipt_days =
            parse_env_u32("ZL_EXPENSE_RETENTION_ORIGINAL_RECEIPT_DAYS", &v)?;
        insert_attr(
            attribution,
            "retention.original_receipt_days",
            cfg.retention.original_receipt_days,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_CREDENTIALS_DIRECTORY") {
        cfg.credentials.directory = v;
        insert_attr(
            attribution,
            "credentials.directory",
            cfg.credentials.directory.clone(),
            ConfigSource::Env,
        );
    }
    Ok(())
}

fn parse_env_u32(name: &str, value: &str) -> Result<u32, AppError> {
    value
        .parse()
        .map_err(|_| AppError::config(format!("{} must be a positive integer", name)))
}

fn insert_attr<T: Serialize>(
    attribution: &mut BTreeMap<String, ResolvedValue>,
    key: &str,
    value: T,
    source: ConfigSource,
) {
    attribution.insert(
        key.to_string(),
        ResolvedValue {
            value: serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
            source,
        },
    );
}

fn validate_resolved(cfg: &Config) -> Result<(), AppError> {
    if cfg.server.listen_address.trim().is_empty() {
        return Err(AppError::config("server.listen_address must not be empty"));
    }
    if cfg.database.max_connections == 0 {
        return Err(AppError::config(
            "database.max_connections must be greater than zero",
        ));
    }
    if cfg.concurrency.receipt_extraction == 0 {
        return Err(AppError::config(
            "concurrency.receipt_extraction must be greater than zero",
        ));
    }
    if cfg.concurrency.outbound_delivery == 0 {
        return Err(AppError::config(
            "concurrency.outbound_delivery must be greater than zero",
        ));
    }
    if cfg.retention.original_receipt_days < 1 || cfg.retention.original_receipt_days > 30 {
        return Err(AppError::config(
            "retention.original_receipt_days must be between 1 and 30",
        ));
    }
    if cfg.database.url_credential.trim().is_empty() {
        return Err(AppError::config(
            "database.url_credential must not be empty",
        ));
    }
    if cfg.zalo.bot_token_credential.trim().is_empty() {
        return Err(AppError::config(
            "zalo.bot_token_credential must not be empty",
        ));
    }
    if cfg.zalo.webhook_secret_credential.trim().is_empty() {
        return Err(AppError::config(
            "zalo.webhook_secret_credential must not be empty",
        ));
    }
    Ok(())
}

/// Resolve database URL from credential file or TEST_DATABASE_URL / ZL_EXPENSE_DATABASE_URL env.
fn resolve_database_url(cfg: &Config, credentials_dir: &Path) -> Result<String, AppError> {
    if let Ok(url) = env::var("TEST_DATABASE_URL") {
        return Ok(url);
    }
    if let Ok(url) = env::var("ZL_EXPENSE_DATABASE_URL") {
        return Ok(url);
    }

    let cred_path = credentials_dir.join(&cfg.database.url_credential);
    let contents = fs::read_to_string(&cred_path).map_err(|_| {
        AppError::config(format!(
            "database credential reference '{}' not found at expected path",
            cfg.database.url_credential
        ))
    })?;
    let url = contents.trim();
    if url.is_empty() {
        return Err(AppError::config("database credential file is empty"));
    }
    Ok(url.to_string())
}
