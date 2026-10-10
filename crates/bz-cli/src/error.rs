//! CLI errors and their exit codes (`docs/CLI.md#exit-codes`).

use std::fmt;

use serde_json::json;

#[derive(Debug, Clone, PartialEq)]
pub enum CliError {
    /// Exit 2.
    Usage(String),
    /// Exit 3.
    Daemon(String),
    /// API error envelope; exit 4 (404), 5 (409), 6 (elevation / confirmation codes), else 1.
    Api {
        status: u16,
        code: String,
        message: String,
    },
    /// Exit 6.
    ConfirmationRequired(String),
    /// Exit 7.
    JobFailed(String),
    /// Exit 130.
    Interrupted,
    /// Exit 1.
    Other(String),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Daemon(_) => 3,
            CliError::Api { status, code, .. } => match (status, code.as_str()) {
                (_, "confirmation_required" | "elevation_denied" | "permission_denied") => 6,
                (404, _) | (_, "not_found") => 4,
                (409, _) | (_, "conflict") => 5,
                _ => 1,
            },
            CliError::ConfirmationRequired(_) => 6,
            CliError::JobFailed(_) => 7,
            CliError::Interrupted => 130,
            CliError::Other(_) => 1,
        }
    }

    /// The machine-readable `error.code`.
    pub fn code(&self) -> &str {
        match self {
            CliError::Usage(_) => "usage",
            CliError::Daemon(_) => "daemon_unreachable",
            CliError::Api { code, .. } => code,
            CliError::ConfirmationRequired(_) => "confirmation_required",
            CliError::JobFailed(_) => "job_failed",
            CliError::Interrupted => "interrupted",
            CliError::Other(_) => "error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            CliError::Usage(m)
            | CliError::Daemon(m)
            | CliError::ConfirmationRequired(m)
            | CliError::JobFailed(m)
            | CliError::Other(m) => m.clone(),
            CliError::Api { message, .. } => message.clone(),
            CliError::Interrupted => "interrupted".into(),
        }
    }

    /// `{"error":{"code","message"}}`.
    pub fn to_json(&self) -> serde_json::Value {
        json!({ "error": { "code": self.code(), "message": self.message() } })
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Api { code, message, .. } => write!(f, "{message} ({code})"),
            other => f.write_str(&other.message()),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        CliError::Other(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes() {
        let api = |status, code: &str| CliError::Api {
            status,
            code: code.into(),
            message: String::new(),
        };
        assert_eq!(api(404, "not_found").exit_code(), 4);
        assert_eq!(api(409, "conflict").exit_code(), 5);
        assert_eq!(api(500, "internal").exit_code(), 1);
        assert_eq!(api(403, "elevation_denied").exit_code(), 6);
        assert_eq!(CliError::Daemon("x".into()).exit_code(), 3);
        assert_eq!(CliError::JobFailed("x".into()).exit_code(), 7);
        assert_eq!(CliError::Interrupted.exit_code(), 130);
    }
}
