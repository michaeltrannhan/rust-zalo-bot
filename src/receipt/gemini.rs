//! Gemini generateContent adapter for receipt extraction.

use std::fmt;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::{DateTime, Utc};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::redirect::Policy;
use serde_json::{Value, json};

use super::downscale::downscale_to_jpeg;
use super::error::ReceiptError;
use super::types::{ExtractedAttempt, ExtractionMeta, ExtractionResult};
use super::validate::{validate_amount_minor, validate_currency, validate_merchant};

pub const EXTRACTION_PROMPT_VERSION: &str = "extraction-json-v2";

const EXTRACTION_PROMPT: &str = "Extract one Vietnamese receipt or invoice into JSON matching the schema. \
Read the merchant/store carefully from the header (company or shop name), not product line items. \
amount_minor is the FINAL amount the customer pays in VND đồng as an integer \
(example: 118.000đ → 118000). Prefer totals labeled Tổng cộng, Thành tiền, Tổng thanh toán, \
or similar over unit prices or subtotals. \
occurred_at is the receipt date/time as RFC3339 UTC; if only a calendar date is printed, \
use that date at 00:00:00Z. \
Choose the best category_key from the allowed enum. \
Set unsupported=true only when the image is clearly not a receipt/invoice.";

const CATEGORY_KEYS: &[&str] = &[
    "an-uong",
    "thuc-pham",
    "di-lai",
    "hoa-don",
    "mua-sam",
    "suc-khoe",
    "giai-tri",
    "giao-duc",
    "nha-o",
    "thu-nhap",
    "hoan-tien",
    "chuyen-khoan",
    "khac",
];

/// Configuration for the Gemini HTTP extractor. Secrets must not appear in Debug.
pub struct GeminiExtractorConfig {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    pub profile_name: String,
    pub timeout: Duration,
    pub max_input_bytes: usize,
    pub max_output_tokens: u32,
    pub thinking_effort: String,
    pub schema_version: String,
}

pub struct GeminiHttpExtractor {
    api_base: String,
    api_key: String,
    model: String,
    profile_name: String,
    max_input_bytes: usize,
    max_output_tokens: u32,
    thinking_effort: String,
    schema_version: String,
    client: reqwest::Client,
}

impl GeminiHttpExtractor {
    pub fn new(config: GeminiExtractorConfig) -> Result<Self, ReceiptError> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(Policy::none())
            .build()
            .map_err(|_| ReceiptError::dependency("failed to build gemini HTTP client"))?;
        Ok(Self {
            api_base: config.api_base.trim_end_matches('/').to_string(),
            api_key: config.api_key,
            model: config.model,
            profile_name: config.profile_name,
            max_input_bytes: config.max_input_bytes,
            max_output_tokens: config.max_output_tokens,
            thinking_effort: config.thinking_effort,
            schema_version: config.schema_version,
            client,
        })
    }

    fn static_meta(&self) -> ExtractionMeta {
        ExtractionMeta {
            provider: "gemini".to_string(),
            model: self.model.clone(),
            profile_name: self.profile_name.clone(),
            prompt_version: EXTRACTION_PROMPT_VERSION.to_string(),
            input_tokens: None,
            output_tokens: None,
        }
    }

    fn execute(&self, bytes: &[u8]) -> Result<ExtractedAttempt, ReceiptError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.execute_async(bytes))
        })
    }

    async fn execute_async(&self, bytes: &[u8]) -> Result<ExtractedAttempt, ReceiptError> {
        let jpeg = downscale_to_jpeg(bytes)?;
        if jpeg.len() > self.max_input_bytes {
            return Err(ReceiptError::validation(
                "extraction input exceeds profile max_input_bytes",
            ));
        }

        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.api_base, self.model
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-goog-api-key",
            HeaderValue::from_str(&self.api_key)
                .map_err(|_| ReceiptError::auth("gemini credential is invalid"))?,
        );

        let response = match self
            .client
            .post(url)
            .headers(headers)
            .json(&self.request_body(&jpeg))
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Err(ReceiptError::timeout("gemini request timed out"));
            }
            Err(_) => return Err(ReceiptError::dependency("gemini request failed")),
        };

        let status = response.status();
        if status.is_redirection() {
            return Err(ReceiptError::validation(
                "gemini returned an unexpected redirect",
            ));
        }
        if let Some(error) = classify_http_status(status.as_u16()) {
            return Err(error);
        }

        let body = response
            .text()
            .await
            .map_err(|_| ReceiptError::dependency("failed to read gemini response"))?;
        parse_generate_content(&body, self.static_meta())
    }

    fn request_body(&self, jpeg: &[u8]) -> Value {
        let mut generation_config = json!({
            "responseMimeType": "application/json",
            "responseSchema": response_schema(),
            "maxOutputTokens": self.max_output_tokens,
        });
        if let Some(thinking) = thinking_config(&self.model, &self.thinking_effort) {
            generation_config["thinkingConfig"] = thinking;
        }
        json!({
            "contents": [{
                "role": "user",
                "parts": [
                    { "text": EXTRACTION_PROMPT },
                    {
                        "inlineData": {
                            "mimeType": "image/jpeg",
                            "data": BASE64.encode(jpeg)
                        }
                    }
                ]
            }],
            "generationConfig": generation_config
        })
    }
}

