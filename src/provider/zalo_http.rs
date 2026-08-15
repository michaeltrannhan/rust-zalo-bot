//! Real HTTP adapter for the Zalo Bot API.

use super::error::ZaloProviderError;
use super::parse::{parse_image_webhook, parse_text_webhook, verify_webhook_secret};
use super::send::{send_message, validate_outbound};
use super::types::{
    DEFAULT_API_BASE, NormalizedInboundImage, NormalizedInboundText, SendMessageResult,
    ZaloHttpConfig,
};

pub struct ZaloHttpAdapter {
    config: ZaloHttpConfig,
    client: reqwest::Client,
}

impl ZaloHttpAdapter {
    pub fn new(config: ZaloHttpConfig) -> Result<Self, ZaloProviderError> {
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|err| {
                ZaloProviderError::new(
                    crate::error::ErrorClass::Dependency,
                    format!("build HTTP client: {err}"),
                )
            })?;
        Ok(Self { config, client })
    }

    pub fn config(&self) -> &ZaloHttpConfig {
        &self.config
    }

    pub fn verify_webhook_secret(
        &self,
        header_value: Option<&str>,
    ) -> Result<(), ZaloProviderError> {
        verify_webhook_secret(&self.config, header_value)
    }

    pub fn parse_text_webhook(
        &self,
        body: &[u8],
    ) -> Result<NormalizedInboundText, ZaloProviderError> {
        parse_text_webhook(&self.config, body)
    }

    pub fn parse_image_webhook(
        &self,
        body: &[u8],
    ) -> Result<NormalizedInboundImage, ZaloProviderError> {
        parse_image_webhook(&self.config, body)
    }

    pub fn validate_outbound(&self, chat_id: &str, text: &str) -> Result<(), ZaloProviderError> {
        validate_outbound(chat_id, text)
    }

    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<SendMessageResult, ZaloProviderError> {
        send_message(&self.client, &self.config, chat_id, text).await
    }
}

impl Default for ZaloHttpConfig {
    fn default() -> Self {
        Self {
            api_base: DEFAULT_API_BASE.to_string(),
            bot_token: String::new(),
            webhook_secret: String::new(),
            provider_scope: "zalo_bot".to_string(),
            request_timeout: std::time::Duration::from_secs(10),
        }
    }
}
