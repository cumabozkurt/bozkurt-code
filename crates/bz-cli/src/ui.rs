//! Terminal output: colours, tables, confirmations.

use std::io::{BufRead, IsTerminal, Write};

use comfy_table::{presets, ContentArrangement, Table};
use serde::Serialize;

use crate::error::CliError;
use crate::i18n::{is_yes, t};

#[derive(Debug, Clone, Default)]
pub struct Ui {
    pub json: bool,
    pub quiet: bool,
    pub yes: bool,
    pub color: bool,
    /// stdin + stderr are terminals (prompts / progress bars allowed).
    pub interactive: bool,
}

impl Ui {
    pub fn new(json: bool, quiet: bool, yes: bool, no_color: bool) -> Ui {
        let env_no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
        let tty = std::io::stdout().is_terminal();
        Ui {
            json,
            quiet,
            yes,
            color: !json && !no_color && !env_no_color && tty,
            interactive: !json && std::io::stdin().is_terminal() && std::io::stderr().is_terminal(),
        }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    pub fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    pub fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    /// Colour a status word.
    pub fn status(&self, s: &str) -> String {
        match s {
            "running" | "ok" | "done" => self.green(s),
            "crashed" | "fail" | "failed" => self.red(s),
            "external" | "warn" | "starting" | "stopping" => self.yellow(s),
            _ => self.dim(s),
        }
    }

    /// Human line on stdout (suppressed by `--json` / `--quiet`).
    pub fn line(&self, s: impl AsRef<str>) {
        if !self.json && !self.quiet {
            println!("{}", s.as_ref());
        }
    }

    /// `✔ message`.
    pub fn ok(&self, s: impl AsRef<str>) {
        self.line(format!("{} {}", self.green("✔"), s.as_ref()));
    }

    /// Note on stderr (suppressed by `--json` / `--quiet`).
    pub fn note(&self, s: impl AsRef<str>) {
        if !self.json && !self.quiet {
            eprintln!("{}", s.as_ref());
        }
    }

    /// Print a value as pretty JSON on stdout (only with `--json`).
    pub fn json_out<T: Serialize + ?Sized>(&self, v: &T) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into())
            );
        }
    }

    /// A borderless table with a header.
    pub fn table(&self, header: &[&str]) -> Table {
        let mut t = Table::new();
        t.load_preset(presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Dynamic);
        if !self.color {
            t.force_no_tty();
        }
        t.set_header(header.iter().map(|h| self.bold(h)).collect::<Vec<_>>());
        t
    }

    pub fn print_table(&self, t: &Table) {
        if !self.json && !self.quiet {
            // comfy-table pads the first column with a space with this preset; trim lines.
            for l in t.to_string().lines() {
                println!("{}", l.trim_end().strip_prefix(' ').unwrap_or(l.trim_end()));
            }
        }
    }

    /// Ask `Proceed? [y/N]` after naming what will change. `--yes` skips; `--json` (or a
    /// non-interactive stdin) without `--yes` fails with exit 6.
    pub fn confirm(&self, summary: &str, changes: &[String]) -> Result<(), CliError> {
        if self.yes {
            return Ok(());
        }
        if self.json || !self.interactive {
            return Err(CliError::ConfirmationRequired(format!(
                "{summary} — {}",
                t("confirm_needed_json")
            )));
        }
        eprintln!("{}", self.bold(summary));
        if !changes.is_empty() {
            eprintln!("{}", t("will_change"));
            for c in changes {
                eprintln!("  • {c}");
            }
        }
        eprint!("{}", t("proceed"));
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        if is_yes(&answer) {
            Ok(())
        } else {
            Err(CliError::ConfirmationRequired(t("aborted").into()))
        }
    }

    /// Prompt for a free-form answer (interactive only).
    pub fn ask(&self, prompt: &str) -> Result<String, CliError> {
        if !self.interactive {
            return Err(CliError::ConfirmationRequired(
                t("confirm_needed_json").into(),
            ));
        }
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer)?;
        Ok(answer.trim().to_string())
    }
}

/// `2h 11m`, `45s`, `3d 4h`.
pub fn human_duration(secs: u64) -> String {
    let (d, h, m, s) = (
        secs / 86400,
        secs % 86400 / 3600,
        secs % 3600 / 60,
        secs % 60,
    );
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

/// `12.3 MB`.
pub fn human_bytes(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanize() {
        assert_eq!(human_duration(7860), "2h 11m");
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
    }

    #[test]
    fn confirm_without_tty_or_yes_fails_with_6() {
        let ui = Ui {
            json: true,
            ..Ui::default()
        };
        let e = ui.confirm("x", &[]).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        let ui = Ui {
            yes: true,
            json: true,
            ..Ui::default()
        };
        assert!(ui.confirm("x", &[]).is_ok());
    }
}
