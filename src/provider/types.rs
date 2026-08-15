//! Normalized provider domain types (no wire shapes).

use std::time::Duration;

use chrono::{DateTime, Utc};

pub const DEFAULT_API_BASE: &str = "https://bot-api.zaloplatforms.com";
pub const SECRET_HEADER: &str = "X-Bot-Api-Secret-Token";
pub const EVENT_TEXT_RECEIVED: &str = "message.text.received";

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMessageResult {
    pub provider_message_id: String,
}
