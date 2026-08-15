//! Stable error classes and CLI exit codes.

use std::fmt;

/// Stable redaction-safe error class identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Validation,
    NotFound,
    Conflict,
    Forbidden,
    ConsentRequired,
    QuotaExceeded,
    Unsupported,
    Duplicate,
    Auth,
    RateLimited,
    Timeout,
    ProviderError,
    ProviderAmbiguous,
    Transient,
    KillSwitch,
    Internal,
    Config,
    Dependency,
    Migration,
    Permission,
    Cancelled,
    PreflightFailed,
    HealthFailed,
}

impl ErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Forbidden => "forbidden",
            Self::ConsentRequired => "consent_required",
            Self::QuotaExceeded => "quota_exceeded",
            Self::Unsupported => "unsupported",
            Self::Duplicate => "duplicate",
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::Timeout => "timeout",
            Self::ProviderError => "provider_error",
            Self::ProviderAmbiguous => "provider_ambiguous",
            Self::Transient => "transient",
            Self::KillSwitch => "kill_switch",
            Self::Internal => "internal",
            Self::Config => "config_error",
            Self::Dependency => "dependency_error",
            Self::Migration => "migration_error",
            Self::Permission => "permission_error",
            Self::Cancelled => "cancelled",
            Self::PreflightFailed => "preflight_failed",
            Self::HealthFailed => "health_failed",
        }
    }
}

impl fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// CLI exit code taxonomy from product contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Success = 0,
    RuntimeError = 1,
    UsageError = 2,
    ConfigError = 3,
    DependencyError = 4,
    MigrationError = 5,
    PermissionError = 6,
    ConflictError = 7,
    Cancelled = 8,
    PreflightFailed = 10,
    HealthFailed = 11,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

/// Application error with stable class and redacted message.
#[derive(Debug)]
pub struct AppError {
    pub class: ErrorClass,
    pub message: String,
}

impl AppError {
    pub fn new(class: ErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Config, message)
    }

    pub fn dependency(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Dependency, message)
    }

    pub fn migration(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Migration, message)
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Validation, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorClass::Internal, message)
    }

    pub fn exit_code(&self) -> ExitCode {
        match self.class {
            ErrorClass::Config => ExitCode::ConfigError,
            ErrorClass::Dependency => ExitCode::DependencyError,
            ErrorClass::Migration => ExitCode::MigrationError,
            ErrorClass::Permission => ExitCode::PermissionError,
            ErrorClass::Conflict => ExitCode::ConflictError,
            ErrorClass::Cancelled => ExitCode::Cancelled,
            ErrorClass::PreflightFailed => ExitCode::PreflightFailed,
            ErrorClass::HealthFailed => ExitCode::HealthFailed,
            ErrorClass::Validation => ExitCode::UsageError,
            ErrorClass::Internal => ExitCode::RuntimeError,
            _ => ExitCode::RuntimeError,
        }
    }

    /// JSON line for stderr — never includes secrets or connection URLs.
    pub fn to_json_line(&self) -> String {
        let payload = serde_json::json!({
            "error_class": self.class.as_str(),
            "message": self.message,
        });
        payload.to_string()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for AppError {}
