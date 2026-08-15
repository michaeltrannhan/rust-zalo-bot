//! Durable-work operational errors.

use crate::error::ErrorClass;

/// Work store failure with a stable error class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkError {
    pub class: ErrorClass,
    pub message: String,
}

impl WorkError {
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

    pub fn dependency(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Dependency, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Cancelled, message)
    }
}

impl std::fmt::Display for WorkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for WorkError {}
