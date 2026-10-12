use std::time::Duration;
use tokio::sync::mpsc;
use uuid::Uuid;

use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{LsClient, LsClientConfig};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;
use lanflare_core::discovery::registry::PeerRegistry;
use lanflare_core::server::start_server;
use lanflare_proto::discovery::{DeviceType, ProtocolType, RegisterDtoV2};

#[tokio::test]
async fn test_plain_http_multi_file_transfer_e2e() {
    let temp_download_dir =
        std::env::temp_dir().join(format!("lanflare_e2e_rx_{}", Uuid::new_v4()));
    let temp_upload_dir =
        std::env::temp_dir().join(format!("lanflare_e2e_tx_{}", Uuid::new_v4()));

    tokio::fs::create_dir_all(&temp_download_dir).await.unwrap();
    tokio::fs::create_dir_all(&temp_upload_dir).await.unwrap();

    // 1. Prepare files to send
    let file1_path = temp_upload_dir.join("doc.txt");
    let file2_path = temp_upload_dir.join("binary.dat");
    let file3_path = temp_upload_dir.join("nested.json");

    let file1_content = b"This is a test document transferred over LanFlare HTTP.".to_vec();
    let file2_content = vec![0xAB; 256 * 1024]; // 256 KB
    let file3_content = br#"{"key": "value", "speed": 1000}"#.to_vec();

    tokio::fs::write(&file1_path, &file1_content).await.unwrap();
    tokio::fs::write(&file2_path, &file2_content).await.unwrap();
    tokio::fs::write(&file3_path, &file3_content).await.unwrap();

    let out1 = OutgoingFile::from_path(&file1_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();
    let out2 = OutgoingFile::from_path(&file2_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();
    let out3 = OutgoingFile::from_path(&file3_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    let files_to_send = vec![out1, out2, out3];

    // 2. Start receiver server
    let server_config = AppConfig {
        alias: "Receiver Node".to_string(),
        port: 0,
        download_dir: temp_download_dir.clone(),
        auto_accept: true,
        pin: None,
    };
    let registry = PeerRegistry::new();
    let (server_ev_tx, _server_ev_rx) = mpsc::channel(100);

    let server_handle = start_server(server_config, None, registry, Some(server_ev_tx))
        .await
        .expect("Server should bind");
    let server_port = server_handle.bound_port();
    let base_url = format!("http://127.0.0.1:{server_port}");

    // 3. Setup client and orchestrator
    let client = LsClient::new(LsClientConfig::default()).expect("Client should build");

    // Test info endpoint first
    let info = client.get_info(&base_url).await.expect("Get info should succeed");
    assert_eq!(info.alias, "Receiver Node");

    let sender_info = RegisterDtoV2 {
        alias: "Sender Node".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "SENDER_FP".to_string(),
        port: 53317,
        protocol: ProtocolType::Http,
        download: false,
    };

    // Test register endpoint
    let reg_resp = client
        .register(&base_url, &sender_info)
        .await
        .expect("Register should succeed");
    assert_eq!(reg_resp.alias, "Receiver Node");

    let orchestrator = TransferOrchestrator::new(client, sender_info);

    let (progress_tx, mut progress_rx) = mpsc::channel(100);
    let options = TransferOptions {
        max_concurrency: 2,
        pin: None,
        cancel_token: None,
        progress_interval: Duration::from_millis(10),
        ..Default::default()
    };

    // 4. Run transfer
    orchestrator
        .transfer(&base_url, files_to_send, options, Some(progress_tx))
        .await
        .expect("Transfer should succeed");

    // 5. Verify progress events
    let mut saw_started = false;
    let mut finished_files = 0;
    let mut saw_completed = false;

    while let Ok(progress) = progress_rx.try_recv() {
        match progress {
            TransferProgress::Started { total_bytes, .. } => {
                assert_eq!(
                    total_bytes,
                    (file1_content.len() + file2_content.len() + file3_content.len()) as u64
                );
                saw_started = true;
            }
            TransferProgress::FileFinished { .. } => {
                finished_files += 1;
            }
            TransferProgress::SessionCompleted { total_files, .. } => {
                assert_eq!(total_files, 3);
                saw_completed = true;
            }
            _ => {}
        }
    }

    assert!(saw_started, "Must observe Started progress event");
    assert_eq!(finished_files, 3, "All 3 files must finish");
    assert!(saw_completed, "Must observe SessionCompleted event");

    // 6. Verify received files on disk
    let rx_file1 = temp_download_dir.join("doc.txt");
    let rx_file2 = temp_download_dir.join("binary.dat");
    let rx_file3 = temp_download_dir.join("nested.json");

    assert!(rx_file1.exists());
    assert!(rx_file2.exists());
    assert!(rx_file3.exists());

    let rx_content1 = tokio::fs::read(&rx_file1).await.unwrap();
    let rx_content2 = tokio::fs::read(&rx_file2).await.unwrap();
    let rx_content3 = tokio::fs::read(&rx_file3).await.unwrap();

    assert_eq!(rx_content1, file1_content);
    assert_eq!(rx_content2, file2_content);
    assert_eq!(rx_content3, file3_content);

    // Stop server and clean up
    server_handle.stop();
    server_handle.wait().await;
    let _ = tokio::fs::remove_dir_all(&temp_download_dir).await;
    let _ = tokio::fs::remove_dir_all(&temp_upload_dir).await;
}

#[tokio::test]
async fn test_https_mtls_pin_protected_transfer_e2e() {
    let temp_download_dir =
        std::env::temp_dir().join(format!("lanflare_e2e_tls_rx_{}", Uuid::new_v4()));
    let temp_upload_dir =
        std::env::temp_dir().join(format!("lanflare_e2e_tls_tx_{}", Uuid::new_v4()));

    tokio::fs::create_dir_all(&temp_download_dir).await.unwrap();
    tokio::fs::create_dir_all(&temp_upload_dir).await.unwrap();

    let server_ident = generate_self_signed().unwrap();
    let client_ident = generate_self_signed().unwrap();

    let file_path = temp_upload_dir.join("secure.bin");
    let content = b"Confidential payload over mutual TLS with PIN verification".to_vec();
    tokio::fs::write(&file_path, &content).await.unwrap();

    let out_file = OutgoingFile::from_path(&file_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    // 1. Start HTTPS server with PIN
    let pin_code = "7890".to_string();
    let server_config = AppConfig {
        alias: "Secure Receiver".to_string(),
        port: 0,
        download_dir: temp_download_dir.clone(),
        auto_accept: true,
        pin: Some(pin_code.clone()),
    };

    let registry = PeerRegistry::new();
    let server_handle = start_server(
        server_config,
        Some(server_ident.clone()),
        registry,
        None,
    )
    .await
    .expect("HTTPS server should bind");

    let server_port = server_handle.bound_port();
    let base_url = format!("https://127.0.0.1:{server_port}");

    // 2. Build client configured with pinned server fingerprint and client identity
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(5),
        tls_identity: Some(client_ident.clone()),
        target_fingerprint: Some(server_ident.fingerprint.clone()),
    };

    let client = LsClient::new(client_config).expect("mTLS client should build cleanly");

    let sender_info = RegisterDtoV2 {
        alias: "Secure Sender".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Desktop".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: client_ident.fingerprint.clone(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
    };

    let orchestrator = TransferOrchestrator::new(client.clone(), sender_info.clone());

    // 3. Test wrong PIN -> should fail with UnauthorizedPin
    let bad_pin_options = TransferOptions {
        max_concurrency: 1,
        pin: Some("0000".to_string()),
        cancel_token: None,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let res_bad_pin = orchestrator
        .transfer(&base_url, vec![out_file.clone()], bad_pin_options, None)
        .await;
    assert!(res_bad_pin.is_err());

    // 4. Test correct PIN -> should succeed
    let good_pin_options = TransferOptions {
        max_concurrency: 1,
        pin: Some(pin_code),
        cancel_token: None,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    orchestrator
        .transfer(&base_url, vec![out_file], good_pin_options, None)
        .await
        .expect("Transfer with correct PIN over HTTPS mTLS must succeed");

    // 5. Verify received file
    let rx_file = temp_download_dir.join("secure.bin");
    assert!(rx_file.exists());
    let rx_content = tokio::fs::read(&rx_file).await.unwrap();
    assert_eq!(rx_content, content);

    server_handle.stop();
    server_handle.wait().await;
    let _ = tokio::fs::remove_dir_all(&temp_download_dir).await;
    let _ = tokio::fs::remove_dir_all(&temp_upload_dir).await;
}
