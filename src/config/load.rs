//! Configuration loading with env overrides and source attribution.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;
use sqlx::postgres::PgConnectOptions;

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
#[derive(Clone)]
pub struct ResolvedConfig {
    pub listen_address: String,
    pub database_url: String,
    pub max_connections: u32,
    pub receipt_extraction: u32,
    pub outbound_delivery: u32,
    pub original_receipt_days: u32,
    pub credentials_directory: PathBuf,
    pub allowed_provider_sender_ids: BTreeSet<String>,
    pub bot_token_credential: String,
    pub webhook_secret_credential: String,
    pub zalo_api_base: String,
    pub zalo_send_timeout_seconds: u64,
    pub webhook_max_body_bytes: usize,
    pub attribution: BTreeMap<String, ResolvedValue>,
}

impl std::fmt::Debug for ResolvedConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedConfig")
            .field("listen_address", &self.listen_address)
            .field("database_url", &"[REDACTED]")
            .field("max_connections", &self.max_connections)
            .field("receipt_extraction", &self.receipt_extraction)
            .field("outbound_delivery", &self.outbound_delivery)
            .field("original_receipt_days", &self.original_receipt_days)
            .field("credentials_directory", &self.credentials_directory)
            .field(
                "allowed_provider_sender_ids",
                &format_args!(
                    "[REDACTED; {} entries]",
                    self.allowed_provider_sender_ids.len()
                ),
            )
            .field("bot_token_credential", &self.bot_token_credential)
            .field("webhook_secret_credential", &self.webhook_secret_credential)
            .field("zalo_api_base", &self.zalo_api_base)
            .field("zalo_send_timeout_seconds", &self.zalo_send_timeout_seconds)
            .field("webhook_max_body_bytes", &self.webhook_max_body_bytes)
            .field("attribution", &self.attribution)
            .finish()
    }
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

    pub fn is_provider_sender_allowed(&self, provider_sender_id: &str) -> bool {
        self.allowed_provider_sender_ids
            .contains(provider_sender_id)
    }

    pub fn read_zalo_bot_token(&self) -> Result<String, AppError> {
        self.read_credential(&self.bot_token_credential)
    }

    pub fn read_webhook_secret(&self) -> Result<String, AppError> {
        self.read_credential(&self.webhook_secret_credential)
    }

    fn read_credential(&self, reference: &str) -> Result<String, AppError> {
        let value = fs::read_to_string(self.credentials_directory.join(reference))
            .map_err(|_| AppError::config("required credential is unavailable"))?;
        let value = value.trim();
        if value.is_empty() {
            return Err(AppError::config("required credential is unavailable"));
        }
        Ok(value.to_string())
    }
}

