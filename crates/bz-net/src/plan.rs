//! Change plans: every modification outside `$BOZKURT_HOME` is first described as a [`Plan`]
//! (shown to the user for confirmation), then performed by [`crate::elevation::apply`].

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Target operating system of a plan. Plans are pure data, so a plan for any OS can be
/// generated (and unit-tested) on any OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Os {
    /// macOS
    Macos,
    /// Linux
    Linux,
    /// Windows
    Windows,
}

impl Os {
    /// The OS this binary was compiled for (non-macOS/Windows unixes are treated as Linux).
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        })
    }
}

/// What a single step does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Create or replace a file with `contents` (parent directories are created).
    WriteFile {
        /// Absolute path.
        path: PathBuf,
        /// Full new contents.
        contents: String,
        /// Unix permission bits to apply after writing (ignored on Windows).
        mode: Option<u32>,
    },
    /// Delete a file if it exists.
    RemoveFile {
        /// Absolute path.
        path: PathBuf,
    },
    /// Run a program with arguments (no shell).
    Run {
        /// Program name or path.
        program: String,
        /// Arguments.
        args: Vec<String>,
        /// Continue even if the command fails.
        ignore_failure: bool,
    },
    /// Run a snippet in the platform shell: `/bin/sh` on macOS/Linux, PowerShell on Windows.
    Shell {
        /// Script text.
        script: String,
        /// Continue even if the snippet fails.
        ignore_failure: bool,
    },
}

/// One step of a plan, with a human-readable description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// What this step does, for the confirmation dialog.
    pub description: String,
    /// The action.
    pub action: Action,
    /// Whether the step needs administrator rights.
    pub elevated: bool,
}

/// A named, ordered list of changes for one OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Short title ("Route *.test to BOZKURT DNS").
    pub title: String,
    /// The OS this plan targets.
    pub os: Os,
    /// Steps in order.
    pub steps: Vec<Step>,
    /// Extra files (besides `WriteFile`/`RemoveFile` targets) that commands in this plan modify
    /// and that must be backed up first.
    pub backup_paths: Vec<PathBuf>,
    /// Free-form notes for the user (caveats, follow-ups).
    pub notes: Vec<String>,
}

impl Plan {
    /// New empty plan.
    pub fn new(title: impl Into<String>, os: Os) -> Self {
        Plan {
            title: title.into(),
            os,
            steps: Vec::new(),
            backup_paths: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// True when there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// True if any step needs administrator rights.
    pub fn requires_elevation(&self) -> bool {
        self.steps.iter().any(|s| s.elevated)
    }

    /// Append a step.
    pub fn push(
        &mut self,
        description: impl Into<String>,
        action: Action,
        elevated: bool,
    ) -> &mut Self {
        self.steps.push(Step {
            description: description.into(),
            action,
            elevated,
        });
        self
    }

    /// Append a file write.
    pub fn write_file(
        &mut self,
        description: impl Into<String>,
        path: impl Into<PathBuf>,
        contents: impl Into<String>,
        mode: Option<u32>,
        elevated: bool,
    ) -> &mut Self {
        self.push(
            description,
            Action::WriteFile {
                path: path.into(),
                contents: contents.into(),
                mode,
            },
            elevated,
        )
    }

    /// Append a file removal.
    pub fn remove_file(
        &mut self,
        description: impl Into<String>,
        path: impl Into<PathBuf>,
        elevated: bool,
    ) -> &mut Self {
        self.push(
            description,
            Action::RemoveFile { path: path.into() },
            elevated,
        )
    }

    /// Append a program invocation.
    pub fn run<I, S>(
        &mut self,
        description: impl Into<String>,
        program: &str,
        args: I,
        elevated: bool,
    ) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.push(
            description,
            Action::Run {
                program: program.to_string(),
                args: args.into_iter().map(Into::into).collect(),
                ignore_failure: false,
            },
            elevated,
        )
    }

    /// Append a shell snippet.
    pub fn shell(
        &mut self,
        description: impl Into<String>,
        script: impl Into<String>,
        elevated: bool,
    ) -> &mut Self {
        self.push(
            description,
            Action::Shell {
                script: script.into(),
                ignore_failure: false,
            },
            elevated,
        )
    }

    /// Every path that must be backed up before applying (write/remove targets + `backup_paths`),
    /// de-duplicated, in order.
    pub fn files_to_backup(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        let mut add = |p: &PathBuf| {
            if !out.contains(p) {
                out.push(p.clone());
            }
        };
        for s in &self.steps {
            match &s.action {
                Action::WriteFile { path, .. } | Action::RemoveFile { path } => add(path),
                _ => {}
            }
        }
        for p in &self.backup_paths {
            add(p);
        }
        out
    }

    /// Human-readable lines, one per step (prefixed with `[admin]` when elevated).
    pub fn describe(&self) -> Vec<String> {
        self.steps
            .iter()
            .map(|s| {
                if s.elevated {
                    format!("[admin] {}", s.description)
                } else {
                    s.description.clone()
                }
            })
            .collect()
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} ({})", self.title, self.os)?;
        for (i, line) in self.describe().iter().enumerate() {
            writeln!(f, "  {}. {}", i + 1, line)?;
        }
        for n in &self.notes {
            writeln!(f, "  note: {n}")?;
        }
        Ok(())
    }
}