impl super::extractor::ReceiptExtractor for GeminiHttpExtractor {
    fn extract(&self, bytes: &[u8]) -> Result<ExtractedAttempt, ReceiptError> {
        self.execute(bytes)
    }

    fn meta(&self) -> ExtractionMeta {
        self.static_meta()
    }
}

impl fmt::Debug for GeminiHttpExtractor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeminiHttpExtractor")
            .field("model", &self.model)
            .field("profile_name", &self.profile_name)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for GeminiExtractorConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeminiExtractorConfig")
            .field("api_base", &"[REDACTED]")
            .field("model", &self.model)
            .field("profile_name", &self.profile_name)
            .field("timeout", &self.timeout)
            .field("max_input_bytes", &self.max_input_bytes)
            .field("max_output_tokens", &self.max_output_tokens)
            .field("thinking_effort", &self.thinking_effort)
            .field("schema_version", &self.schema_version)
            .finish_non_exhaustive()
    }
}

fn thinking_config(model: &str, effort: &str) -> Option<Value> {
    let model = model.to_ascii_lowercase();
    if model.starts_with("gemini-3") {
        // Gemini 3 uses thinkingLevel; budget is ignored / rejected.
        let level = match effort {
            "none" | "low" => "LOW",
            "medium" | "high" => "HIGH",
            _ => return None,
        };
        return Some(json!({ "thinkingLevel": level }));
    }
    thinking_budget(effort).map(|budget| json!({ "thinkingBudget": budget }))
}

fn thinking_budget(effort: &str) -> Option<u32> {
    match effort {
        // Explicit 0 disables dynamic thinking on flash models (default is on).
        "none" => Some(0),
        "low" => Some(2048),
        "medium" => Some(8192),
        "high" => Some(16384),
        _ => None,
    }
}

fn response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "merchant": { "type": "string" },
            "amount_minor": { "type": "integer" },
            "currency": { "type": "string" },
            "category_key": { "type": "string", "enum": CATEGORY_KEYS },
            "transaction_type": { "type": "string", "enum": ["expense", "income", "transfer"] },
            "occurred_at": { "type": "string" },
            "confidence": { "type": "number" },
            "unsupported": { "type": "boolean" }
        },
        "required": [
            "merchant",
            "amount_minor",
            "currency",
            "category_key",
            "transaction_type",
            "occurred_at",
            "confidence",
            "unsupported"
        ]
    })
}

fn classify_http_status(status: u16) -> Option<ReceiptError> {
    match status {
        200..=299 => None,
        401 | 403 => Some(ReceiptError::auth("gemini authentication failed")),
        429 => Some(ReceiptError::rate_limited("gemini rate limited")),
        400 | 404 | 411 | 413 | 422 => {
            Some(ReceiptError::validation("gemini rejected the request"))
        }
        500..=599 => Some(ReceiptError::transient("gemini upstream error")),
        _ => Some(ReceiptError::validation(
            "gemini returned an unexpected status",
        )),
    }
}

fn parse_generate_content(
    body: &str,
    mut meta: ExtractionMeta,
) -> Result<ExtractedAttempt, ReceiptError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|_| ReceiptError::validation("gemini response was not json"))?;
    if parsed.pointer("/promptFeedback/blockReason").is_some() {
        return Err(ReceiptError::validation("gemini blocked the prompt"));
    }
    if let Some(reason) = parsed
        .pointer("/candidates/0/finishReason")
        .and_then(Value::as_str)
        && reason != "STOP"
        && reason != "stop"
    {
        return Err(ReceiptError::validation("gemini did not finish normally"));
    }
    let text = extract_candidate_text(&parsed)
        .ok_or_else(|| ReceiptError::validation("gemini response missing candidates"))?;
    if let Some(input) = parsed
        .pointer("/usageMetadata/promptTokenCount")
        .and_then(Value::as_i64)
    {
        meta.input_tokens = i32::try_from(input).ok();
    }
    if let Some(output) = parsed
        .pointer("/usageMetadata/candidatesTokenCount")
        .and_then(Value::as_i64)
    {
        meta.output_tokens = i32::try_from(output).ok();
    }
    let result = parse_extraction_json(text)?;
    Ok(ExtractedAttempt { result, meta })
}

fn extract_candidate_text(parsed: &Value) -> Option<&str> {
    let parts = parsed.pointer("/candidates/0/content/parts")?.as_array()?;
    // Prefer the last non-thought text part so thinking models still yield JSON.
    parts.iter().rev().find_map(|part| {
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        part.get("text")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })
}

