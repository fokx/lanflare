use clap::Parser;
use lanflare::{Cli, Commands};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;

#[test]
fn test_cli_parsing_status() {
    let cli = Cli::try_parse_from(["lanflare", "status"]).expect("parse status");
    assert!(matches!(cli.command, Some(Commands::Status)));
}

#[test]
fn test_cli_parsing_scan() {
    let cli = Cli::try_parse_from(["lanflare", "scan", "--duration", "5", "--no-mdns", "-s"])
        .expect("parse scan");
    match cli.command {
        Some(Commands::Scan(args)) => {
            assert_eq!(args.duration, 5);
            assert!(!args.no_multicast);
            assert!(args.no_mdns);
            assert!(args.subnet);
            assert!(!args.no_subnet);
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
    let result = lanflare::commands::status(&config, &identity).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_cli_send_handler_file_not_found() {
    let config = AppConfig::default();
    let identity = generate_self_signed().expect("generate identity");
    let send_opts = lanflare::commands::SendOptions {
        files: vec!["/nonexistent/path/to/random_file.bin".into()],
        to: Some("192.168.1.20".to_string()),
        pin: None,
    };
    let result = lanflare::commands::send(&config, &identity, &send_opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("File not found"));
}

#[test]
fn test_match_target_peer() {
    use lanflare_core::discovery::registry::{DiscoveredPeer, PeerEndpoint};
    use lanflare_core::discovery::DiscoverySource;
    use lanflare_proto::discovery::{DeviceType, ProtocolType};
    use std::collections::{HashMap, HashSet};
    use std::net::{IpAddr, Ipv4Addr};

    let mut sources = HashSet::new();
    sources.insert(DiscoverySource::Multicast);

    let p1 = DiscoveredPeer {
        fingerprint: "AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555FFFF66667777888899990000".to_string(),
        alias: "Bob's Workstation".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
        endpoints: vec![PeerEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)),
            scope_id: None,
            sources: sources.clone(),
            first_seen_ms: 1000,
            last_seen_ms: 1000,
        }],
        discovery_sources: sources.clone(),
        first_seen_ms: 1000,
        last_seen_ms: 1000,
        metadata: HashMap::new(),
    };

    let p2 = DiscoveredPeer {
        fingerprint: "1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF1234567890ABCDEF".to_string(),
        alias: "Alice's Phone".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Pixel 8".to_string()),
        device_type: Some(DeviceType::Mobile),
        port: 53318,
        protocol: ProtocolType::Https,
        download: false,
        endpoints: vec![PeerEndpoint {
            ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)),
            scope_id: None,
            sources: sources.clone(),
            first_seen_ms: 1000,
            last_seen_ms: 1000,
        }],
        discovery_sources: sources,
        first_seen_ms: 1000,
        last_seen_ms: 1000,
        metadata: HashMap::new(),
    };

    let peers = vec![p1, p2];

    // Query by alias exact
    assert_eq!(lanflare::commands::match_target_peer("Bob's Workstation", &peers).unwrap().alias, "Bob's Workstation");
    // Query by alias lowercase substring
    assert_eq!(lanflare::commands::match_target_peer("phone", &peers).unwrap().alias, "Alice's Phone");
    // Query by IP
    assert_eq!(lanflare::commands::match_target_peer("192.168.1.50", &peers).unwrap().alias, "Bob's Workstation");
    // Query by fingerprint prefix
    assert_eq!(lanflare::commands::match_target_peer("12345678", &peers).unwrap().alias, "Alice's Phone");
    // Non-matching query
    assert!(lanflare::commands::match_target_peer("Nonexistent", &peers).is_none());
    assert!(lanflare::commands::match_target_peer("", &peers).is_none());
}

#[tokio::test]
async fn test_cli_transfer_orchestration_e2e() {
    use std::time::Duration;
    use tokio::sync::mpsc;
    use uuid::Uuid;
    use lanflare_core::client::transfer::{OutgoingFile, TransferOptions, TransferOrchestrator};
    use lanflare_core::client::{LsClient, LsClientConfig};
    use lanflare_core::discovery::registry::PeerRegistry;
    use lanflare_core::server::start_server;
    use lanflare_proto::discovery::{DeviceType, ProtocolType, RegisterDtoV2};

    let temp_rx_dir = std::env::temp_dir().join(format!("cli_rx_{}", Uuid::new_v4()));
    let temp_tx_dir = std::env::temp_dir().join(format!("cli_tx_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_rx_dir).await.unwrap();
    tokio::fs::create_dir_all(&temp_tx_dir).await.unwrap();

    let test_file = temp_tx_dir.join("payload.txt");
    tokio::fs::write(&test_file, b"LanFlare CLI transfer verified!").await.unwrap();

    let outgoing = OutgoingFile::from_path(&test_file)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    // Spawn server with ephemeral identity
    let rx_identity = generate_self_signed().unwrap();
    let rx_config = AppConfig {
        alias: "CLI Receiver".to_string(),
        port: 0,
        download_dir: temp_rx_dir.clone(),
        auto_accept: true,
        pin: None,
        ..Default::default()
    };
    let registry = PeerRegistry::new();
    let (ev_tx, _ev_rx) = mpsc::channel(100);
    let server_handle = start_server(rx_config, Some(rx_identity.clone()), registry, Some(ev_tx))
        .await
        .unwrap();
    let bound_port = server_handle.bound_port();
    let base_url = format!("https://127.0.0.1:{bound_port}");

    // Client setup with receiver's pinned fingerprint
    let tx_identity = generate_self_signed().unwrap();
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(3),
        tls_identity: Some(tx_identity.clone()),
        target_fingerprint: Some(rx_identity.fingerprint.clone()),
    };
    let client = LsClient::new(client_config).unwrap();
    let sender_info = RegisterDtoV2 {
        alias: "CLI Sender".to_string(),
        version: "2.0".to_string(),
        device_model: Some("CLI Test".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: tx_identity.fingerprint.clone(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
    };
    let orchestrator = TransferOrchestrator::new(client, sender_info);

    let (progress_tx, mut progress_rx) = mpsc::channel(100);
    let mut tracker = lanflare::tui::TransferProgressTracker::new(false);

    let transfer_task = tokio::spawn(async move {
        orchestrator
            .transfer(
                &base_url,
                vec![outgoing],
                TransferOptions::default(),
                Some(progress_tx),
            )
            .await
    });

    while let Some(ev) = progress_rx.recv().await {
        tracker.handle_event(&ev);
    }

    transfer_task.await.unwrap().unwrap();

    // Verify file written
    let received_path = temp_rx_dir.join("payload.txt");
    assert!(received_path.exists());
    let content = tokio::fs::read(&received_path).await.unwrap();
    assert_eq!(content, b"LanFlare CLI transfer verified!");

    server_handle.stop();
}
