pub mod commands;
pub mod tui;

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "lanflare",
    version,
    about = "Fast, cross-platform local network file sharing (LocalSend v2.2 compatible)"
)]
pub struct Cli {
    /// Device alias advertised on the network
    #[arg(long, global = true)]
    pub alias: Option<String>,

    /// Port to bind services on
    #[arg(long, short, global = true)]
    pub port: Option<u16>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scan and list nearby peers on the local network
    Scan(ScanArgs),

    /// Send files to a peer
    Send(SendArgs),

    /// Run receiver daemon
    Receive(ReceiveArgs),

    /// Print local status and identity
    Status,
}

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Scan duration in seconds
    #[arg(long, short = 't', default_value = "3")]
    pub duration: u64,

    /// Disable UDP Multicast discovery
    #[arg(long)]
    pub no_multicast: bool,

    /// Disable mDNS discovery
    #[arg(long)]
    pub no_mdns: bool,
}

#[derive(Args, Debug)]
pub struct SendArgs {
    /// File path(s) to send
    #[arg(required = true)]
    pub files: Vec<PathBuf>,

    /// Destination IP, alias, or fingerprint
    #[arg(long, short)]
    pub to: Option<String>,

    /// Security PIN if required by receiver
    #[arg(long)]
    pub pin: Option<String>,
}

#[derive(Args, Debug)]
pub struct ReceiveArgs {
    /// Directory where received files will be saved
    #[arg(long, short)]
    pub dir: Option<PathBuf>,

    /// Port to listen on (defaults to configured port)
    #[arg(long, short)]
    pub port: Option<u16>,

    /// Automatically accept transfers without interactive prompt
    #[arg(long)]
    pub auto_accept: bool,

    /// Require incoming connections to provide this PIN
    #[arg(long)]
    pub pin: Option<String>,
}
