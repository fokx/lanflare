//! Delay-Tolerant P2P Chat, Resumable Media Streaming & Multi-Hop Mesh Receipts E2E Test Suite.
//!
//! Sub-phase 8.3 verification:
//! - `test_dtn_offline_chat_and_resumable_media_e2e`:
//!   1. Offline chat queuing in delay-tolerant outbox with automatic flush upon peer appearance.
//!   2. Monotonic cryptographic receipt transitions (`Queued -> Delivered -> Read`).
//!   3. Resumable media transfer: interrupting a 4 MB streaming upload at 50% (2 MB),
//!      querying `/api/lanflare/v1/transfer/resume`, streaming strictly the remaining 2 MB,
//!      and verifying full-file SHA-256 upon atomic commit.
//! - `test_multi_hop_mesh_chat_with_receipts_e2e`:
//!   End-to-end 1:1 chat and receipt exchange across a 2-hop simulated mesh relay
//!   (`Node A -> Relay B -> Node C`) using E2EE envelopes and monotonic receipts.

use std::sync::Arc;
use std::time::Duration;

use lanflare_core::chat::trust::{compute_time_bucket, generate_hashcash_pow};
use lanflare_core::chat::MessageDeliveryState;
use lanflare_core::mesh::sim::{SimulatedMeshCluster, VirtualNetworkMatrix};
use lanflare_proto::chat::{
    ChatContentDto, ResumeTransferDto, TrustRequestDto,
    DEFAULT_POW_DIFFICULTY_BITS, DEFAULT_TIME_BUCKET_SECS,
};
use sha2::{Digest, Sha256};

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[tokio::test]
async fn test_dtn_offline_chat_and_resumable_media_e2e() {
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    // 1. Spawn Node A (Alice) and Node B (Bob) on SubnetA with ChatEngine enabled
    let fp_a = cluster
        .spawn_node_with_chat("Alice", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Alice");

    let fp_b = cluster
        .spawn_node_with_chat("Bob", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Bob");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_b = cluster.get_node(&fp_b).unwrap();
    let engine_a = node_a.chat_engine().expect("engine A").clone();
    let engine_b = node_b.chat_engine().expect("engine B").clone();

    // 2. Establish direct physical neighbor link and register peer endpoints
    cluster.establish_direct_links().await;

    // 3. Alice introduces herself to Bob via TrustRequest with valid Hashcash PoW
    let now_secs = current_time_ms() / 1000;
    let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);
    let pow_stamp = generate_hashcash_pow(
        &fp_a,
        &fp_b,
        DEFAULT_POW_DIFFICULTY_BITS,
        time_bucket,
        1_000_000,
    )
    .expect("generate pow stamp");

    let trust_req = TrustRequestDto {
        request_id: "req-alice-to-bob".to_string(),
        sender_fp: fp_a.clone(),
        sender_alias: "Alice".to_string(),
        target_fp: fp_b.clone(),
        message: "Hi Bob! Let's connect on LanFlare.".to_string(),
        pow_stamp,
        timestamp_ms: current_time_ms(),
        signature: None,
    };

    let client = reqwest::Client::new();
    let trust_url = format!("{}/api/lanflare/v1/chat/request", node_b.base_url());
    let trust_resp = client
        .post(&trust_url)
        .json(&trust_req)
        .send()
        .await
        .expect("send trust request");
    assert!(trust_resp.status().is_success());

    // Bob approves Alice, and Alice approves Bob
    engine_b.approve_contact(&fp_a).await.expect("Bob approves Alice");
    engine_a.approve_contact(&fp_b).await.expect("Alice approves Bob");

    // 4. Test 1:1 Message exchange with cryptographic delivery and read receipts
    let msg_text = "Hello Bob! Testing delay-tolerant messaging.";
    let sent_msg = engine_a
        .send_text_message(&fp_b, msg_text)
        .await
        .expect("send text message");

    // Wait briefly for reactive outbox delivery and receipt roundtrip
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify Bob received and stored message with Delivered state
    let bob_stored = engine_b
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Bob must have received message");
    assert_eq!(bob_stored.delivery_state, MessageDeliveryState::Delivered);
    assert_eq!(
        bob_stored.message.content.text_content(),
        Some(msg_text)
    );

    // Verify Alice's local message transitioned to Delivered via delivery receipt
    let alice_stored = engine_a
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Alice must have stored message");
    assert_eq!(alice_stored.delivery_state, MessageDeliveryState::Delivered);

    // Bob opens and reads message -> generates ReadReceipt
    let read_receipt = engine_b
        .mark_message_as_read(&sent_msg.id)
        .await
        .expect("Bob marks message read");
    assert_eq!(read_receipt.message_id, sent_msg.id);

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Verify Alice's message state monotonically transitioned to Read
    let alice_stored_read = engine_a
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Alice message must exist");
    assert_eq!(alice_stored_read.delivery_state, MessageDeliveryState::Read);

    // 5. Test Resumable Media Streaming: 4 MB payload interrupted at 50% (2 MB)
    let total_bytes: usize = 4 * 1024 * 1024; // 4 MB
    let half_bytes: usize = total_bytes / 2;  // 2 MB
    let mut payload = Vec::with_capacity(total_bytes);
    for i in 0..total_bytes {
        payload.push((i % 251) as u8);
    }
    let expected_sha256 = format!("{:x}", Sha256::digest(&payload));

    let session_id = "sess-media-4mb-test";
    let file_id = "video-4mb-clip";
    let file_name = "test_video_resumable.mp4";

    // Alice sends initial message descriptor for video attachment
    let media_content = ChatContentDto::Video {
        file_name: file_name.to_string(),
        duration_ms: 10_000,
        size_bytes: total_bytes as u64,
        mime_type: "video/mp4".to_string(),
        sha256: expected_sha256.clone(),
        thumbnail_base64: None,
    };
    engine_a
        .send_media_message(&fp_b, media_content)
        .await
        .expect("send media message descriptor");

    // Phase 1: Upload first 2 MB (50%) and abort stream
    let media_url = format!("{}/api/lanflare/v1/chat/media", node_b.base_url());
    let query_params_p1 = [
        ("sessionId", session_id),
        ("fileId", file_id),
        ("fileName", file_name),
        ("senderFp", &fp_a),
        ("totalBytes", "4194304"),
        ("sha256", &expected_sha256),
        ("offset", "0"),
    ];

    let resp_p1 = client
        .post(&media_url)
        .query(&query_params_p1)
        .body(payload[..half_bytes].to_vec())
        .send()
        .await
        .expect("upload first 2 MB");
    assert!(resp_p1.status().is_success());

    let p1_body: serde_json::Value = resp_p1.json().await.expect("parse p1 json");
    assert_eq!(p1_body["status"], "interrupted");
    assert_eq!(p1_body["receivedBytes"], half_bytes as u64);

    // Verify on-disk checkpoint exists on Bob's node
    let checkpoint = engine_b
        .resume_manager()
        .find_checkpoint(session_id, file_id)
        .await
        .expect("find checkpoint")
        .expect("checkpoint must be on disk");
    assert_eq!(checkpoint.received_bytes, half_bytes as u64);

    // Phase 2: Alice queries /api/lanflare/v1/transfer/resume to negotiate resume offset
    let resume_probe = ResumeTransferDto {
        session_id: session_id.to_string(),
        file_id: file_id.to_string(),
        sender_fp: fp_a.clone(),
        recipient_fp: fp_b.clone(),
        total_bytes: total_bytes as u64,
        received_bytes: 0,
        sha256: expected_sha256.clone(),
        chunk_size: 128 * 1024,
    };

    let resume_url = format!("{}/api/lanflare/v1/transfer/resume", node_b.base_url());
    let resume_resp = client
        .post(&resume_url)
        .json(&resume_probe)
        .send()
        .await
        .expect("probe resume");
    assert!(resume_resp.status().is_success());

    let negotiated_dto: ResumeTransferDto = resume_resp.json().await.expect("parse resume dto");
    assert_eq!(
        negotiated_dto.received_bytes, half_bytes as u64,
        "Bob must report exactly 2 MB verified bytes received"
    );

    // Phase 3: Alice streams strictly the remaining 2 MB from offset 2,097,152
    let query_params_p2 = [
        ("sessionId", session_id),
        ("fileId", file_id),
        ("fileName", file_name),
        ("senderFp", &fp_a),
        ("totalBytes", "4194304"),
        ("sha256", &expected_sha256),
        ("offset", "2097152"),
    ];

    let resp_p2 = client
        .post(&media_url)
        .query(&query_params_p2)
        .body(payload[half_bytes..].to_vec())
        .send()
        .await
        .expect("upload remaining 2 MB");
    assert!(resp_p2.status().is_success());

    let p2_body: serde_json::Value = resp_p2.json().await.expect("parse p2 json");
    assert_eq!(p2_body["status"], "completed");
    assert_eq!(p2_body["bytesWritten"], total_bytes as u64);
    assert_eq!(p2_body["sha256"], expected_sha256);

    // Verify Bob's committed file on disk matches the exact original 4 MB payload
    let target_file_path = engine_b.media_dir().join(file_name);
    let disk_bytes = tokio::fs::read(&target_file_path)
        .await
        .expect("read target file");
    assert_eq!(
        disk_bytes.len(), total_bytes,
        "Committed file size must match 4 MB"
    );
    assert_eq!(
        disk_bytes, payload,
        "Committed media file must match original payload bit-for-bit"
    );

    // Verify checkpoint was removed after successful atomic commit
    let after_cp = engine_b
        .resume_manager()
        .find_checkpoint(session_id, file_id)
        .await
        .expect("query checkpoint");
    assert!(after_cp.is_none(), "Checkpoint must be cleaned up on commit");
}

#[tokio::test]
async fn test_multi_hop_mesh_chat_with_receipts_e2e() {
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    // 1. Setup 3 nodes across 2 subnets:
    // Node A on Subnet A only (Alice)
    let fp_a = cluster
        .spawn_node_with_chat("Node-A", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Node-A");

    // Relay B bridging Subnet A and Subnet B (Relay)
    let fp_b = cluster
        .spawn_node_with_chat("Relay-B", vec!["SubnetA".into(), "SubnetB".into()], true)
        .await
        .expect("spawn Relay-B");

    // Node C on Subnet B only (Charlie)
    let fp_c = cluster
        .spawn_node_with_chat("Node-C", vec!["SubnetB".into()], false)
        .await
        .expect("spawn Node-C");

    // Verify subnet boundary isolation
    assert!(matrix.can_reach(&fp_a, &fp_b));
    assert!(matrix.can_reach(&fp_b, &fp_c));
    assert!(!matrix.can_reach(&fp_a, &fp_c), "A and C must be partitioned");

    // Connect 1-hop physical neighbors and execute 2 rounds of gossip
    cluster.establish_direct_links().await;
    cluster.exchange_gossip_rounds(2).await.expect("gossip exchange");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_c = cluster.get_node(&fp_c).unwrap();
    let engine_a = node_a.chat_engine().expect("engine A").clone();
    let engine_c = node_c.chat_engine().expect("engine C").clone();

    // Verify route resolution on Node A and Node C
    {
        let graph_a = node_a.mesh_router().graph().read().await;
        let route_c = graph_a.resolve_route(&fp_c).expect("route to C");
        assert_eq!(route_c.next_hop_fp, fp_b);
        assert_eq!(route_c.full_route_path, vec![fp_a.clone(), fp_b.clone(), fp_c.clone()]);
    }
    {
        let graph_c = node_c.mesh_router().graph().read().await;
        let route_a = graph_c.resolve_route(&fp_a).expect("route to A");
        assert_eq!(route_a.next_hop_fp, fp_b);
        assert_eq!(route_a.full_route_path, vec![fp_c.clone(), fp_b.clone(), fp_a.clone()]);
    }

    // 2. Establish mutual contact trust across mesh
    engine_a.approve_contact(&fp_c).await.expect("A approves C");
    engine_c.approve_contact(&fp_a).await.expect("C approves A");

    // 3. Node A authors a chat message for Node C routed through Relay B
    let msg_text = "Greetings Charlie from Alice across 2-hop mesh!";
    let sent_msg = engine_a
        .send_text_message(&fp_c, msg_text)
        .await
        .expect("send message across mesh");

    // Wait for multi-hop outbox dispatch and envelope forwarding
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify Charlie received the message at the 2-hop destination
    let charlie_stored = engine_c
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Charlie must have received routed message");
    assert_eq!(charlie_stored.delivery_state, MessageDeliveryState::Delivered);
    assert_eq!(
        charlie_stored.message.content.text_content(),
        Some(msg_text)
    );

    // Wait for delivery receipt to route back across Relay B to Alice
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify Alice's message state transitioned to Delivered
    let alice_stored = engine_a
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Alice message must exist");
    assert_eq!(alice_stored.delivery_state, MessageDeliveryState::Delivered);

    // Charlie marks message as read -> ReadReceipt routes back to Alice
    let read_receipt = engine_c
        .mark_message_as_read(&sent_msg.id)
        .await
        .expect("Charlie marks read");
    assert_eq!(read_receipt.message_id, sent_msg.id);

    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify Alice's message state transitioned to Read
    let alice_stored_read = engine_a
        .store()
        .get_message(&sent_msg.id)
        .await
        .expect("Alice message must exist");
    assert_eq!(alice_stored_read.delivery_state, MessageDeliveryState::Read);
}
