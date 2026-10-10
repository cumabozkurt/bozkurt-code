//! `bozkurt-mcp` binary: MCP over stdio. Logs go to stderr only.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("bozkurt-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "bozkurt-mcp {}: BOZKURT CODE MCP server (stdio).\n\nRun it from an MCP client; see `bz mcp register <client>`.\nEnvironment: BOZKURT_HOME, BOZKURT_MCP_NO_START=1 (do not auto-start bozkurtd).",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    let home = bz_cli::daemon::home(None);
    let mut server = bz_mcp::Server::new(home);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if let Err(e) = bz_mcp::serve(&mut server, stdin.lock(), stdout.lock()) {
        eprintln!("bozkurt-mcp: {e}");
        std::process::exit(1);
    }
}
