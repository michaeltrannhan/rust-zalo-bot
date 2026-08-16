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

/// Storage backend selection for receipt originals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageBackend {
    Filesystem,
    S3,
    Memory,
}

impl StorageBackend {
    fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "filesystem" => Ok(Self::Filesystem),
            "s3" => Ok(Self::S3),
            "memory" => Ok(Self::Memory),
            _ => Err(AppError::config(
                "storage.backend must be filesystem, s3, or memory",
            )),
        }
    }
}

/// Extraction backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractionBackend {
    Fake,
    Gemini,
}

impl ExtractionBackend {
    fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fake" => Ok(Self::Fake),
            "gemini" => Ok(Self::Gemini),
            _ => Err(AppError::config(
                "extraction.backend must be fake or gemini",
            )),
        }
    }
}

/// Named AI profile after config-load validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAiProfile {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub credential: String,
    pub task: String,
    pub timeout_seconds: u64,
    pub max_attempts: u32,
    pub max_input_bytes: usize,
    pub max_output_tokens: u32,
    pub thinking_effort: String,
    pub schema_version: String,
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
    pub storage_backend: StorageBackend,
    pub storage_directory: PathBuf,
    pub storage_endpoint: Option<String>,
    pub storage_bucket: Option<String>,
    pub storage_region: String,
    pub storage_access_key_credential: Option<String>,
    pub storage_secret_key_credential: Option<String>,
    pub storage_force_path_style: bool,
    pub extraction_backend: ExtractionBackend,
    pub extraction_default_profile: String,
    pub gemini_api_base: String,
    pub ai_profiles: Vec<ResolvedAiProfile>,
    pub per_user_daily_receipts: u64,
    pub monthly_extraction_pages: u64,
    pub zalo_monthly_messages: u64,
    pub monthly_insight_narratives: u64,
    pub insights_llm_enabled: bool,
    pub extraction_enabled: bool,
    pub outbound_enabled: bool,
    pub metrics_enabled: bool,
    pub metrics_bind: String,
    pub update_public_keys_directory: PathBuf,
    pub update_install_path: PathBuf,
    pub update_state_directory: PathBuf,
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
            .field("storage_backend", &self.storage_backend)
            .field("storage_directory", &self.storage_directory)
            .field(
                "storage_endpoint",
                &self
                    .storage_endpoint
                    .as_ref()
                    .map(|_| "[REDACTED]")
                    .unwrap_or("[unset]"),
            )
            .field("storage_bucket", &self.storage_bucket)
            .field("storage_region", &self.storage_region)
            .field(
                "storage_access_key_credential",
                &self.storage_access_key_credential,
            )
            .field(
                "storage_secret_key_credential",
                &self.storage_secret_key_credential,
            )
            .field("storage_force_path_style", &self.storage_force_path_style)
            .field("extraction_backend", &self.extraction_backend)
            .field(
                "extraction_default_profile",
                &self.extraction_default_profile,
            )
            .field("gemini_api_base", &self.gemini_api_base)
            .field(
                "ai_profiles",
                &format_args!("[{} profiles]", self.ai_profiles.len()),
            )
            .field("per_user_daily_receipts", &self.per_user_daily_receipts)
            .field("monthly_extraction_pages", &self.monthly_extraction_pages)
            .field("zalo_monthly_messages", &self.zalo_monthly_messages)
            .field(
                "monthly_insight_narratives",
                &self.monthly_insight_narratives,
            )
            .field("insights_llm_enabled", &self.insights_llm_enabled)
            .field("extraction_enabled", &self.extraction_enabled)
            .field("outbound_enabled", &self.outbound_enabled)
            .field("metrics_enabled", &self.metrics_enabled)
            .field("metrics_bind", &self.metrics_bind)
            .field(
                "update_public_keys_directory",
                &self.update_public_keys_directory,
            )
            .field("update_install_path", &self.update_install_path)
            .field("update_state_directory", &self.update_state_directory)
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

    pub fn read_storage_access_key(&self) -> Result<String, AppError> {
        let reference = self
            .storage_access_key_credential
            .as_ref()
            .ok_or_else(|| AppError::config("storage access key credential is unavailable"))?;
        self.read_credential(reference)
    }

    pub fn read_storage_secret_key(&self) -> Result<String, AppError> {
        let reference = self
            .storage_secret_key_credential
            .as_ref()
            .ok_or_else(|| AppError::config("storage secret key credential is unavailable"))?;
        self.read_credential(reference)
    }

    pub fn read_named_credential(&self, reference: &str) -> Result<String, AppError> {
        self.read_credential(reference)
    }

    pub fn extraction_profile(&self, name: &str) -> Option<&ResolvedAiProfile> {
        self.ai_profiles.iter().find(|profile| profile.name == name)
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

    let storage_backend = StorageBackend::parse(&cfg.storage.backend)?;
    let storage_directory = PathBuf::from(&cfg.storage.directory);
    let extraction_backend = ExtractionBackend::parse(&cfg.extraction.backend)?;
    let ai_profiles = resolve_ai_profiles(&cfg)?;

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
        storage_backend,
        storage_directory,
        storage_endpoint: cfg.storage.endpoint.clone(),
        storage_bucket: cfg.storage.bucket.clone(),
        storage_region: cfg.storage.region.clone(),
        storage_access_key_credential: cfg.storage.access_key_credential.clone(),
        storage_secret_key_credential: cfg.storage.secret_key_credential.clone(),
        storage_force_path_style: cfg.storage.force_path_style,
        extraction_backend,
        extraction_default_profile: cfg.extraction.default_profile.clone(),
        gemini_api_base: cfg.extraction.api_base.trim_end_matches('/').to_string(),
        ai_profiles,
        per_user_daily_receipts: cfg.quotas.per_user_daily_receipts,
        monthly_extraction_pages: cfg.quotas.monthly_extraction_pages,
        zalo_monthly_messages: cfg.quotas.zalo_monthly_messages,
        monthly_insight_narratives: cfg.quotas.monthly_insight_narratives,
        insights_llm_enabled: cfg.insights.llm_enabled,
        extraction_enabled: cfg.features.extraction_enabled,
        outbound_enabled: cfg.features.outbound_enabled,
        metrics_enabled: cfg.metrics.enabled,
        metrics_bind: cfg.metrics.bind.clone(),
        update_public_keys_directory: PathBuf::from(&cfg.update.public_keys_directory),
        update_install_path: PathBuf::from(&cfg.update.install_path),
        update_state_directory: PathBuf::from(&cfg.update.state_directory),
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
    target.storage = source.storage.clone();
    target.extraction = source.extraction.clone();
    target.ai = source.ai.clone();
    target.quotas = source.quotas.clone();
    target.insights = source.insights.clone();
    target.features = source.features.clone();
    target.metrics = source.metrics.clone();
    target.update = source.update.clone();
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
    insert_attr(
        attribution,
        "storage.backend",
        defaults.storage.backend,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "storage.directory",
        defaults.storage.directory,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "storage.region",
        defaults.storage.region,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "storage.force_path_style",
        defaults.storage.force_path_style,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "extraction.backend",
        defaults.extraction.backend,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "extraction.default_profile",
        defaults.extraction.default_profile,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "extraction.api_base",
        defaults.extraction.api_base,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "ai.profiles",
        serde_json::json!({ "count": defaults.ai.profiles.len() }),
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "quotas.per_user_daily_receipts",
        defaults.quotas.per_user_daily_receipts,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "quotas.monthly_extraction_pages",
        defaults.quotas.monthly_extraction_pages,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "quotas.zalo_monthly_messages",
        defaults.quotas.zalo_monthly_messages,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "quotas.monthly_insight_narratives",
        defaults.quotas.monthly_insight_narratives,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "insights.llm_enabled",
        defaults.insights.llm_enabled,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "features.extraction_enabled",
        defaults.features.extraction_enabled,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "features.outbound_enabled",
        defaults.features.outbound_enabled,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "metrics.enabled",
        defaults.metrics.enabled,
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "metrics.bind",
        defaults.metrics.bind.clone(),
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "update.public_keys_directory",
        defaults.update.public_keys_directory.clone(),
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "update.install_path",
        defaults.update.install_path.clone(),
        ConfigSource::Default,
    );
    insert_attr(
        attribution,
        "update.state_directory",
        defaults.update.state_directory.clone(),
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
    record!("storage", "backend", cfg.storage.backend.clone());
    record!("storage", "directory", cfg.storage.directory.clone());
    if let Some(endpoint) = &cfg.storage.endpoint {
        insert_attr(
            attribution,
            "storage.endpoint",
            "[REDACTED]",
            ConfigSource::File,
        );
        let _ = endpoint;
    }
    record!("storage", "bucket", cfg.storage.bucket.clone());
    record!("storage", "region", cfg.storage.region.clone());
    if cfg.storage.access_key_credential.is_some() {
        insert_attr(
            attribution,
            "storage.access_key_credential",
            cfg.storage.access_key_credential.clone(),
            ConfigSource::File,
        );
    }
    if cfg.storage.secret_key_credential.is_some() {
        insert_attr(
            attribution,
            "storage.secret_key_credential",
            cfg.storage.secret_key_credential.clone(),
            ConfigSource::File,
        );
    }
    record!("storage", "force_path_style", cfg.storage.force_path_style);
    record!("extraction", "backend", cfg.extraction.backend.clone());
    record!(
        "extraction",
        "default_profile",
        cfg.extraction.default_profile.clone()
    );
    record!("extraction", "api_base", cfg.extraction.api_base.clone());
    if document.get("ai").is_some() {
        insert_attr(
            attribution,
            "ai.profiles",
            serde_json::json!({ "count": cfg.ai.profiles.len() }),
            ConfigSource::File,
        );
    }
    record!(
        "quotas",
        "per_user_daily_receipts",
        cfg.quotas.per_user_daily_receipts
    );
    record!(
        "quotas",
        "monthly_extraction_pages",
        cfg.quotas.monthly_extraction_pages
    );
    record!(
        "quotas",
        "zalo_monthly_messages",
        cfg.quotas.zalo_monthly_messages
    );
    record!(
        "quotas",
        "monthly_insight_narratives",
        cfg.quotas.monthly_insight_narratives
    );
    record!("insights", "llm_enabled", cfg.insights.llm_enabled);
    record!(
        "features",
        "extraction_enabled",
        cfg.features.extraction_enabled
    );
    record!(
        "features",
        "outbound_enabled",
        cfg.features.outbound_enabled
    );
    record!("metrics", "enabled", cfg.metrics.enabled);
    record!("metrics", "bind", cfg.metrics.bind.clone());
    record!(
        "update",
        "public_keys_directory",
        cfg.update.public_keys_directory.clone()
    );
    record!("update", "install_path", cfg.update.install_path.clone());
    record!(
        "update",
        "state_directory",
        cfg.update.state_directory.clone()
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
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_BACKEND") {
        cfg.storage.backend = v;
        insert_attr(
            attribution,
            "storage.backend",
            cfg.storage.backend.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_DIRECTORY") {
        cfg.storage.directory = v;
        insert_attr(
            attribution,
            "storage.directory",
            cfg.storage.directory.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_ENDPOINT") {
        cfg.storage.endpoint = Some(v);
        insert_attr(
            attribution,
            "storage.endpoint",
            "[REDACTED]",
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_BUCKET") {
        cfg.storage.bucket = Some(v);
        insert_attr(
            attribution,
            "storage.bucket",
            cfg.storage.bucket.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_REGION") {
        cfg.storage.region = v;
        insert_attr(
            attribution,
            "storage.region",
            cfg.storage.region.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_ACCESS_KEY_CREDENTIAL") {
        cfg.storage.access_key_credential = Some(v);
        insert_attr(
            attribution,
            "storage.access_key_credential",
            cfg.storage.access_key_credential.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_SECRET_KEY_CREDENTIAL") {
        cfg.storage.secret_key_credential = Some(v);
        insert_attr(
            attribution,
            "storage.secret_key_credential",
            cfg.storage.secret_key_credential.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_STORAGE_FORCE_PATH_STYLE") {
        cfg.storage.force_path_style = parse_env_bool("ZL_EXPENSE_STORAGE_FORCE_PATH_STYLE", &v)?;
        insert_attr(
            attribution,
            "storage.force_path_style",
            cfg.storage.force_path_style,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_EXTRACTION_BACKEND") {
        cfg.extraction.backend = v;
        insert_attr(
            attribution,
            "extraction.backend",
            cfg.extraction.backend.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_EXTRACTION_DEFAULT_PROFILE") {
        cfg.extraction.default_profile = v;
        insert_attr(
            attribution,
            "extraction.default_profile",
            cfg.extraction.default_profile.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_GEMINI_API_BASE") {
        cfg.extraction.api_base = v;
        insert_attr(
            attribution,
            "extraction.api_base",
            cfg.extraction.api_base.clone(),
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_PER_USER_DAILY_RECEIPTS") {
        cfg.quotas.per_user_daily_receipts =
            parse_env_u64("ZL_EXPENSE_PER_USER_DAILY_RECEIPTS", &v)?;
        insert_attr(
            attribution,
            "quotas.per_user_daily_receipts",
            cfg.quotas.per_user_daily_receipts,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_MONTHLY_EXTRACTION_PAGES") {
        cfg.quotas.monthly_extraction_pages =
            parse_env_u64("ZL_EXPENSE_MONTHLY_EXTRACTION_PAGES", &v)?;
        insert_attr(
            attribution,
            "quotas.monthly_extraction_pages",
            cfg.quotas.monthly_extraction_pages,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_ZALO_MONTHLY_MESSAGES") {
        cfg.quotas.zalo_monthly_messages = parse_env_u64("ZL_EXPENSE_ZALO_MONTHLY_MESSAGES", &v)?;
        insert_attr(
            attribution,
            "quotas.zalo_monthly_messages",
            cfg.quotas.zalo_monthly_messages,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_MONTHLY_INSIGHT_NARRATIVES") {
        cfg.quotas.monthly_insight_narratives =
            parse_env_u64("ZL_EXPENSE_MONTHLY_INSIGHT_NARRATIVES", &v)?;
        insert_attr(
            attribution,
            "quotas.monthly_insight_narratives",
            cfg.quotas.monthly_insight_narratives,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_INSIGHTS_LLM_ENABLED") {
        cfg.insights.llm_enabled = parse_env_bool("ZL_EXPENSE_INSIGHTS_LLM_ENABLED", &v)?;
        insert_attr(
            attribution,
            "insights.llm_enabled",
            cfg.insights.llm_enabled,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_EXTRACTION_ENABLED") {
        cfg.features.extraction_enabled = parse_env_bool("ZL_EXPENSE_EXTRACTION_ENABLED", &v)?;
        insert_attr(
            attribution,
            "features.extraction_enabled",
            cfg.features.extraction_enabled,
            ConfigSource::Env,
        );
    }
    if let Ok(v) = env::var("ZL_EXPENSE_OUTBOUND_ENABLED") {
        cfg.features.outbound_enabled = parse_env_bool("ZL_EXPENSE_OUTBOUND_ENABLED", &v)?;
        insert_attr(
            attribution,
            "features.outbound_enabled",
            cfg.features.outbound_enabled,
            ConfigSource::Env,
        );
    }
    Ok(())
}

fn parse_env_bool(name: &str, value: &str) -> Result<bool, AppError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(AppError::config(format!("{} must be a boolean", name))),
    }
}

fn parse_env_u32(name: &str, value: &str) -> Result<u32, AppError> {
    value
        .parse()
        .map_err(|_| AppError::config(format!("{} must be a positive integer", name)))
}

fn parse_env_u64(name: &str, value: &str) -> Result<u64, AppError> {
    let parsed = value
        .parse()
        .map_err(|_| AppError::config(format!("{} must be a positive integer", name)))?;
    if parsed == 0 {
        return Err(AppError::config(format!(
            "{} must be greater than zero",
            name
        )));
    }
    Ok(parsed)
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
    let backend = StorageBackend::parse(&cfg.storage.backend)?;
    match backend {
        StorageBackend::Filesystem => {
            if cfg.storage.directory.trim().is_empty() {
                return Err(AppError::config("storage.directory must not be empty"));
            }
        }
        StorageBackend::S3 => {
            if !cfg.storage.force_path_style {
                return Err(AppError::config(
                    "storage.force_path_style must be true; virtual-hosted-style S3 is not supported",
                ));
            }
            let endpoint = cfg.storage.endpoint.as_deref().map(str::trim).unwrap_or("");
            if endpoint.is_empty() {
                return Err(AppError::config(
                    "storage.endpoint is required for s3 backend",
                ));
            }
            validate_storage_endpoint(endpoint)?;
            let bucket = cfg.storage.bucket.as_deref().map(str::trim).unwrap_or("");
            if bucket.is_empty() {
                return Err(AppError::config(
                    "storage.bucket is required for s3 backend",
                ));
            }
            let access_key = cfg
                .storage
                .access_key_credential
                .as_deref()
                .map(str::trim)
                .unwrap_or("");
            let secret_key = cfg
                .storage
                .secret_key_credential
                .as_deref()
                .map(str::trim)
                .unwrap_or("");
            if !valid_credential_reference(access_key) {
                return Err(AppError::config(
                    "storage.access_key_credential must be a safe credential name",
                ));
            }
            if !valid_credential_reference(secret_key) {
                return Err(AppError::config(
                    "storage.secret_key_credential must be a safe credential name",
                ));
            }
        }
        StorageBackend::Memory => {}
    }
    if cfg.storage.region.trim().is_empty() {
        return Err(AppError::config("storage.region must not be empty"));
    }
    validate_extraction(cfg)?;
    if cfg.quotas.per_user_daily_receipts == 0 {
        return Err(AppError::config(
            "quotas.per_user_daily_receipts must be greater than zero",
        ));
    }
    if cfg.quotas.monthly_extraction_pages == 0 {
        return Err(AppError::config(
            "quotas.monthly_extraction_pages must be greater than zero",
        ));
    }
    if cfg.quotas.zalo_monthly_messages == 0 {
        return Err(AppError::config(
            "quotas.zalo_monthly_messages must be greater than zero",
        ));
    }
    if cfg.quotas.monthly_insight_narratives == 0 {
        return Err(AppError::config(
            "quotas.monthly_insight_narratives must be greater than zero",
        ));
    }
    Ok(())
}

fn validate_extraction(cfg: &Config) -> Result<(), AppError> {
    let backend = ExtractionBackend::parse(&cfg.extraction.backend)?;
    validate_https_or_loopback(
        &cfg.extraction.api_base,
        "extraction.api_base must use HTTPS (HTTP is allowed only for a loopback IP)",
    )?;
    if cfg.extraction.default_profile.trim().is_empty() {
        return Err(AppError::config(
            "extraction.default_profile must not be empty",
        ));
    }
    let mut names = BTreeSet::new();
    for profile in &cfg.ai.profiles {
        if profile.name.trim().is_empty() || !names.insert(profile.name.clone()) {
            return Err(AppError::config(
                "ai.profiles names must be unique and non-empty",
            ));
        }
        if profile.provider != "gemini" {
            return Err(AppError::config(
                "ai.profiles provider must be gemini in the first release",
            ));
        }
        if profile.model.trim().is_empty() {
            return Err(AppError::config("ai.profiles model must not be empty"));
        }
        if !valid_credential_reference(&profile.credential) {
            return Err(AppError::config(
                "ai.profiles credential must be a safe credential name",
            ));
        }
        if profile.task != "extraction" && profile.task != "insight" {
            return Err(AppError::config(
                "ai.profiles task must be extraction or insight",
            ));
        }
        if profile.timeout_seconds == 0 || profile.timeout_seconds > 120 {
            return Err(AppError::config(
                "ai.profiles timeout_seconds must be between 1 and 120",
            ));
        }
        if profile.max_attempts == 0 || profile.max_attempts > 10 {
            return Err(AppError::config(
                "ai.profiles max_attempts must be between 1 and 10",
            ));
        }
        if profile.max_input_bytes == 0 || profile.max_input_bytes > 10 * 1024 * 1024 {
            return Err(AppError::config(
                "ai.profiles max_input_bytes must be between 1 and 10485760",
            ));
        }
        if profile.max_output_tokens == 0 || profile.max_output_tokens > 8192 {
            return Err(AppError::config(
                "ai.profiles max_output_tokens must be between 1 and 8192",
            ));
        }
        validate_thinking_effort(&profile.model, &profile.thinking_effort)?;
        if profile.schema_version.trim().is_empty() {
            return Err(AppError::config(
                "ai.profiles schema_version must not be empty",
            ));
        }
    }
    if backend == ExtractionBackend::Gemini {
        let profile = cfg
            .ai
            .profiles
            .iter()
            .find(|profile| profile.name == cfg.extraction.default_profile)
            .ok_or_else(|| {
                AppError::config(
                    "extraction.default_profile must name a configured ai.profiles entry",
                )
            })?;
        if profile.task != "extraction" || profile.provider != "gemini" {
            return Err(AppError::config(
                "extraction.default_profile must be a gemini extraction profile",
            ));
        }
    }
    Ok(())
}

fn validate_thinking_effort(model: &str, effort: &str) -> Result<(), AppError> {
    match effort {
        "none" => Ok(()),
        "low" | "medium" | "high" => {
            if model_supports_thinking(model) {
                Ok(())
            } else {
                Err(AppError::config(
                    "ai.profiles thinking_effort is not supported for this model",
                ))
            }
        }
        _ => Err(AppError::config(
            "ai.profiles thinking_effort must be none, low, medium, or high",
        )),
    }
}

fn model_supports_thinking(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("gemini-2.5") || model.contains("thinking")
}

fn resolve_ai_profiles(cfg: &Config) -> Result<Vec<ResolvedAiProfile>, AppError> {
    Ok(cfg
        .ai
        .profiles
        .iter()
        .map(|profile| ResolvedAiProfile {
            name: profile.name.clone(),
            provider: profile.provider.clone(),
            model: profile.model.clone(),
            credential: profile.credential.clone(),
            task: profile.task.clone(),
            timeout_seconds: profile.timeout_seconds,
            max_attempts: profile.max_attempts,
            max_input_bytes: profile.max_input_bytes,
            max_output_tokens: profile.max_output_tokens,
            thinking_effort: profile.thinking_effort.clone(),
            schema_version: profile.schema_version.clone(),
        })
        .collect())
}

fn validate_https_or_loopback(value: &str, message: &str) -> Result<(), AppError> {
    let url = reqwest::Url::parse(value).map_err(|_| AppError::config(message))?;
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
        return Err(AppError::config(message));
    }
    Ok(())
}

fn validate_storage_endpoint(value: &str) -> Result<(), AppError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| AppError::config("storage.endpoint must be a valid URL"))?;
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
            "storage.endpoint must use HTTPS (HTTP is allowed only for a loopback IP)",
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
