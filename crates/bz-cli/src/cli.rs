//! Argument definitions (clap derive) for `bz` (`docs/CLI.md`).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::register::McpClient;

#[derive(Debug, Parser)]
#[command(
    name = "bz",
    version,
    about = "BOZKURT CODE: local dev environment (runtimes, services, sites)",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Args, Clone, Default)]
pub struct Global {
    /// Print raw API JSON on stdout; no colours, spinners or prompts
    #[arg(long, global = true)]
    pub json: bool,
    /// Assume "yes" to confirmation prompts
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,
    /// Print only errors
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// Log HTTP requests (repeat for more)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Do not auto-start the daemon (exit 3 if it is not running)
    #[arg(long, global = true)]
    pub no_start: bool,
    /// BOZKURT_HOME to use
    #[arg(long, global = true, env = "BOZKURT_HOME", value_name = "DIR")]
    pub home: Option<PathBuf>,
    /// Disable colours (NO_COLOR is honoured too)
    #[arg(long, global = true)]
    pub no_color: bool,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Start a service, or all of them
    Start { service: Option<String> },
    /// Stop a service, or all of them
    Stop { service: Option<String> },
    /// Restart a service, or all of them
    Restart { service: Option<String> },
    /// Overview: services, runtimes, sites
    Status,
    /// List services
    Services,

    /// PHP versions
    Php(RtArgs),
    /// Node.js versions
    Node(RtArgs),
    /// Python versions
    Python(RtArgs),
    /// Bun versions
    Bun(RtArgs),
    /// Go versions
    Go(RtArgs),
    /// Runtime commands taking the kind as an argument (`bz runtime install php 8.3`)
    Runtime {
        #[command(subcommand)]
        cmd: RuntimeAlias,
    },

    /// Link a project directory as a site
    Link(LinkArgs),
    /// Remove a site (project files are never touched)
    Unlink { site: Option<String> },
    /// List sites
    Sites,
    /// Open a site in the browser
    Open { site: Option<String> },
    /// Enable HTTPS for a site
    Secure { site: Option<String> },
    /// Disable HTTPS for a site
    Unsecure { site: Option<String> },
    /// Share a site on a public URL (Ctrl-C stops sharing)
    Share { site: Option<String> },
    /// Site commands (`bz site start <site>`)
    Site {
        #[command(subcommand)]
        cmd: SiteAlias,
    },

    /// Show logs of a service, a site or `all`
    Logs(LogsArgs),

    /// Run diagnostics (exit 1 if any check fails)
    Doctor {
        /// Run one fix by id (e.g. `dns.configure`, `tls.trust`, `services.caddy.start`)
        #[arg(long, value_name = "ID")]
        fix: Option<String>,
    },
    /// Local DNS for the TLD
    Dns {
        #[command(subcommand)]
        cmd: DnsCmd,
    },
    /// Trust the BOZKURT local CA (`bz trust status`, `bz trust --undo`)
    #[command(args_conflicts_with_subcommands = true)]
    Trust {
        /// Remove the CA from the trust stores
        #[arg(long)]
        undo: bool,
        #[command(subcommand)]
        cmd: Option<TrustSub>,
    },
    /// TLS commands (`bz tls trust|untrust|status`)
    #[command(hide = true)]
    Tls {
        #[command(subcommand)]
        cmd: TlsCmd,
    },
    /// Databases on mariadb / postgres
    Db {
        #[command(subcommand)]
        cmd: DbCmd,
    },
    /// Import vhosts and DNS from Valet, Herd, MAMP, XAMPP, Laragon, Homebrew
    Import {
        /// Import everything found
        #[arg(long)]
        all: bool,
    },
    /// Remote access for the mobile companion
    Remote {
        #[command(subcommand)]
        cmd: Option<RemoteCmd>,
    },
    /// Read or change config.toml
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Background jobs
    Jobs { id: Option<String> },
    /// Service commands (`bz service start dns`)
    #[command(hide = true)]
    Service {
        #[command(subcommand)]
        cmd: ServiceAlias,
    },

