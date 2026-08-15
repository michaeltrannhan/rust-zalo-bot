//! Normalized provider domain types (no wire shapes).

use std::time::Duration;

use chrono::{DateTime, Utc};

pub const DEFAULT_API_BASE: &str = "https://bot-api.zaloplatforms.com";
pub const SECRET_HEADER: &str = "X-Bot-Api-Secret-Token";
pub const EVENT_TEXT_RECEIVED: &str = "message.text.received";
pub const EVENT_IMAGE_RECEIVED: &str = "message.image.received";

pub const DEFAULT_MEDIA_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub const DEFAULT_MEDIA_TIMEOUT_SECS: u64 = 15;
pub const DEFAULT_MEDIA_MAX_REDIRECTS: u32 = 3;

pub const DEFAULT_MEDIA_HOST_SUFFIXES: [&str; 5] = [
    "zaloplatforms.com",
    "zaloapp.com",
    "zdn.vn",
    "zadn.vn",
    "zapps.me",
];

#[derive(Clone, PartialEq, Eq)]
pub struct ZaloHttpConfig {
    pub api_base: String,
    pub bot_token: String,
    pub webhook_secret: String,
    pub provider_scope: String,
    pub request_timeout: Duration,
}

impl std::fmt::Debug for ZaloHttpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZaloHttpConfig")
            .field("api_base", &self.api_base)
            .field("bot_token", &"[REDACTED]")
            .field("webhook_secret", &"[REDACTED]")
            .field("provider_scope", &self.provider_scope)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl ZaloHttpConfig {
    pub fn normalized_api_base(&self) -> String {
        self.api_base.trim_end_matches('/').to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundEventKind {
    TextReceived,
    ImageReceived,
    Unsupported(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedInboundText {
    pub provider_scope: String,
    pub event_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
    pub kind: InboundEventKind,
}

impl std::fmt::Debug for NormalizedInboundText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NormalizedInboundText")
            .field("provider_scope", &self.provider_scope)
            .field("event_id", &"[REDACTED]")
            .field("sender_id", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("text", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedInboundImage {
    pub provider_scope: String,
    pub event_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub image_url: String,
    pub caption: String,
    pub received_at: DateTime<Utc>,
    pub kind: InboundEventKind,
}

impl std::fmt::Debug for NormalizedInboundImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NormalizedInboundImage")
            .field("provider_scope", &self.provider_scope)
            .field("event_id", &"[REDACTED]")
            .field("sender_id", &"[REDACTED]")
            .field("chat_id", &"[REDACTED]")
            .field("image_url", &"[REDACTED]")
            .field("caption", &"[REDACTED]")
            .field("received_at", &self.received_at)
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MediaDownloadPolicy {
    pub host_suffixes: Vec<String>,
    pub max_bytes: u64,
    pub total_timeout: Duration,
    pub max_redirects: u32,
    pub require_https: bool,
    /// Test-only. When true, resolved addresses may be private for an
    /// allowlisted hostname. Literal private/loopback URL hosts and redirect
    /// targets remain rejected. Production default is false.
    pub permit_private_resolved_addresses: bool,
    /// Test-only. When set, this explicit URL port is accepted. Production
    /// default is `None` (HTTPS 443 / implicit scheme default only).
    pub allowed_explicit_port: Option<u16>,
}

impl std::fmt::Debug for MediaDownloadPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaDownloadPolicy")
            .field("host_suffixes", &self.host_suffixes)
            .field("max_bytes", &self.max_bytes)
            .field("total_timeout", &self.total_timeout)
            .field("max_redirects", &self.max_redirects)
            .field("require_https", &self.require_https)
            .field(
                "permit_private_resolved_addresses",
                &self.permit_private_resolved_addresses,
            )
            .field("allowed_explicit_port", &self.allowed_explicit_port)
            .finish()
    }
}

impl MediaDownloadPolicy {
    pub fn production_default() -> Self {
        Self {
            host_suffixes: DEFAULT_MEDIA_HOST_SUFFIXES
                .iter()
                .map(|suffix| (*suffix).to_string())
                .collect(),
            max_bytes: DEFAULT_MEDIA_MAX_BYTES,
            total_timeout: Duration::from_secs(DEFAULT_MEDIA_TIMEOUT_SECS),
            max_redirects: DEFAULT_MEDIA_MAX_REDIRECTS,
            require_https: true,
            permit_private_resolved_addresses: false,
            allowed_explicit_port: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct MediaDownloadResult {
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

impl std::fmt::Debug for MediaDownloadResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaDownloadResult")
            .field("bytes_len", &self.bytes.len())
            .field("content_type_present", &self.content_type.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMessageResult {
    pub provider_message_id: String,
}
