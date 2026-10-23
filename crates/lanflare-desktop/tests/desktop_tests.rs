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

#[tokio::test]
async fn test_desktop_mesh_and_chat_state_commands() {
    use lanflare_desktop::commands::*;
    use lanflare_proto::toolbox::ClipboardPayloadDto;

    // 1. Initialize DesktopState
    let state = Arc::new(DesktopState::new().expect("DesktopState initialization must succeed"));

    // Verify initial state properties
    assert_eq!(state.mesh_router.local_fingerprint().len(), 64);
    assert_eq!(state.device_ring.ring_id(), "lanflare_ring");
    assert_eq!(state.catalog.total_items(), 0);

    // 2. Mesh & Simulator Commands
    let topo = get_mesh_topology_internal(&state).await.expect("Get topology must succeed");
    assert_eq!(topo.local_fingerprint, state.mesh_router.local_fingerprint());

    let sim_payload = StartSimulationPayload {
        node_count: Some(2),
        subnets: Some(vec!["SubnetA".to_string()]),
        latency_ms: Some(10),
    };
    let sim_topo = start_mesh_simulation_internal(&state, Some(sim_payload))
        .await
        .expect("Start mesh simulation must succeed");
    assert!(sim_topo.nodes.len() >= 2);

    // Test toggle link
    let node_fps: Vec<String> = sim_topo.nodes.iter().map(|n| n.fingerprint.clone()).collect();
    let fp_a = &node_fps[0];
    let fp_b = &node_fps[1];

    let severed = toggle_mesh_link_internal(
        &state,
        ToggleMeshLinkPayload {
            from_fp: fp_a.clone(),
            to_fp: fp_b.clone(),
            severed: Some(true),
        },
    )
    .await
    .expect("Sever link must succeed");
    assert!(severed);
    assert!(state.virtual_matrix.is_link_severed(fp_a, fp_b));

    let restored = toggle_mesh_link_internal(
        &state,
        ToggleMeshLinkPayload {
            from_fp: fp_a.clone(),
            to_fp: fp_b.clone(),
            severed: Some(false),
        },
    )
    .await
    .expect("Restore link must succeed");
    assert!(!restored);
    assert!(!state.virtual_matrix.is_link_severed(fp_a, fp_b));

    // 3. Encrypted Chat & Stranger Trust Commands
    let temp_chat_dir = std::env::temp_dir().join(format!("lanflare_test_chat_{}", uuid::Uuid::new_v4()));
    *state.chat_dir.write().await = temp_chat_dir;

    let target_fp = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB".to_string();

    // Send introduction to stranger (Hashcash PoW)
    let stranger_msg = send_chat_message_internal(
        &state,
        SendChatMessagePayload {
            target: target_fp.clone(),
            message: "Hello from LanFlare Desktop!".to_string(),
            intro: None,
        },
    )
    .await
    .expect("Send stranger handshake must succeed");
    assert_eq!(stranger_msg.recipient_fp, target_fp);

    // List conversations
    let convs = list_chat_conversations_internal(&state)
        .await
        .expect("List conversations must succeed");
    assert!(!convs.is_empty());
    assert!(convs.iter().any(|c| c.target_fp == target_fp));

    // Approve contact
    respond_trust_request_internal(
        &state,
        RespondTrustRequestPayload {
            peer_fingerprint: target_fp.clone(),
            approve: true,
        },
    )
    .await
    .expect("Approve contact must succeed");

    // Send trusted text message
    let trusted_msg = send_chat_message_internal(
        &state,
        SendChatMessagePayload {
            target: target_fp.clone(),
            message: "Glad to connect!".to_string(),
            intro: None,
        },
    )
    .await
    .expect("Send trusted message must succeed");
    assert_eq!(trusted_msg.recipient_fp, target_fp);

    // Query chat history
    let history = get_chat_history_internal(
        &state,
        GetChatHistoryPayload {
            conversation_id: target_fp.clone(),
            limit: Some(10),
        },
    )
    .await
    .expect("Get chat history must succeed");
    assert!(!history.is_empty());

    // Create decentralized chat group
    let group_roster = create_chat_group_internal(
        &state,
        CreateChatGroupPayload {
            group_name: "Developers".to_string(),
            member_fingerprints: vec![fp_b.clone()],
        },
    )
    .await
    .expect("Create chat group must succeed");
    assert_eq!(group_roster.group_name, "Developers");

    // 4. LAN Toolbox Commands
    // Universal Clipboard state
    let clip_state = get_clipboard_state_internal(&state)
        .expect("Get clipboard state must succeed");
    assert_eq!(clip_state.ring_id, "lanflare_ring");

    // Broadcast clipboard payload
    let synced = broadcast_clipboard_internal(
        &state,
        BroadcastClipboardPayload {
            payload: ClipboardPayloadDto::Text {
                text: "Synced Clipboard Text".to_string(),
            },
        },
    )
    .await
    .expect("Broadcast clipboard must succeed");
    assert!(synced);

    // Federated file search
    let search_hits = search_federated_files_internal(
        &state,
        SearchFederatedFilesPayload {
            query: "report".to_string(),
            pin: None,
            timeout_secs: Some(1),
        },
    )
    .await
    .expect("Search federated files must succeed");
    // Local catalog has no files matching "report" yet
    assert!(search_hits.is_empty());

    // CRDT Collaborative Scratchpad
    let doc_sync = get_collab_doc_internal(
        &state,
        GetCollabDocPayload {
            doc_id: "doc-101".to_string(),
            title: Some("Shared Notes".to_string()),
        },
    )
    .await
    .expect("Get collab doc must succeed");
    assert_eq!(doc_sync.doc_id, "doc-101");
    assert_eq!(doc_sync.title, "Shared Notes");

    // Apply incremental CRDT edit
    let edit = lanflare_proto::toolbox::ScratchpadEditDto {
        doc_id: "doc-101".to_string(),
        edit_id: "edit-1".to_string(),
        author_fp: state.mesh_router.local_fingerprint().to_string(),
        lamport: 1,
        timestamp_ms: 1000,
        op: lanflare_proto::toolbox::ScratchpadOpDto::Insert {
            after_id: None,
            atom_id: lanflare_proto::toolbox::AtomId {
                lamport: 1,
                author_fp: state.mesh_router.local_fingerprint().to_string(),
                counter: 0,
            },
            value: 'X',
        },
    };
    let edit_applied = apply_collab_edit_internal(
        &state,
        ApplyCollabEditPayload { edit },
    )
    .await
    .expect("Apply collab edit must succeed");
    assert!(edit_applied);

    let doc_updated = get_collab_doc_internal(
        &state,
        GetCollabDocPayload {
            doc_id: "doc-101".to_string(),
            title: None,
        },
    )
    .await
    .expect("Get updated collab doc must succeed");
    assert_eq!(doc_updated.atoms.len(), 1);
    assert_eq!(doc_updated.atoms[0].value, 'X');
}