/// Load configuration from file path with env overrides for CI.
pub fn load_config(config_path: Option<&Path>) -> Result<ResolvedConfig, AppError> {
    let mut attribution = BTreeMap::new();
    let mut cfg = Config::default();
    record_default_sources(&mut attribution);

    if let Some(path) = config_path {
        let contents =
            fs::read_to_string(path).map_err(|_| AppError::config("failed to read config file"))?;
        let document: toml::Value =
            toml::from_str(&contents).map_err(|_| AppError::config("invalid config TOML"))?;
        let file_cfg: Config =
            toml::from_str(&contents).map_err(|_| AppError::config("invalid config TOML"))?;
        merge_config(&mut cfg, &file_cfg);
        record_file_sources(&mut attribution, &file_cfg, &document);
    }

    apply_env_overrides(&mut cfg, &mut attribution)?;

    validate_resolved(&cfg)?;

    let credentials_dir = PathBuf::from(&cfg.credentials.directory);
    let database_url = resolve_database_url(&cfg, &credentials_dir)?;
    PgConnectOptions::from_str(&database_url)
        .map_err(|_| AppError::config("database credential is not a valid PostgreSQL URL"))?;

    let resolved = ResolvedConfig {
        listen_address: cfg.server.listen_address.clone(),
        database_url,
        max_connections: cfg.database.max_connections,
        receipt_extraction: cfg.concurrency.receipt_extraction,
        outbound_delivery: cfg.concurrency.outbound_delivery,
        original_receipt_days: cfg.retention.original_receipt_days,
        credentials_directory: credentials_dir,
        allowed_provider_sender_ids: cfg
            .access
            .allowed_provider_sender_ids
            .iter()
            .cloned()
            .collect(),
        bot_token_credential: cfg.zalo.bot_token_credential.clone(),
        webhook_secret_credential: cfg.zalo.webhook_secret_credential.clone(),
        zalo_api_base: cfg.zalo.api_base.trim_end_matches('/').to_string(),
        zalo_send_timeout_seconds: cfg.zalo.send_timeout_seconds,
        webhook_max_body_bytes: cfg.zalo.webhook_max_body_bytes,
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
    target.access = source.access.clone();
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
    insert_allowlist_attr(
        attribution,
        &defaults.access.allowed_provider_sender_ids,
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
    insert_attr(
        attribution,
        "zalo.api_base",
        defaults.zalo.api_base,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "zalo.send_timeout_seconds",
        defaults.zalo.send_timeout_seconds,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "zalo.webhook_max_body_bytes",
        defaults.zalo.webhook_max_body_bytes,
        ConfigSource::Default,
    );
}

fn record_file_sources(
    attribution: &mut BTreeMap<String, ResolvedValue>,
    cfg: &Config,
    document: &toml::Value,
) {
    macro_rules! record {
        ($section:literal, $field:literal, $value:expr) => {
            if document
                .get($section)
                .and_then(|section| section.get($field))
                .is_some()
            {
                insert_attr(
                    attribution,
                    concat!($section, ".", $field),
                    $value,
                    ConfigSource::File,
                );
            }
        };
    }

    record!(
        "server",
        "listen_address",
        cfg.server.listen_address.clone()
    );
    record!(
        "database",
        "url_credential",
        cfg.database.url_credential.clone()
    );
    record!("database", "max_connections", cfg.database.max_connections);
    record!(
        "concurrency",
        "receipt_extraction",
        cfg.concurrency.receipt_extraction
    );
    record!(
        "concurrency",
        "outbound_delivery",
        cfg.concurrency.outbound_delivery
    );
    record!(
        "retention",
        "original_receipt_days",
        cfg.retention.original_receipt_days
    );
    record!(
        "credentials",
        "directory",
        cfg.credentials.directory.clone()
    );
    if document
        .get("access")
        .and_then(|section| section.get("allowed_provider_sender_ids"))
        .is_some()
    {
        insert_allowlist_attr(
            attribution,
            &cfg.access.allowed_provider_sender_ids,
            ConfigSource::File,
        );
    }
    record!(
        "zalo",
        "bot_token_credential",
        cfg.zalo.bot_token_credential.clone()
    );
    record!(
        "zalo",
        "webhook_secret_credential",
        cfg.zalo.webhook_secret_credential.clone()
    );
    record!("zalo", "api_base", cfg.zalo.api_base.clone());
    record!(
        "zalo",
        "send_timeout_seconds",
        cfg.zalo.send_timeout_seconds
    );
    record!(
        "zalo",
        "webhook_max_body_bytes",
        cfg.zalo.webhook_max_body_bytes
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
    if let Ok(v) = env::var("ZL_EXPENSE_ALLOWED_PROVIDER_SENDER_IDS") {
        cfg.access.allowed_provider_sender_ids = v
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        insert_allowlist_attr(
            attribution,
            &cfg.access.allowed_provider_sender_ids,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_ZALO_API_BASE") {
        cfg.zalo.api_base = v;
        insert_attr(
            attribution,
            "zalo.api_base",
            cfg.zalo.api_base.clone(),
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

fn insert_allowlist_attr(
    attribution: &mut BTreeMap<String, ResolvedValue>,
    values: &[String],
    source: ConfigSource,
) {
    insert_attr(
        attribution,
        "access.allowed_provider_sender_ids",
        serde_json::json!({ "count": values.len() }),
        source,
    );
}

fn validate_resolved(cfg: &Config) -> Result<(), AppError> {
    if cfg
        .server
        .listen_address
        .parse::<std::net::SocketAddr>()
        .is_err()
    {
        return Err(AppError::config(
            "server.listen_address must be a valid IP socket address",
        ));
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
    if !valid_credential_reference(&cfg.database.url_credential) {
        return Err(AppError::config(
            "database.url_credential must be a safe credential name",
        ));
    }
    if !valid_credential_reference(&cfg.zalo.bot_token_credential) {
        return Err(AppError::config(
            "zalo.bot_token_credential must be a safe credential name",
        ));
    }
    if !valid_credential_reference(&cfg.zalo.webhook_secret_credential) {
        return Err(AppError::config(
            "zalo.webhook_secret_credential must be a safe credential name",
        ));
    }
    if cfg.credentials.directory.trim().is_empty() {
        return Err(AppError::config("credentials.directory must not be empty"));
    }
    if cfg
        .access
        .allowed_provider_sender_ids
        .iter()
        .any(|value| !valid_provider_sender_id(value))
    {
        return Err(AppError::config(
            "access.allowed_provider_sender_ids contains an invalid identifier",
        ));
    }
    validate_zalo_api_base(&cfg.zalo.api_base)?;
    if cfg.zalo.send_timeout_seconds == 0 || cfg.zalo.send_timeout_seconds > 60 {
        return Err(AppError::config(
            "zalo.send_timeout_seconds must be between 1 and 60",
        ));
    }
    if cfg.zalo.webhook_max_body_bytes == 0 || cfg.zalo.webhook_max_body_bytes > 1_048_576 {
        return Err(AppError::config(
            "zalo.webhook_max_body_bytes must be between 1 and 1048576",
        ));
    }
    Ok(())
}

fn valid_provider_sender_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 128
        && trimmed == value
        && !trimmed.chars().any(char::is_control)
}

fn validate_zalo_api_base(value: &str) -> Result<(), AppError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| AppError::config("zalo.api_base must be a valid URL"))?;
    let is_https = url.scheme() == "https";
    let is_loopback_http = url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
    if (!is_https && !is_loopback_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::config(
            "zalo.api_base must use HTTPS (HTTP is allowed only for a loopback IP)",
        ));
    }
    Ok(())
}

fn valid_credential_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
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
