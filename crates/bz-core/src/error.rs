//! Error type shared by the engine. Each variant maps to a stable API error `code`
//! and an HTTP status the daemon should use.

use crate::models::{ApiError, ApiErrorInner};

/// Engine error. `code()` is the API `error.code`, `http_status()` the suggested status.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    /// A download with no upstream checksum and no pinned digest (P1-1).
    #[error("unverified download: {0}")]
    Unverified(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl CoreError {
    /// Stable machine-readable code used in the API error envelope.
    pub fn code(&self) -> &'static str {
        match self {
            CoreError::NotFound(_) => "not_found",
            CoreError::InvalidInput(_) => "invalid_input",
            CoreError::Conflict(_) => "conflict",
            CoreError::Unsupported(_) => "unsupported",
            CoreError::Upstream(_) => "upstream",
            CoreError::Checksum { .. } => "checksum_mismatch",
            CoreError::Unverified(_) => "unverified_download",
            CoreError::Io(_) => "io",
            CoreError::Other(_) => "internal",
        }
    }

    /// Suggested HTTP status code.
    pub fn http_status(&self) -> u16 {
        match self {
            CoreError::NotFound(_) => 404,
            CoreError::InvalidInput(_) => 400,
            CoreError::Conflict(_) => 409,
            CoreError::Unsupported(_) | CoreError::Unverified(_) => 422,
            CoreError::Upstream(_) | CoreError::Checksum { .. } => 502,
            CoreError::Io(_) | CoreError::Other(_) => 500,
        }
    }

    /// Build the API error envelope.
    pub fn to_api(&self) -> ApiError {
        ApiError {
            error: ApiErrorInner {
                code: self.code().to_string(),
                message: self.to_string(),
            },
        }
    }
}

impl From<reqwest::Error> for CoreError {
    fn from(e: reqwest::Error) -> Self {
        // include the source chain ("error sending request" alone is useless)
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg.push_str(": ");
            msg.push_str(&s.to_string());
            src = s.source();
        }
        CoreError::Upstream(msg)
    }
}

impl From<serde_json::Error> for CoreError {
    fn from(e: serde_json::Error) -> Self {
        CoreError::InvalidInput(format!("json: {e}"))
    }
}

impl From<toml::de::Error> for CoreError {
    fn from(e: toml::de::Error) -> Self {
        CoreError::InvalidInput(format!("toml: {e}"))
    }
}

/// Engine result alias.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope() {
        let e = CoreError::NotFound("site shop".into());
        let v = serde_json::to_value(e.to_api()).unwrap();
        assert_eq!(v["error"]["code"], "not_found");
        assert_eq!(e.http_status(), 404);
    }
}
