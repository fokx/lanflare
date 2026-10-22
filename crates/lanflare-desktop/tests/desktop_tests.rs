use lanflare_desktop::state::DesktopState;
use std::sync::Arc;
use std::path::PathBuf;

#[tokio::test]
async fn test_desktop_state_initialization() {
    let state = DesktopState::new().expect("State init must succeed");
    assert_eq!(state.identity.read().await.fingerprint.len(), 64);
    assert_eq!(state.registry.count(), 0);

    let cfg = state.config.read().await;
    assert_eq!(cfg.port, 53317);
    assert!(!cfg.auto_accept);
    assert!(cfg.pin.is_none());
}

#[tokio::test]
async fn test_desktop_config_update() {
    let state = Arc::new(DesktopState::new().expect("State init must succeed"));
    
    // Simulate updating config via payload
    {
        let mut cfg = state.config.write().await;
        cfg.alias = "Custom Device".to_string();
        cfg.port = 54321;
        cfg.auto_accept = true;
        cfg.pin = Some("1234".to_string());
        cfg.download_dir = PathBuf::from("/tmp/lanflare_downloads");
    }

    let cfg = state.config.read().await;
    assert_eq!(cfg.alias, "Custom Device");
    assert_eq!(cfg.port, 54321);
    assert!(cfg.auto_accept);
    assert_eq!(cfg.pin.as_deref(), Some("1234"));
    assert_eq!(cfg.download_dir, PathBuf::from("/tmp/lanflare_downloads"));
}

#[tokio::test]
async fn test_discovered_peer_dto_conversion() {
    use lanflare_core::discovery::registry::{DiscoveredPeer, DiscoverySource, PeerEndpoint};
    use lanflare_desktop::commands::DiscoveredPeerDto;
    use lanflare_proto::discovery::{DeviceType, ProtocolType};
    use std::collections::{HashMap, HashSet};
    use std::net::IpAddr;

    let mut sources = HashSet::new();
    sources.insert(DiscoverySource::Multicast);

    let ep = PeerEndpoint {
        ip: "192.168.1.150".parse::<IpAddr>().unwrap(),
        scope_id: None,
        sources: sources.clone(),
        first_seen_ms: 1000,
        last_seen_ms: 2000,
    };

    let peer = DiscoveredPeer {
        fingerprint: "AABBCCDD".to_string(),
        alias: "Test Node".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux Desktop".to_string()),
        device_type: Some(DeviceType::Desktop),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
        endpoints: vec![ep],
        discovery_sources: sources,
        first_seen_ms: 1000,
        last_seen_ms: 2000,
        metadata: HashMap::new(),
        hop_count: None,
        route_path: None,
    };

    let dto = DiscoveredPeerDto::from(&peer);
    assert_eq!(dto.fingerprint, "AABBCCDD");
    assert_eq!(dto.endpoints, vec!["192.168.1.150:53317".to_string()]);
    assert_eq!(dto.sources, 1);
    assert_eq!(dto.last_seen, 2000);

    // Verify JSON serialization camelCase keys match frontend expectations
    let json_val = serde_json::to_value(&dto).expect("Serialization must succeed");
    assert_eq!(json_val["endpoints"][0], "192.168.1.150:53317");
    assert_eq!(json_val["deviceModel"], "Linux Desktop");
    assert_eq!(json_val["deviceType"], "desktop");
    assert_eq!(json_val["sources"], 1);
    assert_eq!(json_val["lastSeen"], 2000);
}

#[tokio::test]
async fn test_pending_incoming_session_and_decision() {
    use lanflare_core::server::PrepareUploadDecision;
    use lanflare_desktop::state::{IncomingFileInfo, PendingIncomingSession};
    use tokio::sync::oneshot;

    let (decision_tx, decision_rx) = oneshot::channel();
    let pending = PendingIncomingSession {
        session_id: "sess-123".to_string(),
        sender_alias: "Alice".to_string(),
        sender_ip: "192.168.1.50".to_string(),
        files: vec![IncomingFileInfo {
            id: "file-1".to_string(),
            file_name: "photo.jpg".to_string(),
            size: 1024,
            file_type: "image/jpeg".to_string(),
            path: None,
        }],
        decision_tx: Some(decision_tx),
    };

    // Simulate accept decision
    let file_ids: Vec<String> = pending.files.iter().map(|f| f.id.clone()).collect();
    let tx = pending.decision_tx.unwrap();
    let _ = tx.send(PrepareUploadDecision::Accept(file_ids));

    let decision = decision_rx.await.expect("Decision must be received");
    assert_eq!(decision, PrepareUploadDecision::Accept(vec!["file-1".to_string()]));
}

#[tokio::test]
async fn test_send_files_payload_supports_endpoint_fallback() {
    use lanflare_desktop::commands::SendFilesPayload;

    let payload = SendFilesPayload {
        target_fingerprint: "NONEXISTENT_FP_12345".to_string(),
        target_endpoint: Some("127.0.0.1:53317".to_string()),
        files: vec!["/path/to/test.txt".to_string()],
        pin: None,
    };

    let json_val = serde_json::to_value(&payload).expect("Serialization must succeed");
    assert_eq!(json_val["targetFingerprint"], "NONEXISTENT_FP_12345");
    assert_eq!(json_val["targetEndpoint"], "127.0.0.1:53317");
    assert_eq!(json_val["files"][0], "/path/to/test.txt");
}

#[tokio::test]
async fn test_transfer_progress_payload_failure_and_rejection() {
    use lanflare_desktop::commands::TransferProgressPayload;

    let payload = TransferProgressPayload {
        session_id: "test-session-456".to_string(),
        event_type: "session_failed".to_string(),
        file_id: None,
        file_name: None,
        file_bytes_sent: None,
        file_total_bytes: None,
        session_bytes_sent: None,
        session_total_bytes: None,
        bytes_per_second: None,
        eta_seconds: None,
        error: Some("Transfer request was declined by the recipient.".to_string()),
    };

    let json_val = serde_json::to_value(&payload).expect("Serialization must succeed");
    assert_eq!(json_val["sessionId"], "test-session-456");
    assert_eq!(json_val["eventType"], "session_failed");
    assert_eq!(
        json_val["error"],
        "Transfer request was declined by the recipient."
    );
}
