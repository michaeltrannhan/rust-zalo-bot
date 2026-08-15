//! Webhook secret verification and text webhook parsing.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::error::ErrorClass;

use super::error::ZaloProviderError;
use super::types::{EVENT_TEXT_RECEIVED, InboundEventKind, NormalizedInboundText, ZaloHttpConfig};

pub fn verify_webhook_secret(
    config: &ZaloHttpConfig,
    header_value: Option<&str>,
) -> Result<(), ZaloProviderError> {
    let configured = config.webhook_secret.trim();
    if configured.is_empty() {
        return Err(ZaloProviderError::new(
            ErrorClass::Forbidden,
            "webhook secret not configured",
        ));
    }
    let provided = header_value.map(str::trim).unwrap_or_default();
    if provided.len() != configured.len()
        || provided.as_bytes().ct_eq(configured.as_bytes()).unwrap_u8() != 1
    {
        return Err(ZaloProviderError::new(
            ErrorClass::Auth,
            "invalid webhook secret",
        ));
    }
    Ok(())
}

pub fn parse_text_webhook(
    config: &ZaloHttpConfig,
    body: &[u8],
) -> Result<NormalizedInboundText, ZaloProviderError> {
    let outer: ProviderEnvelope =
        serde_json::from_slice(body).map_err(|_| validation("invalid webhook body"))?;
    if matches!(outer.ok, Some(false)) {
        return Err(validation("webhook envelope was not ok"));
    }

    let (event_name, message_value) = extract_envelope(outer)?;

    let kind = if event_name == EVENT_TEXT_RECEIVED {
        InboundEventKind::TextReceived
    } else {
        InboundEventKind::Unsupported(event_name.clone())
    };

    let wire: WireMessage =
        serde_json::from_value(message_value).map_err(|_| validation("invalid message payload"))?;

    let event_id = wire
        .message_id
        .as_ref()
        .and_then(id_string)
        .unwrap_or_default();
    let sender_id = wire
        .from
        .as_ref()
        .and_then(|party| party.id.as_ref())
        .and_then(id_string)
        .unwrap_or_default();
    let chat_id = wire
        .chat
        .as_ref()
        .and_then(|party| party.id.as_ref())
        .and_then(id_string)
        .unwrap_or_default();

    if event_id.is_empty() {
        return Err(validation("message missing message_id"));
    }
    if sender_id.is_empty() {
        return Err(validation("message missing from.id"));
    }
    if chat_id.is_empty() {
        return Err(validation("message missing chat.id"));
    }

    let received_at = if wire.date != 0 {
        provider_time(wire.date)
    } else {
        Utc::now()
    };

    Ok(NormalizedInboundText {
        provider_scope: config.provider_scope.clone(),
        event_id,
        sender_id,
        chat_id,
        text: wire.text.unwrap_or_default(),
        received_at,
        kind,
    })
}

fn extract_envelope(outer: ProviderEnvelope) -> Result<(String, Value), ZaloProviderError> {
    let mut event_name = outer.event_name.unwrap_or_default();
    let mut message = outer.message;
    if let Some(result) = outer.result
        && !result.is_null()
    {
        let inner: InnerEnvelope =
            serde_json::from_value(result).map_err(|_| validation("invalid webhook result"))?;
        if event_name.is_empty() {
            event_name = inner.event_name.unwrap_or_default();
        }
        if message.is_none() {
            message = inner.message;
        }
    }
    if event_name.trim().is_empty() {
        return Err(validation("webhook event_name is required"));
    }
    let message_value = message.ok_or_else(|| validation("message payload required"))?;
    Ok((event_name, message_value))
}

fn validation(message: impl Into<String>) -> ZaloProviderError {
    ZaloProviderError::new(ErrorClass::Validation, message)
}

fn provider_time(value: i64) -> DateTime<Utc> {
    const MILLISECOND_THRESHOLD: i64 = 100_000_000_000;
    if value >= MILLISECOND_THRESHOLD || value <= -MILLISECOND_THRESHOLD {
        Utc.timestamp_millis_opt(value)
            .single()
            .unwrap_or_else(Utc::now)
    } else {
        Utc.timestamp_opt(value, 0)
            .single()
            .unwrap_or_else(Utc::now)
    }
}

fn id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct ProviderEnvelope {
    ok: Option<bool>,
    result: Option<Value>,
    event_name: Option<String>,
    message: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct InnerEnvelope {
    event_name: Option<String>,
    message: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    message_id: Option<Value>,
    from: Option<WireParty>,
    chat: Option<WireParty>,
    #[serde(default)]
    date: i64,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireParty {
    id: Option<Value>,
}
