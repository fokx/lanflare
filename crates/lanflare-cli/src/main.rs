use clap::Parser;
use lanflare_cli::{commands, Cli, Commands};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let mut config = AppConfig::default();
    if let Some(alias) = cli.alias {
        config.alias = alias;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    // Generate or load TLS identity for this session
    let identity = generate_self_signed()?;

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
        Some(Commands::Status) | None => {
            commands::status(&config, &identity).await?;
        }
    }

    Ok(())
}
