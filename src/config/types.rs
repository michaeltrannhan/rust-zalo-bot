//! Configuration file deserialization types.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub credentials: CredentialsConfig,
    #[serde(default)]
    pub access: AccessConfig,
    #[serde(default)]
    pub zalo: ZaloConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub extraction: ExtractionConfig,
    #[serde(default)]
    pub ai: AiConfig,
    #[serde(default)]
    pub quotas: QuotasConfig,
    #[serde(default)]
    pub insights: InsightsConfig,
    #[serde(default)]
    pub features: FeaturesConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub update: UpdateConfig,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    /// Provider-issued sender identifiers permitted to create or use accounts.
    /// An empty list deliberately denies every sender.
    #[serde(default)]
    pub allowed_provider_sender_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_listen_address")]
    pub listen_address: String,
}

fn default_listen_address() -> String {
    "127.0.0.1:8080".to_string()
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            listen_address: default_listen_address(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    #[serde(default = "default_url_credential")]
    pub url_credential: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_url_credential() -> String {
    "database".to_string()
}

fn default_max_connections() -> u32 {
    5
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url_credential: default_url_credential(),
            max_connections: default_max_connections(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConcurrencyConfig {
    #[serde(default = "default_receipt_extraction")]
    pub receipt_extraction: u32,
    #[serde(default = "default_outbound_delivery")]
    pub outbound_delivery: u32,
}

fn default_receipt_extraction() -> u32 {
    1
}

fn default_outbound_delivery() -> u32 {
    4
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            receipt_extraction: default_receipt_extraction(),
            outbound_delivery: default_outbound_delivery(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionConfig {
    #[serde(default = "default_retention_days")]
    pub original_receipt_days: u32,
}

fn default_retention_days() -> u32 {
    7
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            original_receipt_days: default_retention_days(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialsConfig {
    #[serde(default = "default_credentials_dir")]
    pub directory: String,
}

fn default_credentials_dir() -> String {
    "/etc/zl-expense/credentials".to_string()
}

impl Default for CredentialsConfig {
    fn default() -> Self {
        Self {
            directory: default_credentials_dir(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZaloConfig {
    #[serde(default = "default_bot_token_credential")]
    pub bot_token_credential: String,
    #[serde(default = "default_webhook_secret_credential")]
    pub webhook_secret_credential: String,
    #[serde(default = "default_api_base")]
    pub api_base: String,
    #[serde(default = "default_send_timeout_seconds")]
    pub send_timeout_seconds: u64,
    #[serde(default = "default_webhook_max_body_bytes")]
    pub webhook_max_body_bytes: usize,
}

fn default_bot_token_credential() -> String {
    "zalo-bot".to_string()
}

fn default_webhook_secret_credential() -> String {
    "webhook-secret".to_string()
}

fn default_api_base() -> String {
    "https://bot-api.zaloplatforms.com".to_string()
}

fn default_send_timeout_seconds() -> u64 {
    10
}

fn default_webhook_max_body_bytes() -> usize {
    1_048_576
}

impl Default for ZaloConfig {
    fn default() -> Self {
        Self {
            bot_token_credential: default_bot_token_credential(),
            webhook_secret_credential: default_webhook_secret_credential(),
            api_base: default_api_base(),
            send_timeout_seconds: default_send_timeout_seconds(),
            webhook_max_body_bytes: default_webhook_max_body_bytes(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    #[serde(default = "default_storage_backend")]
    pub backend: String,
    #[serde(default = "default_storage_directory")]
    pub directory: String,
    pub endpoint: Option<String>,
    pub bucket: Option<String>,
    #[serde(default = "default_storage_region")]
    pub region: String,
    pub access_key_credential: Option<String>,
    pub secret_key_credential: Option<String>,
    #[serde(default = "default_force_path_style")]
    pub force_path_style: bool,
}

fn default_storage_backend() -> String {
    "filesystem".to_string()
}

fn default_storage_directory() -> String {
    "/var/lib/zl-expense/objects".to_string()
}

fn default_storage_region() -> String {
    "us-east-1".to_string()
}

fn default_force_path_style() -> bool {
    true
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_storage_backend(),
            directory: default_storage_directory(),
            endpoint: None,
            bucket: None,
            region: default_storage_region(),
            access_key_credential: None,
            secret_key_credential: None,
            force_path_style: default_force_path_style(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractionConfig {
    #[serde(default = "default_extraction_backend")]
    pub backend: String,
    #[serde(default = "default_extraction_profile")]
    pub default_profile: String,
    #[serde(default = "default_gemini_api_base")]
    pub api_base: String,
}

fn default_extraction_backend() -> String {
    "fake".to_string()
}

fn default_extraction_profile() -> String {
    "receipt-fast".to_string()
}

fn default_gemini_api_base() -> String {
    "https://generativelanguage.googleapis.com".to_string()
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            backend: default_extraction_backend(),
            default_profile: default_extraction_profile(),
            api_base: default_gemini_api_base(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AiConfig {
    #[serde(default)]
    pub profiles: Vec<AiProfileConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AiProfileConfig {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub credential: String,
    pub task: String,
    #[serde(default = "default_ai_timeout_seconds")]
    pub timeout_seconds: u64,
    #[serde(default = "default_ai_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_ai_max_input_bytes")]
    pub max_input_bytes: usize,
    #[serde(default = "default_ai_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_thinking_effort")]
    pub thinking_effort: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: String,
}

fn default_ai_timeout_seconds() -> u64 {
    30
}

fn default_ai_max_attempts() -> u32 {
    3
}

fn default_ai_max_input_bytes() -> usize {
    4_194_304
}

fn default_ai_max_output_tokens() -> u32 {
    2048
}

fn default_thinking_effort() -> String {
    "none".to_string()
}

fn default_schema_version() -> String {
    "v1".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotasConfig {
    #[serde(default = "default_per_user_daily_receipts")]
    pub per_user_daily_receipts: u64,
    #[serde(default = "default_monthly_extraction_pages")]
    pub monthly_extraction_pages: u64,
    #[serde(default = "default_zalo_monthly_messages")]
    pub zalo_monthly_messages: u64,
    #[serde(default = "default_monthly_insight_narratives")]
    pub monthly_insight_narratives: u64,
}

fn default_per_user_daily_receipts() -> u64 {
    20
}

fn default_monthly_extraction_pages() -> u64 {
    80
}

fn default_zalo_monthly_messages() -> u64 {
    3000
}

fn default_monthly_insight_narratives() -> u64 {
    30
}

impl Default for QuotasConfig {
    fn default() -> Self {
        Self {
            per_user_daily_receipts: default_per_user_daily_receipts(),
            monthly_extraction_pages: default_monthly_extraction_pages(),
            zalo_monthly_messages: default_zalo_monthly_messages(),
            monthly_insight_narratives: default_monthly_insight_narratives(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InsightsConfig {
    #[serde(default = "default_insights_llm_enabled")]
    pub llm_enabled: bool,
}

fn default_insights_llm_enabled() -> bool {
    false
}

impl Default for InsightsConfig {
    fn default() -> Self {
        Self {
            llm_enabled: default_insights_llm_enabled(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeaturesConfig {
    #[serde(default = "default_extraction_enabled")]
    pub extraction_enabled: bool,
    #[serde(default = "default_outbound_enabled")]
    pub outbound_enabled: bool,
}

fn default_extraction_enabled() -> bool {
    true
}

fn default_outbound_enabled() -> bool {
    true
}

impl Default for FeaturesConfig {
    fn default() -> Self {
        Self {
            extraction_enabled: default_extraction_enabled(),
            outbound_enabled: default_outbound_enabled(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    #[serde(default = "default_metrics_enabled")]
    pub enabled: bool,
    #[serde(default = "default_metrics_bind")]
    pub bind: String,
}

fn default_metrics_enabled() -> bool {
    false
}

fn default_metrics_bind() -> String {
    "127.0.0.1:9090".to_string()
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: default_metrics_enabled(),
            bind: default_metrics_bind(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateConfig {
    #[serde(default = "default_update_public_keys_directory")]
    pub public_keys_directory: String,
    #[serde(default = "default_update_install_path")]
    pub install_path: String,
    #[serde(default = "default_update_state_directory")]
    pub state_directory: String,
}

fn default_update_public_keys_directory() -> String {
    "/etc/zl-expense/update-keys".to_string()
}

fn default_update_install_path() -> String {
    "/usr/bin/zl-expense".to_string()
}

fn default_update_state_directory() -> String {
    "/var/lib/zl-expense/update".to_string()
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            public_keys_directory: default_update_public_keys_directory(),
            install_path: default_update_install_path(),
            state_directory: default_update_state_directory(),
        }
    }
}
