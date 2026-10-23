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

    /// Serve an entire directory to peers on localnet via dufs-compatible file server
    Serve(ServeArgs),

    /// Print local status and identity
    Status,

    /// Multi-hop mesh overlay operations, simulation, and network diagnostics
    Mesh(MeshArgs),

    /// Encrypted peer-to-peer and group chat with stranger trust gating
    Chat(ChatArgs),

    /// Federated file search across 1-hop LAN and multi-hop mesh peers
    Search(SearchArgs),
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

    /// Force active /24 subnet IP scan
    #[arg(long, short = 's')]
    pub subnet: bool,

    /// Disable fallback /24 subnet IP scan
    #[arg(long)]
    pub no_subnet: bool,
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

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Directory path to serve
    #[arg(required = true)]
    pub path: PathBuf,

    /// Port to listen on (defaults to configured port or 53317)
    #[arg(long, short)]
    pub port: Option<u16>,

    /// Allow peers to upload files into the folder
    #[arg(long)]
    pub allow_upload: bool,

    /// Allow peers to delete files in the folder
    #[arg(long)]
    pub allow_delete: bool,

    /// Require PIN/password for access (defaults to generating a strong PIN)
    #[arg(long)]
    pub pin: Option<String>,

    /// Disable PIN authentication (allow open access without password)
    #[arg(long)]
    pub no_auth: bool,
}

#[derive(Args, Debug)]
pub struct MeshArgs {
    #[command(subcommand)]
    pub command: MeshCommands,
}

#[derive(Subcommand, Debug)]
pub enum MeshCommands {
    /// Print local node overlay state, relay mode, and known peer topology
    Status(MeshStatusArgs),

    /// Launch interactive/programmatic multi-subnet virtual network sandbox
    Sim(MeshSimArgs),

    /// Run multi-hop traceroute to target fingerprint or alias
    Trace(MeshTraceArgs),

    /// Run in-memory throughput speedtest to target
    Benchmark(MeshBenchmarkArgs),
}

#[derive(Args, Debug, Default)]
pub struct MeshStatusArgs {
    /// Enable relay mode for this node
    #[arg(long)]
    pub relay: bool,
}

#[derive(Args, Debug)]
pub struct MeshSimArgs {
    /// Number of nodes to spawn in virtual cluster
    #[arg(long, short = 'n', default_value = "4")]
    pub nodes: usize,

    /// Duration in seconds to run the simulator (0 = run indefinitely until Ctrl+C)
    #[arg(long, short = 'd', default_value = "5")]
    pub duration: u64,

    /// Subnets to create (comma-separated, e.g. "SubnetA,SubnetB,SubnetC")
    #[arg(long, default_value = "SubnetA,SubnetB,SubnetC")]
    pub subnets: String,

    /// Inject simulated link latency in milliseconds
    #[arg(long, default_value = "20")]
    pub latency_ms: u32,
}

#[derive(Args, Debug)]
pub struct MeshTraceArgs {
    /// Destination peer alias, IP, or fingerprint
    #[arg(required = true)]
    pub target: String,

    /// Maximum Time-To-Live / hop limit
    #[arg(long, default_value = "5")]
    pub ttl: u8,
}

#[derive(Args, Debug)]
pub struct MeshBenchmarkArgs {
    /// Destination peer alias, IP, or fingerprint
    #[arg(required = true)]
    pub target: String,

    /// Number of chunk payloads to stream
    #[arg(long, short = 'n', default_value = "16")]
    pub chunks: u32,

    /// Chunk payload size in bytes (defaults to 128 KB)
    #[arg(long, short = 's', default_value = "131072")]
    pub chunk_size: usize,
}

#[derive(Args, Debug)]
pub struct ChatArgs {
    #[command(subcommand)]
    pub command: ChatCommands,
}

#[derive(Subcommand, Debug)]
pub enum ChatCommands {
    /// Send 1:1 text message with stranger trust verification
    Send(ChatSendArgs),

    /// Inspect or approve stranger trust handshakes
    Trust(ChatTrustArgs),

    /// List causal DAG group conversations and membership
    Groups(ChatGroupsArgs),
}

#[derive(Args, Debug)]
pub struct ChatSendArgs {
    /// Recipient peer alias, IP, or fingerprint
    #[arg(required = true)]
    pub target: String,

    /// Message text to transmit
    #[arg(required = true)]
    pub message: String,

    /// Force sending as introductory stranger trust handshake
    #[arg(long)]
    pub intro: bool,
}

#[derive(Args, Debug)]
pub struct ChatTrustArgs {
    /// Target peer alias or fingerprint to inspect or approve
    #[arg(required = true)]
    pub target: String,

