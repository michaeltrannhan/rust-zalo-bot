//! Provider adapter errors with mandatory redaction.

use std::fmt;

use crate::error::ErrorClass;

use super::redact::redact_value;

#[derive(Clone)]
pub struct ZaloProviderError {
    pub class: ErrorClass,
    message: String,
    token: String,
    chat_id: String,
    text: String,
}

impl ZaloProviderError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            token: String::new(),
            chat_id: String::new(),
            text: String::new(),
        }
    }

    pub fn with_redaction_context(
        mut self,
        token: impl Into<String>,
        chat_id: impl Into<String>,
        text: impl Into<String>,
    ) -> Self {
        self.token = token.into();
        self.chat_id = chat_id.into();
        self.text = text.into();
        self
    }

    pub fn redacted_message(&self) -> String {
        redact_value(&self.message, &self.token, &self.chat_id, &self.text).into_owned()
    }

    pub fn attach_send_context(self, token: &str, chat_id: &str, text: &str) -> Self {
        self.with_redaction_context(token, chat_id, text)
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
