pub mod commands;
pub mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "lanflare", version, about = "Fast, cross-platform local network file sharing")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan and list nearby peers
    Scan,
    /// Send files to a peer
    Send {
        #[arg(required = true)]
        files: Vec<String>,
        #[arg(long, short)]
        to: Option<String>,
    },
    /// Run receiver daemon
    Receive {
        #[arg(long, short)]
        dir: Option<String>,
    },
    /// Print local status and identity
    Status,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Scan) => println!("Scanning local network..."),
        Some(Commands::Send { files, to }) => println!("Sending {:?} to {:?}", files, to),
        Some(Commands::Receive { dir }) => println!("Listening for incoming files into {:?}", dir),
        Some(Commands::Status) | None => println!("LanFlare Node Status: Ready"),
    }

    Ok(())
}
