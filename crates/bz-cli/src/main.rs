//! `bz` binary.

fn main() {
    std::process::exit(bz_cli::commands::run(std::env::args_os()));
}
