//! Configuration errors for Frixit Control Center.

use thiserror::Error;

/// Error conditions encountered during configuration loading, saving, or validation.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Save blocked: UCC_READ_ONLY_PREVIEW mode is active")]
    ReadOnlyPreview,

    #[error("Configuration validation error: {0}")]
    Validation(String),

    #[error("Profile not found: {0}")]
    ProfileNotFound(String),
}
