use clap::Parser;
use lanflare_cli::{Cli, Commands};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;

#[test]
fn test_cli_parsing_status() {
    let cli = Cli::try_parse_from(["lanflare", "status"]).expect("parse status");
    assert!(matches!(cli.command, Some(Commands::Status)));
}

#[test]
fn test_cli_parsing_scan() {
    let cli = Cli::try_parse_from(["lanflare", "scan", "--duration", "5", "--no-mdns"])
        .expect("parse scan");
    match cli.command {
        Some(Commands::Scan(args)) => {
            assert_eq!(args.duration, 5);
            assert!(!args.no_multicast);
            assert!(args.no_mdns);
        }
        _ => panic!("expected Scan command"),
    }
}

#[test]
fn test_cli_parsing_send() {
    let cli = Cli::try_parse_from(["lanflare", "send", "file1.txt", "file2.pdf", "-t", "192.168.1.5", "--pin", "1234"])
        .expect("parse send");
    match cli.command {
        Some(Commands::Send(args)) => {
            assert_eq!(args.files.len(), 2);
            assert_eq!(args.to.as_deref(), Some("192.168.1.5"));
            assert_eq!(args.pin.as_deref(), Some("1234"));
        }
        _ => panic!("expected Send command"),
    }
}

#[test]
fn test_cli_parsing_receive() {
    let cli = Cli::try_parse_from(["lanflare", "receive", "-d", "/tmp/downloads", "-p", "53318", "--auto-accept", "--pin", "9999"])
        .expect("parse receive");
    match cli.command {
        Some(Commands::Receive(args)) => {
            assert_eq!(args.dir.unwrap().to_str(), Some("/tmp/downloads"));
            assert_eq!(args.port, Some(53318));
            assert!(args.auto_accept);
            assert_eq!(args.pin.as_deref(), Some("9999"));
        }
        _ => panic!("expected Receive command"),
    }
}

#[tokio::test]
async fn test_cli_status_handler() {
    let config = AppConfig::default();
    let identity = generate_self_signed().expect("generate identity");
    let result = lanflare_cli::commands::status(&config, &identity).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_cli_send_handler_file_not_found() {
    let config = AppConfig::default();
    let identity = generate_self_signed().expect("generate identity");
    let send_opts = lanflare_cli::commands::SendOptions {
        files: vec!["/nonexistent/path/to/random_file.bin".into()],
        to: Some("192.168.1.20".to_string()),
        pin: None,
    };
    let result = lanflare_cli::commands::send(&config, &identity, &send_opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("File not found"));
}