    /// AI client integration (MCP)
    Mcp {
        #[command(subcommand)]
        cmd: McpCmd,
    },
    /// Print a shell completion script
    Completions { shell: clap_complete::Shell },
    /// Manage the bozkurtd daemon (`bz daemon` = start)
    Daemon {
        /// Run in the foreground (same as `bz daemon run`)
        #[arg(long)]
        foreground: bool,
        #[command(subcommand)]
        cmd: Option<DaemonCmd>,
    },
}

#[derive(Debug, Args)]
pub struct RtArgs {
    #[command(subcommand)]
    pub cmd: RtCmd,
}

#[derive(Debug, Subcommand)]
pub enum RtCmd {
    /// List versions (installed only unless --available)
    #[command(alias = "ls")]
    List {
        #[arg(long, short)]
        available: bool,
        /// Refresh the upstream index
        #[arg(long)]
        refresh: bool,
    },
    /// Install a version (partial, `lts` or `latest` accepted)
    Install {
        #[arg(id = "ver", value_name = "VERSION")]
        version: String,
        /// Accept a download without a published checksum (static-php.dev) once and pin
        /// its sha256 for later installs
        #[arg(long)]
        allow_unverified: bool,
    },
    /// Make a version the default
    Use {
        #[arg(id = "ver", value_name = "VERSION")]
        version: String,
    },
    /// Remove an installed version
    #[command(alias = "remove", alias = "rm")]
    Uninstall {
        #[arg(id = "ver", value_name = "VERSION")]
        version: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RuntimeAlias {
    /// `bz runtime install <kind> [version]` (version defaults to lts for node, else latest)
    Install {
        kind: String,
        #[arg(id = "ver", value_name = "VERSION")]
        version: Option<String>,
        /// Accept a download without a published checksum once and pin its sha256
        #[arg(long)]
        allow_unverified: bool,
    },
    Use {
        kind: String,
        #[arg(id = "ver", value_name = "VERSION")]
        version: String,
    },
    #[command(alias = "remove", alias = "rm")]
    Uninstall {
        kind: String,
        #[arg(id = "ver", value_name = "VERSION")]
        version: String,
    },
    #[command(alias = "ls")]
    List {
        kind: String,
        #[arg(long, short)]
        available: bool,
        #[arg(long)]
        refresh: bool,
    },
}

#[derive(Debug, Args, Default, Clone)]
pub struct LinkArgs {
    /// Project directory (default: current directory)
    pub path: Option<PathBuf>,
    /// Domain (`shop` or `shop.test`)
    #[arg(long)]
    pub domain: Option<String>,
    #[arg(long, value_name = "VER")]
    pub php: Option<String>,
    #[arg(long, value_name = "VER")]
    pub node: Option<String>,
    /// Reverse-proxy to this port (implies --kind proxy)
    #[arg(long, value_name = "PORT")]
    pub proxy: Option<u16>,
    #[arg(long)]
    pub https: bool,
    #[arg(long)]
    pub wildcard: bool,
    /// Extra domain (repeatable)
    #[arg(long = "alias", value_name = "NAME")]
    pub aliases: Vec<String>,
    #[arg(long, value_parser = ["php", "proxy", "static", "app"])]
    pub kind: Option<String>,
    /// Document root relative to the project
    #[arg(long, value_name = "DIR")]
    pub root: Option<String>,
    /// Dev command to run (implies --kind app)
    #[arg(long, value_name = "CMD")]
    pub command: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum SiteAlias {
    Start {
        site: Option<String>,
    },
    Stop {
        site: Option<String>,
    },
    Restart {
        site: Option<String>,
    },
    #[command(alias = "remove", alias = "unlink")]
    Rm {
        site: Option<String>,
    },
    /// Set one field: `bz site set <site> <key> <value>`
    Set {
        site: String,
        key: String,
        value: String,
    },
    #[command(alias = "ls")]
    List,
    #[command(alias = "show", alias = "get")]
    Info {
        site: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct LogsArgs {
    /// Service name, site, or `all`
    pub source: Option<String>,
    #[arg(short = 'n', long, default_value_t = 200)]
    pub lines: usize,
    /// Keep streaming new lines
    #[arg(short, long)]
    pub follow: bool,
}

#[derive(Debug, Subcommand)]
pub enum DnsCmd {
    /// Route *.<tld> to the local resolver (admin prompt; changes are backed up)
    #[command(alias = "configure")]
    Setup {
        #[arg(long)]
        tld: Option<String>,
        /// Restore the OS resolver configuration
        #[arg(long)]
        undo: bool,
    },
    #[command(hide = true)]
    Unconfigure,
    Status,
}

#[derive(Debug, Subcommand)]
pub enum TrustSub {
    Status,
}

#[derive(Debug, Subcommand)]
pub enum TlsCmd {
    Trust,
    Untrust,
    Status,
}

#[derive(Debug, Subcommand)]
pub enum DbCmd {
    #[command(alias = "ls")]
    List {
        #[arg(long, default_value = "mariadb")]
        engine: String,
    },
    Create {
        name: String,
        #[arg(long, default_value = "mariadb")]
        engine: String,
    },
    #[command(alias = "rm", alias = "delete")]
    Drop {
        name: String,
        #[arg(long, default_value = "mariadb")]
        engine: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum RemoteCmd {
    Enable,
    Disable,
    /// Print an 8-digit pairing code and a QR code
    Pair,
    Devices,
    Revoke {
        device_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    Get {
        key: Option<String>,
    },
    /// `bz config set remote.port 7880` (values are parsed as JSON when possible)
    Set {
        key: String,
        value: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum ServiceAlias {
    Install {
        name: String,
        #[arg(id = "ver", value_name = "VERSION")]
        version: Option<String>,
    },
    Start {
        name: Option<String>,
    },
    Stop {
        name: Option<String>,
    },
    Restart {
        name: Option<String>,
    },
    Logs {
        name: String,
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: usize,
        #[arg(short, long)]
        follow: bool,
    },
    #[command(alias = "ls")]
    List,
}

#[derive(Debug, Subcommand)]
pub enum McpCmd {
    /// Write the bozkurt-mcp entry into the client's MCP config (backed up first)
    Register {
        client: McpClient,
        /// Write the project-level config in the current directory
        #[arg(long)]
        project: bool,
        /// Only print the snippet
        #[arg(long)]
        print: bool,
        /// Command to register (default: bozkurt-mcp, or its absolute path if not on PATH)
        #[arg(long, value_name = "PATH")]
        command: Option<String>,
    },
    /// Remove the bozkurt entry from the client's MCP config
    Unregister {
        client: McpClient,
        #[arg(long)]
        project: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum DaemonCmd {
    /// Start bozkurtd in the background
    Start,
    /// Stop bozkurtd
    Stop,
    /// Is bozkurtd running?
    Status,
    /// Run bozkurtd in the foreground
    Run,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("bz").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn verify_cli() {
        Cli::command().debug_assert();
    }

    #[test]
    fn globals_anywhere() {
        let c = parse(&["status", "--json", "-y", "--no-start"]);
        assert!(c.global.json && c.global.yes && c.global.no_start);
        assert!(matches!(c.cmd, Cmd::Status));
    }

    #[test]
    fn runtime_forms() {
        match parse(&["php", "install", "8.3"]).cmd {
            Cmd::Php(RtArgs {
                cmd:
                    RtCmd::Install {
                        version,
                        allow_unverified,
                    },
            }) => {
                assert_eq!(version, "8.3");
                assert!(!allow_unverified);
            }
            o => panic!("{o:?}"),
        }
        match parse(&["node", "list", "--available", "--refresh"]).cmd {
            Cmd::Node(RtArgs {
                cmd: RtCmd::List { available, refresh },
            }) => assert!(available && refresh),
            o => panic!("{o:?}"),
        }
        match parse(&["runtime", "install", "php"]).cmd {
            Cmd::Runtime {
                cmd: RuntimeAlias::Install { kind, version, .. },
            } => assert_eq!((kind.as_str(), version), ("php", None)),
            o => panic!("{o:?}"),
        }
        match parse(&["runtime", "install", "php", "--allow-unverified"]).cmd {
            Cmd::Runtime {
                cmd:
                    RuntimeAlias::Install {
                        allow_unverified, ..
                    },
            } => assert!(allow_unverified),
            o => panic!("{o:?}"),
        }
        match parse(&["runtime", "install", "php", "8.4"]).cmd {
            Cmd::Runtime {
                cmd: RuntimeAlias::Install { version, .. },
            } => assert_eq!(version.as_deref(), Some("8.4")),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn link_flags() {
        match parse(&[
            "link", "--https", "--alias", "a.test", "--alias", "b.test", "--proxy", "3000",
        ])
        .cmd
        {
            Cmd::Link(l) => {
                assert!(l.https && l.path.is_none());
                assert_eq!(l.aliases, vec!["a.test", "b.test"]);
                assert_eq!(l.proxy, Some(3000));
            }
            o => panic!("{o:?}"),
        }
        assert!(Cli::try_parse_from(["bz", "link", "--kind", "ruby"]).is_err());
    }

    #[test]
    fn aliases_from_hints() {
        assert!(matches!(
            parse(&["site", "start", "shop"]).cmd,
            Cmd::Site {
                cmd: SiteAlias::Start { site: Some(_) }
            }
        ));
        assert!(matches!(
            parse(&["daemon"]).cmd,
            Cmd::Daemon {
                cmd: None,
                foreground: false
            }
        ));
        assert!(matches!(
            parse(&["daemon", "stop"]).cmd,
            Cmd::Daemon {
                cmd: Some(DaemonCmd::Stop),
                ..
            }
        ));
        assert!(matches!(
            parse(&["daemon", "--foreground"]).cmd,
            Cmd::Daemon {
                foreground: true,
                ..
            }
        ));
        assert!(matches!(
            parse(&["dns", "configure"]).cmd,
            Cmd::Dns {
                cmd: DnsCmd::Setup { .. }
            }
        ));
        assert!(matches!(
            parse(&["tls", "trust"]).cmd,
            Cmd::Tls { cmd: TlsCmd::Trust }
        ));
        assert!(matches!(
            parse(&["service", "install", "caddy"]).cmd,
            Cmd::Service {
                cmd: ServiceAlias::Install { .. }
            }
        ));
        assert!(matches!(
            parse(&["trust", "status"]).cmd,
            Cmd::Trust {
                cmd: Some(TrustSub::Status),
                ..
            }
        ));
        assert!(matches!(
            parse(&["trust", "--undo"]).cmd,
            Cmd::Trust {
                undo: true,
                cmd: None
            }
        ));
    }

    #[test]
    fn mcp_register_clients() {
        for (arg, want) in [
            ("claude", McpClient::ClaudeCode),
            ("claude-code", McpClient::ClaudeCode),
            ("cursor", McpClient::Cursor),
            ("vscode", McpClient::Vscode),
            ("windsurf", McpClient::Windsurf),
            ("gemini", McpClient::Gemini),
            ("codex", McpClient::Codex),
        ] {
            match parse(&["mcp", "register", arg, "--print"]).cmd {
                Cmd::Mcp {
                    cmd: McpCmd::Register { client, print, .. },
                } => assert!(client == want && print),
                o => panic!("{o:?}"),
            }
        }
        assert!(Cli::try_parse_from(["bz", "mcp", "register", "emacs"]).is_err());
    }

    #[test]
    fn logs_and_db() {
        match parse(&["logs", "caddy", "-n", "50", "-f"]).cmd {
            Cmd::Logs(l) => {
                assert!(l.follow && l.lines == 50 && l.source.as_deref() == Some("caddy"))
            }
            o => panic!("{o:?}"),
        }
        match parse(&["db", "create", "shop", "--engine", "postgres"]).cmd {
            Cmd::Db {
                cmd: DbCmd::Create { name, engine },
            } => assert_eq!((name.as_str(), engine.as_str()), ("shop", "postgres")),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn usage_errors_exit_2() {
        let e = Cli::try_parse_from(["bz", "frobnicate"]).unwrap_err();
        assert_eq!(e.exit_code(), 2);
    }
}
