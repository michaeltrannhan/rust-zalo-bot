//! Outbound sendMessage adapter.

use reqwest::StatusCode;
use serde::Deserialize;

use crate::error::ErrorClass;

use super::error::ZaloProviderError;
use super::redact::redact_value;
use super::types::{SendMessageResult, ZaloHttpConfig};

pub fn validate_outbound(chat_id: &str, text: &str) -> Result<(), ZaloProviderError> {
    if chat_id.trim().is_empty() {
        return Err(ZaloProviderError::new(
            ErrorClass::Validation,
            "provider chat id required",
        ));
    }
    if text.trim().is_empty() {
        return Err(ZaloProviderError::new(
            ErrorClass::Validation,
            "message text required",
        ));
    }
    if text.chars().count() > 2000 {
        return Err(ZaloProviderError::new(
            ErrorClass::Validation,
            "message text exceeds 2000 characters",
        ));
    }
    Ok(())
}

pub async fn send_message(
    client: &reqwest::Client,
    config: &ZaloHttpConfig,
    chat_id: &str,
    text: &str,
) -> Result<SendMessageResult, ZaloProviderError> {
    validate_outbound(chat_id, text)?;

    if config.bot_token.trim().is_empty() {
        return Err(ZaloProviderError::new(
            ErrorClass::Validation,
            "zalo bot token not configured",
        ));
    }

    let url = format!(
        "{}/bot{}/sendMessage",
        config.normalized_api_base(),
        config.bot_token
    );
    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
    });

    let response = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| map_transport_error(&config.bot_token, chat_id, text, err))?;

    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|err| map_transport_error(&config.bot_token, chat_id, text, err))?;

    if !status.is_success() {
        return Err(classify_http_status(
            status,
            &body,
            &config.bot_token,
            chat_id,
            text,
        ));
    }

    parse_success_body(&body, &config.bot_token, chat_id, text)
}

fn parse_success_body(
    body: &[u8],
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<SendMessageResult, ZaloProviderError> {
    let parsed: SendResponse = serde_json::from_slice(body).map_err(|_| {
        ambiguous("malformed sendMessage response").attach_send_context(token, chat_id, text)
    })?;

    if !parsed.ok {
        return Err(
            ambiguous("sendMessage response was not ok").attach_send_context(token, chat_id, text)
        );
    }

    let provider_message_id = parsed
        .result
        .and_then(|result| id_from_value(&result.message_id))
        .unwrap_or_default();

    Ok(SendMessageResult {
        provider_message_id,
    })
}

fn classify_http_status(
    status: StatusCode,
    body: &[u8],
    token: &str,
    chat_id: &str,
    text: &str,
) -> ZaloProviderError {
    let snippet = redact_body_snippet(body, token, chat_id, text);
    let message = format!("zalo api status {}: {}", status.as_u16(), snippet);
    let class = match status.as_u16() {
        429 => ErrorClass::RateLimited,
        500..=599 => ErrorClass::ProviderError,
        400..=499 => ErrorClass::ProviderError,
        _ => ErrorClass::ProviderError,
    };
    ZaloProviderError::new(class, message).attach_send_context(token, chat_id, text)
}

fn map_transport_error(
    token: &str,
    chat_id: &str,
    text: &str,
    err: reqwest::Error,
) -> ZaloProviderError {
    if err.is_timeout() {
        return ambiguous("sendMessage request timed out")
            .attach_send_context(token, chat_id, text);
    }
    let cause = err.without_url().to_string();
    let redacted = redact_value(&cause, token, chat_id, text).into_owned();
    ZaloProviderError::new(
        ErrorClass::ProviderError,
        format!("sendMessage request failed: {redacted}"),
    )
    .attach_send_context(token, chat_id, text)
}

fn ambiguous(message: impl Into<String>) -> ZaloProviderError {
    ZaloProviderError::new(ErrorClass::ProviderAmbiguous, message)
}

fn redact_body_snippet(body: &[u8], token: &str, chat_id: &str, text: &str) -> String {
    let raw = String::from_utf8_lossy(body);
    let trimmed = raw.trim();
    let snippet = if trimmed.len() > 200 {
        trimmed[..200].to_string()
    } else {
        trimmed.to_string()
    };
    redact_value(&snippet, token, chat_id, text).into_owned()
}

fn id_from_value(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[derive(Debug, Deserialize)]
struct SendResponse {
    ok: bool,
    result: Option<SendResultBody>,
}

#[derive(Debug, Deserialize)]
struct SendResultBody {
    message_id: serde_json::Value,
}
