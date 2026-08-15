//! Configuration file deserialization types.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
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
    pub zalo: ZaloConfig,
}

#[derive(Debug, Clone, Deserialize)]
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
pub struct ZaloConfig {
    #[serde(default = "default_bot_token_credential")]
    pub bot_token_credential: String,
    #[serde(default = "default_webhook_secret_credential")]
    pub webhook_secret_credential: String,
}

fn default_bot_token_credential() -> String {
    "zalo-bot".to_string()
}

fn default_webhook_secret_credential() -> String {
    "webhook-secret".to_string()
}

impl Default for ZaloConfig {
    fn default() -> Self {
        Self {
            bot_token_credential: default_bot_token_credential(),
            webhook_secret_credential: default_webhook_secret_credential(),
        }
    }
}