    /// Approve pending stranger handshake from this peer
    #[arg(long)]
    pub approve: bool,

    /// Block this peer
    #[arg(long)]
    pub block: bool,
}

#[derive(Args, Debug, Default)]
pub struct ChatGroupsArgs {
    /// Optional group ID to inspect specific causal DAG state
    #[arg(long)]
    pub id: Option<String>,
}

#[derive(Args, Debug)]
pub struct SearchArgs {
    /// Search query term or filename pattern
    #[arg(required = true)]
    pub query: String,

    /// Security PIN for protected shares
    #[arg(long)]
    pub pin: Option<String>,

    /// Search timeout in seconds
    #[arg(long, default_value = "3")]
    pub timeout: u64,
}

/// Executes the CLI application lifecycle based on parsed command line arguments.
pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let mut config = lanflare_core::config::AppConfig::default();
    if let Some(alias) = cli.alias {
        config.alias = alias;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    // Generate or load TLS identity for this session
    let identity = lanflare_core::crypto::cert::generate_self_signed()?;

    match cli.command {
        Some(Commands::Scan(args)) => {
            let scan_opts = commands::ScanOptions {
                duration_secs: args.duration,
                multicast: !args.no_multicast,
                mdns: !args.no_mdns,
                subnet: args.subnet,
                fallback_subnet: !args.no_subnet,
            };
            commands::scan(&config, &identity, &scan_opts).await?;
        }
        Some(Commands::Send(args)) => {
            let send_opts = commands::SendOptions {
                files: args.files,
                to: args.to,
                pin: args.pin,
            };
            commands::send(&config, &identity, &send_opts).await?;
        }
        Some(Commands::Receive(args)) => {
            let receive_opts = commands::ReceiveOptions {
                dir: args.dir,
                port: args.port,
                auto_accept: args.auto_accept,
                pin: args.pin,
            };
            commands::receive(&config, &identity, &receive_opts).await?;
        }
        Some(Commands::Serve(args)) => {
            let pin = if args.no_auth {
                None
            } else {
                Some(args.pin.unwrap_or_else(lanflare_core::config::generate_share_link_pin))
            };
            let serve_opts = commands::ServeOptions {
                path: args.path,
                port: args.port,
                allow_upload: args.allow_upload,
                allow_delete: args.allow_delete,
                pin,
            };
            commands::serve(&config, &identity, &serve_opts).await?;
        }
        Some(Commands::Mesh(args)) => match args.command {
            MeshCommands::Status(s_args) => {
                let opts = commands::MeshStatusOptions {
                    relay: s_args.relay,
                };
                commands::mesh_status(&config, &identity, &opts).await?;
            }
            MeshCommands::Sim(sim_args) => {
                let subnets = sim_args
                    .subnets
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let opts = commands::MeshSimOptions {
                    nodes: sim_args.nodes,
                    duration: sim_args.duration,
                    subnets,
                    latency_ms: sim_args.latency_ms,
                };
                commands::mesh_sim(&config, &identity, &opts).await?;
            }
            MeshCommands::Trace(trace_args) => {
                let opts = commands::MeshTraceOptions {
                    target: trace_args.target,
                    ttl: trace_args.ttl,
                };
                commands::mesh_trace(&config, &identity, &opts).await?;
            }
            MeshCommands::Benchmark(bench_args) => {
                let opts = commands::MeshBenchmarkOptions {
                    target: bench_args.target,
                    chunks: bench_args.chunks,
                    chunk_size: bench_args.chunk_size,
                };
                commands::mesh_benchmark(&config, &identity, &opts).await?;
            }
        },
        Some(Commands::Chat(args)) => match args.command {
            ChatCommands::Send(send_args) => {
                let opts = commands::ChatSendOptions {
                    target: send_args.target,
                    message: send_args.message,
                    intro: send_args.intro,
                };
                commands::chat_send(&config, &identity, &opts).await?;
            }
            ChatCommands::Trust(trust_args) => {
                let opts = commands::ChatTrustOptions {
                    target: trust_args.target,
                    approve: trust_args.approve,
                    block: trust_args.block,
                };
                commands::chat_trust(&config, &identity, &opts).await?;
            }
            ChatCommands::Groups(groups_args) => {
                let opts = commands::ChatGroupsOptions {
                    id: groups_args.id,
                };
                commands::chat_groups(&config, &identity, &opts).await?;
            }
        },
        Some(Commands::Search(args)) => {
            let opts = commands::SearchOptions {
                query: args.query,
                pin: args.pin,
                timeout: args.timeout,
            };
            commands::search(&config, &identity, &opts).await?;
        }
        Some(Commands::Status) | None => {
            commands::status(&config, &identity).await?;
        }
    }

    Ok(())
}
