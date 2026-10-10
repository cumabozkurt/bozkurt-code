//! Error type for `bz-net`.

use std::path::PathBuf;

/// Result alias used throughout `bz-net`.
pub type Result<T, E = NetError> = std::result::Result<T, E>;

/// Every failure `bz-net` can report.
#[derive(Debug, thiserror::Error)]
pub enum NetError {
    /// Filesystem or socket error, with the path or operation that failed.
    #[error("{context}: {source}")]
    Io {
        /// What we were doing.
        context: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Certificate generation or parsing failed.
    #[error("certificate error: {0}")]
    Cert(String),
    /// DNS wire-format error.
    #[error("dns error: {0}")]
    Dns(String),
    /// An external command failed.
    #[error("command `{command}` failed (exit {code:?}): {stderr}")]
    Command {
        /// The command line, for display.
        command: String,
        /// Exit code, if the process exited normally.
        code: Option<i32>,
        /// Captured standard error (trimmed).
        stderr: String,
    },
    /// The user declined or cancelled an elevation prompt.
    #[error("elevation was cancelled or denied: {0}")]
    ElevationDenied(String),
    /// The requested operation is not available on this system.
    #[error("not supported: {0}")]
    Unsupported(String),
    /// Invalid argument (bad domain, bad TLD, ...).
    #[error("invalid input: {0}")]
    Invalid(String),
    /// A required file is missing.
    #[error("missing file: {}", .0.display())]
    Missing(PathBuf),
}

impl NetError {
    /// Wrap an I/O error with a context string.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        NetError::Io {
            context: context.into(),
            source,
        }
    }
}

pub(crate) trait IoContext<T> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for std::result::Result<T, std::io::Error> {
    fn ctx(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| NetError::io(context(), e))
    }
}
