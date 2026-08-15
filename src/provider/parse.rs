//! Webhook secret verification and text webhook parsing.

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use subtle::ConstantTimeEq;

use crate::error::ErrorClass;

use super::error::ZaloProviderError;
use super::types::{
    EVENT_IMAGE_RECEIVED, EVENT_TEXT_RECEIVED, InboundEventKind, NormalizedInboundImage,
    NormalizedInboundText, ZaloHttpConfig,
};

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

    if event_id.is_empty() || event_id.chars().count() > 256 {
        return Err(validation("message missing message_id"));
    }
    if sender_id.is_empty() || sender_id.chars().count() > 256 {
        return Err(validation("message missing from.id"));
    }
    if chat_id.is_empty() || chat_id.chars().count() > 256 {
        return Err(validation("message missing chat.id"));
    }

    let text = wire.text.unwrap_or_default();
    if kind == InboundEventKind::TextReceived
        && (text.trim().is_empty() || text.chars().count() > 2000)
    {
        return Err(validation("text message has invalid length"));
    }

    let received_at = if wire.date != 0 {
        provider_time(wire.date).ok_or_else(|| validation("message date is out of range"))?
    } else {
        Utc::now()
    };

    Ok(NormalizedInboundText {
        provider_scope: config.provider_scope.clone(),
        event_id,
        sender_id,
        chat_id,
        text,
        received_at,
        kind,
    })
}

pub fn parse_image_webhook(
    config: &ZaloHttpConfig,
    body: &[u8],
) -> Result<NormalizedInboundImage, ZaloProviderError> {
    let outer: ProviderEnvelope =
        serde_json::from_slice(body).map_err(|_| validation("invalid webhook body"))?;
    if matches!(outer.ok, Some(false)) {
        return Err(validation("webhook envelope was not ok"));
    }

    let (event_name, message_value) = extract_envelope(outer)?;
    let event_name = event_name.trim();
    if event_name.chars().count() > 256 {
        return Err(validation("webhook event_name exceeds limit"));
    }

    let kind = if event_name == EVENT_IMAGE_RECEIVED {
        InboundEventKind::ImageReceived
    } else {
        InboundEventKind::Unsupported(event_name.to_string())
    };

    let wire: WireImageMessage =
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

    if event_id.is_empty() || event_id.chars().count() > 256 {
        return Err(validation("message missing message_id"));
    }
    if sender_id.is_empty() || sender_id.chars().count() > 256 {
        return Err(validation("message missing from.id"));
    }
    if chat_id.is_empty() || chat_id.chars().count() > 256 {
        return Err(validation("message missing chat.id"));
    }

    let image_url = extract_image_url(&wire).unwrap_or_default();
    if kind == InboundEventKind::ImageReceived
        && (image_url.is_empty() || image_url.chars().count() > 2048)
    {
        return Err(validation("image message has invalid photo url"));
    }

    let caption = extract_caption(&wire);
    if caption.chars().count() > 2000 {
        return Err(validation("image message caption exceeds limit"));
    }

    let received_at = if wire.date != 0 {
        provider_time(wire.date).ok_or_else(|| validation("message date is out of range"))?
    } else {
        Utc::now()
    };

    Ok(NormalizedInboundImage {
        provider_scope: config.provider_scope.clone(),
        event_id,
        sender_id,
        chat_id,
        image_url,
        caption,
        received_at,
        kind,
    })
}

fn extract_image_url(wire: &WireImageMessage) -> Option<String> {
    for value in [
        &wire.photo,
        &wire.photo_url,
        &wire.file_url,
        &wire.photos,
        &wire.image,
        &wire.attachments,
    ]
    .into_iter()
    .flatten()
    {
        if let Some(url) = extract_url_from_value(value) {
            return Some(url);
        }
    }
    None
}

fn extract_url_from_value(value: &Value) -> Option<String> {
    match value {
        Value::String(url) => {
            let trimmed = url.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Value::Object(map) => {
            for key in ["url", "photo", "photo_url", "file_url", "image"] {
                if let Some(nested) = map.get(key)
                    && let Some(url) = extract_url_from_value(nested)
                {
                    return Some(url);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_url_from_value),
        _ => None,
    }
}

fn extract_caption(wire: &WireImageMessage) -> String {
    if let Some(caption) = wire
        .caption
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return caption.to_string();
    }
    if let Some(text) = wire
        .text
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return text.to_string();
    }
    for value in [
        &wire.photo,
        &wire.photo_url,
        &wire.file_url,
        &wire.photos,
        &wire.image,
        &wire.attachments,
    ]
    .into_iter()
    .flatten()
    {
        if let Some(caption) = extract_caption_from_value(value) {
            return caption;
        }
    }
    String::new()
}

fn extract_caption_from_value(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(caption)) = map.get("caption") {
                let trimmed = caption.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            for key in ["url", "photo", "photo_url", "file_url", "image"] {
                if let Some(nested) = map.get(key)
                    && let Some(caption) = extract_caption_from_value(nested)
                {
                    return Some(caption);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(extract_caption_from_value),
        _ => None,
    }
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

fn provider_time(value: i64) -> Option<DateTime<Utc>> {
    const MILLISECOND_THRESHOLD: i64 = 100_000_000_000;
    if value >= MILLISECOND_THRESHOLD || value <= -MILLISECOND_THRESHOLD {
        Utc.timestamp_millis_opt(value).single()
    } else {
        Utc.timestamp_opt(value, 0).single()
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
struct WireImageMessage {
    message_id: Option<Value>,
    from: Option<WireParty>,
    chat: Option<WireParty>,
    #[serde(default)]
    date: i64,
    photo: Option<Value>,
    photo_url: Option<Value>,
    file_url: Option<Value>,
    photos: Option<Value>,
    image: Option<Value>,
    attachments: Option<Value>,
    caption: Option<String>,
    text: Option<String>,
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
