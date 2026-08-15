//! Provider adapter errors with mandatory redaction.

use std::fmt;

use crate::error::ErrorClass;

use super::redact::redact_value;

#[derive(Clone, Default)]
struct RedactionNeedles {
    token: String,
    chat_id: String,
    text: String,
    url: String,
}

#[derive(Clone)]
pub struct ZaloProviderError {
    pub class: ErrorClass,
    message: String,
    needles: Box<RedactionNeedles>,
}

impl ZaloProviderError {
    pub(crate) fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            needles: Box::default(),
        }
    }

    pub(crate) fn with_redaction_context(
        mut self,
        token: impl Into<String>,
        chat_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.needles.token = token.into();
        self.needles.chat_id = chat_id.into();
        self.needles.text = text.into();
        self
    }

    pub fn redacted_message(&self) -> String {
        redact_value(
            &self.message,
            &self.needles.token,
            &self.needles.chat_id,
            &self.needles.text,
            &self.needles.url,
        )
        .into_owned()
    }

    pub(crate) fn attach_send_context(self, token: &str, chat_id: &str, text: &str) -> Self {
        self.with_redaction_context(token, chat_id, text)
    }

    pub(crate) fn attach_media_context(mut self, url: &str) -> Self {
        self.needles.url = url.to_string();
        self
    }
}

impl fmt::Display for ZaloProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.redacted_message())
    }
}

impl fmt::Debug for ZaloProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZaloProviderError")
            .field("class", &self.class)
            .field("message", &self.redacted_message())
            .finish_non_exhaustive()
    }
}

impl std::error::Error for ZaloProviderError {}
