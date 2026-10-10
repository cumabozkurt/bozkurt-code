//! Host OS / CPU architecture identification used to pick upstream assets.

use std::fmt;

/// Operating system family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    MacOs,
    Linux,
    Windows,
}

/// CPU architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    X64,
    Arm64,
}

/// An (os, arch) pair. Catalogs map this to upstream asset names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Platform {
    pub os: Os,
    pub arch: Arch,
}

impl Platform {
    /// The platform this binary was compiled for.
    pub fn current() -> Platform {
        let os = if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        };
        let arch = if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::X64
        };
        Platform { os, arch }
    }

    pub const fn new(os: Os, arch: Arch) -> Platform {
        Platform { os, arch }
    }

    /// `macos` / `linux` / `windows`.
    pub fn os_str(&self) -> &'static str {
        match self.os {
            Os::MacOs => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }

    /// `x86_64` / `aarch64`.
    pub fn arch_str(&self) -> &'static str {
        match self.arch {
            Arch::X64 => "x86_64",
            Arch::Arm64 => "aarch64",
        }
    }

    /// Go-style arch (`amd64` / `arm64`), used by Go, Caddy, Mailpit.
    pub fn goarch(&self) -> &'static str {
        match self.arch {
            Arch::X64 => "amd64",
            Arch::Arm64 => "arm64",
        }
    }

    /// Rust-style target triple, used by python-build-standalone and postgresql-binaries.
    pub fn triple(&self) -> &'static str {
        match (self.os, self.arch) {
            (Os::MacOs, Arch::Arm64) => "aarch64-apple-darwin",
            (Os::MacOs, Arch::X64) => "x86_64-apple-darwin",
            (Os::Linux, Arch::Arm64) => "aarch64-unknown-linux-gnu",
            (Os::Linux, Arch::X64) => "x86_64-unknown-linux-gnu",
            (Os::Windows, Arch::Arm64) => "aarch64-pc-windows-msvc",
            (Os::Windows, Arch::X64) => "x86_64-pc-windows-msvc",
        }
    }

    pub fn is_windows(&self) -> bool {
        self.os == Os::Windows
    }

    /// Executable suffix (`.exe` on Windows).
    pub fn exe(&self) -> &'static str {
        if self.is_windows() {
            ".exe"
        } else {
            ""
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.os_str(), self.arch_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triples() {
        assert_eq!(
            Platform::new(Os::Linux, Arch::Arm64).triple(),
            "aarch64-unknown-linux-gnu"
        );
        assert_eq!(Platform::new(Os::Windows, Arch::X64).exe(), ".exe");
        assert_eq!(Platform::new(Os::MacOs, Arch::X64).goarch(), "amd64");
    }
}