fn parse_extraction_json(text: &str) -> Result<ExtractionResult, ReceiptError> {
    let parsed: Value = serde_json::from_str(text)
        .map_err(|_| ReceiptError::validation("gemini output was not json"))?;
    let merchant = validate_merchant(
        parsed
            .get("merchant")
            .and_then(Value::as_str)
            .ok_or_else(|| ReceiptError::validation("gemini output missing merchant"))?,
    )?;
    let amount_minor = parsed
        .get("amount_minor")
        .and_then(Value::as_i64)
        .ok_or_else(|| ReceiptError::validation("gemini output missing amount_minor"))?;
    let currency = parsed
        .get("currency")
        .and_then(Value::as_str)
        .ok_or_else(|| ReceiptError::validation("gemini output missing currency"))?
        .to_string();
    let category_key = parsed
        .get("category_key")
        .and_then(Value::as_str)
        .ok_or_else(|| ReceiptError::validation("gemini output missing category_key"))?
        .to_string();
    if !CATEGORY_KEYS.contains(&category_key.as_str()) {
        return Err(ReceiptError::validation(
            "gemini output category_key is not allowed",
        ));
    }
    let transaction_type = parsed
        .get("transaction_type")
        .and_then(Value::as_str)
        .ok_or_else(|| ReceiptError::validation("gemini output missing transaction_type"))?
        .to_string();
    if !matches!(transaction_type.as_str(), "expense" | "income" | "transfer") {
        return Err(ReceiptError::validation(
            "gemini output transaction_type is not allowed",
        ));
    }
    let occurred_at = parsed
        .get("occurred_at")
        .and_then(Value::as_str)
        .ok_or_else(|| ReceiptError::validation("gemini output missing occurred_at"))?;
    let occurred_at = DateTime::parse_from_rfc3339(occurred_at)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|_| ReceiptError::validation("gemini output occurred_at is invalid"))?;
    let confidence = parsed
        .get("confidence")
        .and_then(Value::as_f64)
        .ok_or_else(|| ReceiptError::validation("gemini output missing confidence"))?
        as f32;
    let unsupported = parsed
        .get("unsupported")
        .and_then(Value::as_bool)
        .ok_or_else(|| ReceiptError::validation("gemini output missing unsupported"))?;
    if !unsupported {
        validate_amount_minor(amount_minor)?;
        validate_currency(&currency)?;
    }
    Ok(ExtractionResult {
        merchant,
        amount_minor,
        currency,
        category_key,
        transaction_type,
        occurred_at,
        confidence: confidence.clamp(0.0, 1.0),
        unsupported,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_omits_api_key_and_prompt() {
        let extractor = GeminiHttpExtractor::new(GeminiExtractorConfig {
            api_base: "http://127.0.0.1:1".to_string(),
            api_key: "sk-test-secret".to_string(),
            model: "gemini-2.5-flash".to_string(),
            profile_name: "receipt-fast".to_string(),
            timeout: Duration::from_secs(1),
            max_input_bytes: 1024,
            max_output_tokens: 128,
            thinking_effort: "none".to_string(),
            schema_version: "v1".to_string(),
        })
        .expect("extractor");
        let debug = format!("{extractor:?}");
        assert!(!debug.contains("sk-test-secret"));
        assert!(!debug.contains(EXTRACTION_PROMPT));
        assert!(debug.contains("receipt-fast"));
    }

    #[test]
    fn malformed_candidates_are_validation() {
        let error = parse_generate_content("{}", ExtractionMeta::fake()).unwrap_err();
        assert_eq!(error.class, crate::error::ErrorClass::Validation);
        assert!(!error.message.contains("sk-test-secret"));
    }

    #[test]
    fn skips_thought_parts_when_reading_json() {
        let body = serde_json::json!({
            "candidates": [{
                "finishReason": "STOP",
                "content": {
                    "parts": [
                        { "thought": true, "text": "reasoning..." },
                        { "text": serde_json::json!({
                            "merchant": "Co.opmart",
                            "amount_minor": 325000,
                            "currency": "VND",
                            "category_key": "thuc-pham",
                            "transaction_type": "expense",
                            "occurred_at": "2026-07-15T09:24:00Z",
                            "confidence": 0.95,
                            "unsupported": false
                        }).to_string() }
                    ]
                }
            }]
        })
        .to_string();
        let attempt = parse_generate_content(&body, ExtractionMeta::fake()).expect("parse");
        assert_eq!(attempt.result.merchant, "Co.opmart");
    }

    #[test]
    fn gemini3_uses_thinking_level() {
        let config = thinking_config("gemini-3.6-flash", "high").expect("config");
        assert_eq!(config["thinkingLevel"], "HIGH");
        assert!(config.get("thinkingBudget").is_none());
        let low = thinking_config("gemini-3.5-flash", "low").expect("low");
        assert_eq!(low["thinkingLevel"], "LOW");
        let flash25 = thinking_config("gemini-2.5-flash", "medium").expect("budget");
        assert_eq!(flash25["thinkingBudget"], 8192);
    }
}
