//! Normalized provider domain types (no wire shapes).

use std::time::Duration;

use chrono::{DateTime, Utc};

pub const DEFAULT_API_BASE: &str = "https://bot-api.zaloplatforms.com";
pub const SECRET_HEADER: &str = "X-Bot-Api-Secret-Token";
pub const EVENT_TEXT_RECEIVED: &str = "message.text.received";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZaloHttpConfig {
    pub api_base: String,
    pub bot_token: String,
    pub webhook_secret: String,
    pub provider_scope: String,
    pub request_timeout: Duration,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedInboundText {
    pub provider_scope: String,
    pub event_id: String,
    pub sender_id: String,
    pub chat_id: String,
    pub text: String,
    pub received_at: DateTime<Utc>,
    pub kind: InboundEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendMessageResult {
    pub provider_message_id: String,
}
