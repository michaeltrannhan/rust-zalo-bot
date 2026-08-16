//! Receipt lifecycle operational errors.

use crate::error::ErrorClass;

/// Receipt store failure with a stable error class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptError {
    pub class: ErrorClass,
    pub message: String,
}

impl ReceiptError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Validation, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::NotFound, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Conflict, message)
    }

    pub fn duplicate(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Duplicate, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Unsupported, message)
    }

    pub fn dependency(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Dependency, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Forbidden, message)
    }

    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Transient, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Timeout, message)
    }

    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::RateLimited, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Auth, message)
    }

    pub fn quota_exceeded(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::QuotaExceeded, message)
    }

    pub fn kill_switch(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::KillSwitch, message)
    }
}

impl std::fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for ReceiptError {}
