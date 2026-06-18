//! `robit-qq` binary entry point.
//!
//! NOTE: Full wiring lands in Phase 9. This stub parses CLI args so the
//! binary builds; the QQ Bot connection is added in Phase 9.

use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "robit-qq")]
#[command(about = "Robit AI Agent - QQ Bot")]
#[command(version)]
struct Cli {
    /// Working directory for the agent
    #[arg(long, short = 'w')]
    workdir: Option<std::path::PathBuf>,

    /// Use global storage for session database
    #[arg(long)]
    global_storage: bool,
}

#[tokio::main]
async fn main() {
    let _cli = Cli::parse();
    // Phase 9: load config, bootstrap tools/skills, connect QQ platform, run manager.
    eprintln!("robit-qq: not yet implemented (see implementation plan).");
}
