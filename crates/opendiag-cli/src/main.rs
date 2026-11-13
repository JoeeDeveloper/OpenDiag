use clap::Parser;

/// Open source VAG group diagnostic tool.
#[derive(Parser)]
#[command(name = "opendiag", version, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
    println!(
        "opendiag {} - no command given; try --help",
        env!("CARGO_PKG_VERSION")
    );
}
