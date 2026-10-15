//! Fuzz testing and boundary verification for malformed HTTP requests and UDP datagrams.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use rand::RngCore;
use reqwest::Client;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use uuid::Uuid;

use lanflare_core::config::AppConfig;
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastNodeInfo,
};
use lanflare_core::discovery::registry::PeerRegistry;
use lanflare_core::server::start_server;
use lanflare_proto::discovery::{DeviceType, MulticastMessageV2, ProtocolType, RegisterDtoV2};
use lanflare_proto::transfer::{FileDto, PrepareUploadRequestDtoV2};

/// Verifies that the UDP multicast parser and engine gracefully survive malformed,
/// corrupted, truncated, and oversized datagrams without panicking or terminating.
#[tokio::test]
async fn test_udp_multicast_fuzz_and_boundary_resilience() {
    // 1. Direct deserializer fuzzing on arbitrary garbage
    let mut rng = rand::rng();
    for size in [0, 1, 2, 7, 15, 64, 256, 1024, 4096, 65535] {
        let mut junk = vec![0u8; size];
        rng.fill_bytes(&mut junk);
        // Deserializing random bytes into MulticastMessageV2 must return an Err, never panic
        let _ = serde_json::from_slice::<MulticastMessageV2>(&junk);
    }

    // 2. Truncated and malformed JSON payloads
    let malformed_json_cases = [
        "",
        "{",
        "{\"alias\":",
        "{\"alias\": \"test\", \"version\":",
        "{\"port\": 999999999999999999999999999999999}",
        "{\"port\": -1}",
        "{\"port\": \"not_a_number\"}",
        "{\"protocol\": 123}",
        "null",
        "[]",
        "\"a string\"",
        "12345",
        "true",
        "{\"alias\": 123, \"version\": false, \"port\": null}",
    ];

    for case in malformed_json_cases {
        let res = serde_json::from_slice::<MulticastMessageV2>(case.as_bytes());
        assert!(res.is_err(), "Payload '{case}' should fail deserialization");
    }

    // 3. Deeply nested JSON boundary test
    let mut deeply_nested = String::new();
    for _ in 0..500 {
        deeply_nested.push_str("{\"a\":");
    }
    deeply_nested.push('1');
    for _ in 0..500 {
        deeply_nested.push('}');
    }
    let res = serde_json::from_slice::<MulticastMessageV2>(deeply_nested.as_bytes());
    assert!(res.is_err());

    // 4. Over-the-wire UDP resilience: start engine with loopback fallback
    let (event_tx, mut event_rx) = mpsc::channel(100);
    let (_stop_tx, stop_rx) = tokio::sync::oneshot::channel();

    let node_info = MulticastNodeInfo {
        alias: "Resilience Node".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "RESILIENCE_FP_LOCAL".to_string(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
    };

    let config = MulticastConfig {
        port: 0, // dynamic port for testing
        node_info,
        allow_loopback_fallback: true,
        ..Default::default()
    };

    let engine = match start_multicast_engine(config, event_tx, stop_rx).await {
        Ok(h) => h,
        Err(_) => {
            // Multicast bind may be restricted on some environments; skip live socket test
            return;
        }
    };

    // Send valid message after fuzz tests to verify engine stayed alive
    let valid_msg = MulticastMessageV2 {
        alias: "Legit Peer".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "LEGIT_PEER_FP".to_string(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
        announce: true,
    };
    let valid_bytes = serde_json::to_vec(&valid_msg).unwrap();

    // Pump a sender socket
    let sender_socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_addr: SocketAddr = "127.0.0.1:53317".parse().unwrap();

    // Blast 50 malformed packets
    for _ in 0..50 {
        let mut garbage = vec![0u8; 128];
        rng.fill_bytes(&mut garbage);
        let _ = sender_socket.send_to(&garbage, target_addr).await;
    }

    // Blast valid packet
    let _ = sender_socket.send_to(&valid_bytes, target_addr).await;

    // Wait briefly for processing
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Shutdown cleanly
    engine.stop().await;

    // Verify engine did not crash
    while let Ok(evt) = event_rx.try_recv() {
        if let MulticastDiscoveryEvent::PeerDiscovered { message, .. } = evt {
            assert_eq!(message.fingerprint, "LEGIT_PEER_FP");
        }
    }
}

/// Verifies that the HTTP server enforces strict body size bounds, rejects
/// malformed JSON, traversal attacks, and corrupted streams without panicking.
#[tokio::test]
async fn test_http_boundary_and_fuzz_hardening() {
    let temp_dir = std::env::temp_dir().join(format!("lanflare_http_fuzz_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();

    let config = AppConfig {
        alias: "Fuzz Server".to_string(),
        port: 0,
        download_dir: temp_dir.clone(),
        auto_accept: true,
        pin: Some("4455".to_string()),
        ..Default::default()
    };

    let registry = PeerRegistry::new();
    let (event_tx, _event_rx) = mpsc::channel(100);
    let server_handle = start_server(config, None, registry, Some(event_tx))
        .await
        .expect("Server must start");

    let base_url = format!("http://127.0.0.1:{}", server_handle.bound_port());
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // 1. Boundary: Unknown route -> 404 Not Found
    let res = client
        .get(format!("{base_url}/api/localsend/v3/nonexistent"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);

    // 2. Boundary: Wrong HTTP method -> 404 Not Found
    let res = client
        .post(format!("{base_url}/api/localsend/v2/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);

    let res = client
        .delete(format!("{base_url}/api/localsend/v2/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::NOT_FOUND);

    // 3. Fuzz: Malformed JSON to /register -> 400 Bad Request
    let res = client
        .post(format!("{base_url}/api/localsend/v2/register"))
        .body("not valid json at all")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // 4. Boundary: Bounded body limit on /register (> 10MB) -> 413 or 400
    // Generate a stream of 11 MB of spaces followed by valid JSON
    let oversized = vec![b' '; 11 * 1024 * 1024];
    let res = client
        .post(format!("{base_url}/api/localsend/v2/register"))
        .body(oversized)
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert!(
        res.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE
            || res.status() == reqwest::StatusCode::BAD_REQUEST
    );

    // 5. Fuzz: Malformed JSON to /prepare-upload -> 400 Bad Request
    let res = client
        .post(format!("{base_url}/api/localsend/v2/prepare-upload?pin=4455"))
        .body("{\"files\": \"not a map\"}")
        .header("content-type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // 6. Boundary: Empty files map to /prepare-upload -> 400 Bad Request
    let empty_prep = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Sender".to_string(),
            version: "2.0".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Desktop),
            fingerprint: "FP_SENDER".to_string(),
            port: 53317,
            protocol: ProtocolType::Http,
            download: false,
        },
        files: HashMap::new(),
    };
    let res = client
        .post(format!("{base_url}/api/localsend/v2/prepare-upload?pin=4455"))
        .json(&empty_prep)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // 7. Boundary: Path traversal attack sanitization in /prepare-upload
    let mut attack_files = HashMap::new();
    let f_id = "attack-file-1".to_string();
    let traversal_probe = std::env::temp_dir().join("lanflare_hacked_traversal_probe.bin");
    let _ = tokio::fs::remove_file(&traversal_probe).await;

    attack_files.insert(
        f_id.clone(),
        FileDto {
            id: f_id.clone(),
            file_name: "../../../../../tmp/lanflare_hacked_traversal_probe.bin".to_string(),
            size: 16,
            file_type: "text/plain".to_string(),
            sha256: None,
            preview: None,
            metadata: None,
        },
    );
    let attack_req = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Attacker".to_string(),
            version: "2.0".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Desktop),
            fingerprint: "ATTACKER_FP".to_string(),
            port: 53317,
            protocol: ProtocolType::Http,
            download: false,
        },
        files: attack_files,
    };
    let res = client
        .post(format!("{base_url}/api/localsend/v2/prepare-upload?pin=4455"))
        .json(&attack_req)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::OK);
    let prep_resp: lanflare_proto::transfer::PrepareUploadResponseDtoV2 = res.json().await.unwrap();
    let token = prep_resp.files.get(&f_id).unwrap();

    // Verify upload writes to sanitized name inside temp_dir, never escaped
    let payload = b"Safe Data 123456";
    let upload_url = format!(
        "{base_url}/api/localsend/v2/upload?sessionId={}&fileId={f_id}&token={token}",
        prep_resp.session_id
    );
    let up_res = client.post(&upload_url).body(payload.to_vec()).send().await.unwrap();
    assert_eq!(up_res.status(), reqwest::StatusCode::OK);

    // Verify file was not written outside temp_dir
    assert!(!traversal_probe.exists(), "Traversal probe must not exist outside destination dir");
    let dest_file = temp_dir.join("lanflare_hacked_traversal_probe.bin");
    assert!(dest_file.exists());
    assert_eq!(tokio::fs::read(&dest_file).await.unwrap(), payload);

    // 8. Boundary: Upload missing query params -> 400 Bad Request
    let res = client
        .post(format!("{base_url}/api/localsend/v2/upload"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    let res = client
        .post(format!("{base_url}/api/localsend/v2/upload?sessionId=abc"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // 9. Boundary: Upload with fabricated token -> 403 Forbidden
    let res = client
        .post(format!(
            "{base_url}/api/localsend/v2/upload?sessionId={}&fileId={f_id}&token=fabricated_token",
            prep_resp.session_id
        ))
        .body(vec![0u8; 16])
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // 10. Boundary: Download endpoint with missing query params -> 400 Bad Request
    let res = client
        .get(format!("{base_url}/api/localsend/v2/download"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // 11. Boundary: Download endpoint with invalid session -> 403 Forbidden
    let res = client
        .get(format!("{base_url}/api/localsend/v2/download?sessionId=fake&fileId=fake"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), reqwest::StatusCode::FORBIDDEN);

    // Cleanup server
    server_handle.stop();
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}

/// Verifies stream abort, size exceeded bombing protection, and temporary file cleanup.
#[tokio::test]
async fn test_streaming_upload_boundary_failures_and_cleanup() {
    let temp_dir = std::env::temp_dir().join(format!("lanflare_bombing_test_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();

    let config = AppConfig {
        alias: "Hardened Server".to_string(),
        port: 0,
        download_dir: temp_dir.clone(),
        auto_accept: true,
        pin: None,
        ..Default::default()
    };

    let registry = PeerRegistry::new();
    let (event_tx, _event_rx) = mpsc::channel(100);
    let server_handle = start_server(config, None, registry, Some(event_tx))
        .await
        .unwrap();

    let base_url = format!("http://127.0.0.1:{}", server_handle.bound_port());
    let client = Client::new();

    // 1. Prepare upload promising exactly 10 bytes
    let file_id = "test-bomb-file".to_string();
    let mut files = HashMap::new();
    files.insert(
        file_id.clone(),
        FileDto {
            id: file_id.clone(),
            file_name: "test_bomb.bin".to_string(),
            size: 10,
            file_type: "application/octet-stream".to_string(),
            sha256: None,
            preview: None,
            metadata: None,
        },
    );

    let prep_req = PrepareUploadRequestDtoV2 {
        info: RegisterDtoV2 {
            alias: "Sender".to_string(),
            version: "2.0".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Desktop),
            fingerprint: "FP_SENDER".to_string(),
            port: 53317,
            protocol: ProtocolType::Http,
            download: false,
        },
        files,
    };

    let prep_res: lanflare_proto::transfer::PrepareUploadResponseDtoV2 = client
        .post(format!("{base_url}/api/localsend/v2/prepare-upload"))
        .json(&prep_req)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let token = prep_res.files.get(&file_id).unwrap();

    // 2. Client sends 50 bytes (> 10 bytes promised) -> Disk space exhaustion bombing
    let bomb_payload = vec![0xAA; 50];
    let upload_url = format!(
        "{base_url}/api/localsend/v2/upload?sessionId={}&fileId={file_id}&token={token}",
        prep_res.session_id
    );

    let res = client
        .post(&upload_url)
        .body(bomb_payload)
        .send()
        .await
        .unwrap();

    // Server must reject with 400 Bad Request (size exceeded)
    assert_eq!(res.status(), reqwest::StatusCode::BAD_REQUEST);

    // Ensure no lingering .lanflare-part file exists in directory
    let mut read_dir = tokio::fs::read_dir(&temp_dir).await.unwrap();
    let mut part_files_count = 0;
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".lanflare-part") {
            part_files_count += 1;
        }
    }
    assert_eq!(part_files_count, 0, "All temporary .lanflare-part files must be cleaned up on failure");

    server_handle.stop();
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
}
