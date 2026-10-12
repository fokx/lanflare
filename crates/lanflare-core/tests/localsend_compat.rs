//! Cross-Protocol Interoperability Suite for LocalSend v2.2 specification.
//! Validates LanFlare implementation against LocalSend v2.2 wire protocol specifications,
//! bidirectional HTTP transfers, PIN negotiation, SHA-256 fingerprint verification,
//! and reverse file transfer (browser download API: prepare-download & download).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use uuid::Uuid;

use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{ClientError, LsClient, LsClientConfig};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;
use lanflare_core::discovery::registry::PeerRegistry;
use lanflare_core::server::{
    start_server, DownloadFile, ServerTransferEvent,
};
use lanflare_proto::discovery::{
    DeviceType, InfoResponseDtoV2, ProtocolType, RegisterDtoV2, RegisterResponseDtoV2,
};
use lanflare_proto::transfer::{
    FileDto, PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2,
};

/// Computes uppercase 64-character SHA-256 hex string.
fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{:02X}", b));
    }
    out
}

// ---------------------------------------------------------------------------
// Mock LocalSend v2 Peer (Simulating Official LocalSend v2 App)
// ---------------------------------------------------------------------------

struct MockLocalSendServer {
    port: u16,
    received_files: Arc<tokio::sync::Mutex<HashMap<String, Vec<u8>>>>,
    prepare_upload_calls: Arc<AtomicUsize>,
    _cancel_tx: tokio::sync::oneshot::Sender<()>,
}

impl MockLocalSendServer {
    async fn start(pin: Option<String>, auto_accept: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (_cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let received_files = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let prepare_upload_calls = Arc::new(AtomicUsize::new(0));

        let files_store = received_files.clone();
        let call_counter = prepare_upload_calls.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => break,
                    Ok((stream, _)) = listener.accept() => {
                        let io = TokioIo::new(stream);
                        let files_store = files_store.clone();
                        let call_counter = call_counter.clone();
                        let pin = pin.clone();

                        tokio::spawn(async move {
                            let service = service_fn(move |req: Request<Incoming>| {
                                let files_store = files_store.clone();
                                let call_counter = call_counter.clone();
                                let pin = pin.clone();

                                async move {
                                    let path = req.uri().path().to_string();
                                    let method = req.method().clone();
                                    let query = req.uri().query().map(str::to_string);

                                    match (method, path.as_str()) {
                                        (Method::GET, "/api/localsend/v2/info") | (Method::GET, "/api/localsend/v1/info") => {
                                            let info = InfoResponseDtoV2 {
                                                alias: "Official LocalSend Node".to_string(),
                                                version: "2.2".to_string(),
                                                device_model: Some("Android".to_string()),
                                                device_type: Some(DeviceType::Mobile),
                                                fingerprint: "OFFICIAL_LOCALSEND_FP".to_string(),
                                                port: Some(port),
                                                protocol: Some(ProtocolType::Http),
                                                download: false,
                                            };
                                            let json = serde_json::to_vec(&info).unwrap();
                                            Ok::<_, hyper::Error>(Response::builder()
                                                .status(StatusCode::OK)
                                                .header("content-type", "application/json")
                                                .body(Full::new(Bytes::from(json)))
                                                .unwrap())
                                        }

                                        (Method::POST, "/api/localsend/v2/register") => {
                                            let resp = RegisterResponseDtoV2 {
                                                alias: "Official LocalSend Node".to_string(),
                                                version: "2.2".to_string(),
                                                device_model: Some("Android".to_string()),
                                                device_type: Some(DeviceType::Mobile),
                                                fingerprint: "OFFICIAL_LOCALSEND_FP".to_string(),
                                                download: false,
                                            };
                                            let json = serde_json::to_vec(&resp).unwrap();
                                            Ok(Response::builder()
                                                .status(StatusCode::OK)
                                                .header("content-type", "application/json")
                                                .body(Full::new(Bytes::from(json)))
                                                .unwrap())
                                        }

                                        (Method::POST, "/api/localsend/v2/prepare-upload") => {
                                            call_counter.fetch_add(1, Ordering::SeqCst);

                                            if let Some(ref required_pin) = pin {
                                                let has_pin = query.as_deref().and_then(|q| {
                                                    for pair in q.split('&') {
                                                        let mut s = pair.splitn(2, '=');
                                                        if s.next() == Some("pin") {
                                                            return s.next();
                                                        }
                                                    }
                                                    None
                                                });

                                                if has_pin != Some(required_pin.as_str()) {
                                                    return Ok(Response::builder()
                                                        .status(StatusCode::UNAUTHORIZED)
                                                        .body(Full::new(Bytes::new()))
                                                        .unwrap());
                                                }
                                            }

                                            if !auto_accept {
                                                return Ok(Response::builder()
                                                    .status(StatusCode::FORBIDDEN)
                                                    .body(Full::new(Bytes::new()))
                                                    .unwrap());
                                            }

                                            use http_body_util::BodyExt;
                                            let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
                                            let prep_req: PrepareUploadRequestDtoV2 = serde_json::from_slice(&body_bytes).unwrap();

                                            let session_id = "mock-session-42".to_string();
                                            let mut files_tokens = HashMap::new();
                                            for file_id in prep_req.files.keys() {
                                                files_tokens.insert(file_id.clone(), format!("token_{file_id}"));
                                            }

                                            let prep_resp = PrepareUploadResponseDtoV2 {
                                                session_id,
                                                files: files_tokens,
                                            };
                                            let json = serde_json::to_vec(&prep_resp).unwrap();
                                            Ok(Response::builder()
                                                .status(StatusCode::OK)
                                                .header("content-type", "application/json")
                                                .body(Full::new(Bytes::from(json)))
                                                .unwrap())
                                        }

                                        (Method::POST, "/api/localsend/v2/upload") => {
                                            let query_str = query.unwrap_or_default();
                                            let mut file_id = None;
                                            for pair in query_str.split('&') {
                                                let mut s = pair.splitn(2, '=');
                                                if s.next() == Some("fileId") {
                                                    file_id = s.next().map(str::to_string);
                                                }
                                            }

                                            use http_body_util::BodyExt;
                                            let body_bytes = req.into_body().collect().await.unwrap().to_bytes();
                                            if let Some(fid) = file_id {
                                                files_store.lock().await.insert(fid, body_bytes.to_vec());
                                            }

                                            Ok(Response::builder()
                                                .status(StatusCode::OK)
                                                .body(Full::new(Bytes::new()))
                                                .unwrap())
                                        }

                                        (Method::POST, "/api/localsend/v2/cancel") => {
                                            Ok(Response::builder()
                                                .status(StatusCode::OK)
                                                .body(Full::new(Bytes::new()))
                                                .unwrap())
                                        }

                                        _ => Ok(Response::builder()
                                            .status(StatusCode::NOT_FOUND)
                                            .body(Full::new(Bytes::new()))
                                            .unwrap()),
                                    }
                                }
                            });

                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(io, service)
                                .await;
                        });
                    }
                }
            }
        });

        Self {
            port,
            received_files,
            prepare_upload_calls,
            _cancel_tx,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_lanflare_sender_to_official_localsend_receiver_interop() {
    let mock_receiver = MockLocalSendServer::start(None, true).await;
    let base_url = format!("http://127.0.0.1:{}", mock_receiver.port);

    // Prepare files to send
    let temp_tx = std::env::temp_dir().join(format!("lanflare_compat_tx_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_tx).await.unwrap();

    let f1_path = temp_tx.join("document.pdf");
    let f2_path = temp_tx.join("archive.tar.gz");

    let f1_data = b"%PDF-1.4 Mock PDF file transferred across protocol boundary".to_vec();
    let f2_data = vec![0x37; 64 * 1024]; // 64 KB

    tokio::fs::write(&f1_path, &f1_data).await.unwrap();
    tokio::fs::write(&f2_path, &f2_data).await.unwrap();

    let f1 = OutgoingFile::from_path(&f1_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();
    let f2 = OutgoingFile::from_path(&f2_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    let f1_id = f1.id.clone();
    let f2_id = f2.id.clone();

    let client = LsClient::new(LsClientConfig::default()).unwrap();

    // 1. Verify remote info conforms to LocalSend v2
    let info = client.get_info(&base_url).await.expect("info should succeed");
    assert_eq!(info.alias, "Official LocalSend Node");
    assert_eq!(info.version, "2.2");
    assert_eq!(info.device_type, Some(DeviceType::Mobile));

    // 2. Perform orchestrator transfer
    let sender_info = RegisterDtoV2 {
        alias: "LanFlare Node".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "LANFLARE_FP".to_string(),
        port: 53317,
        protocol: ProtocolType::Http,
        download: false,
    };
    let orchestrator = TransferOrchestrator::new(client, sender_info);

    let (progress_tx, mut progress_rx) = mpsc::channel(100);
    let options = TransferOptions {
        max_concurrency: 2,
        pin: None,
        cancel_token: None,
        progress_interval: Duration::from_millis(10),
        ..Default::default()
    };

    orchestrator
        .transfer(&base_url, vec![f1, f2], options, Some(progress_tx))
        .await
        .expect("Transfer session negotiation should succeed");

    // Pump progress
    let mut saw_completed = false;
    while let Ok(prog) = progress_rx.try_recv() {
        if let TransferProgress::SessionCompleted { total_files, .. } = prog {
            assert_eq!(total_files, 2);
            saw_completed = true;
            break;
        }
    }
    assert!(saw_completed);

    // Verify files received by mock LocalSend app match exact payload
    assert_eq!(mock_receiver.prepare_upload_calls.load(Ordering::SeqCst), 1);
    let received = mock_receiver.received_files.lock().await;
    assert_eq!(received.get(&f1_id).unwrap(), &f1_data);
    assert_eq!(received.get(&f2_id).unwrap(), &f2_data);
}

#[tokio::test]
async fn test_lanflare_sender_to_official_localsend_pin_negotiation() {
    let required_pin = "852963".to_string();
    let mock_receiver = MockLocalSendServer::start(Some(required_pin.clone()), true).await;
    let base_url = format!("http://127.0.0.1:{}", mock_receiver.port);

    let temp_tx = std::env::temp_dir().join(format!("lanflare_pin_tx_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_tx).await.unwrap();
    let f_path = temp_tx.join("pin_protected.txt");
    tokio::fs::write(&f_path, b"Secret Data").await.unwrap();

    let client = LsClient::new(LsClientConfig::default()).unwrap();
    let sender_info = RegisterDtoV2 {
        alias: "LanFlare Node".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "LANFLARE_FP".to_string(),
        port: 53317,
        protocol: ProtocolType::Http,
        download: false,
    };

    let f = OutgoingFile::from_path(&f_path).await.unwrap();

    // 1. Attempt transfer with wrong PIN -> fails with UnauthorizedPin
    let orchestrator = TransferOrchestrator::new(client.clone(), sender_info.clone());
    let (tx1, _rx1) = mpsc::channel(10);
    let wrong_opts = TransferOptions {
        max_concurrency: 1,
        pin: Some("wrong-pin".to_string()),
        cancel_token: None,
        progress_interval: Duration::from_millis(10),
        ..Default::default()
    };
    let err = orchestrator
        .transfer(
            &base_url,
            vec![f.clone()],
            wrong_opts,
            Some(tx1),
        )
        .await
        .err()
        .unwrap();

    assert!(matches!(err, ClientError::UnauthorizedPin));

    // 2. Attempt transfer with correct PIN -> succeeds
    let (tx2, mut rx2) = mpsc::channel(10);
    let correct_opts = TransferOptions {
        max_concurrency: 1,
        pin: Some(required_pin),
        cancel_token: None,
        progress_interval: Duration::from_millis(10),
        ..Default::default()
    };
    orchestrator
        .transfer(
            &base_url,
            vec![f],
            correct_opts,
            Some(tx2),
        )
        .await
        .expect("Transfer with correct PIN should succeed");

    let mut saw_completed = false;
    while let Ok(prog) = rx2.try_recv() {
        if let TransferProgress::SessionCompleted { total_files, .. } = prog {
            assert_eq!(total_files, 1);
            saw_completed = true;
            break;
        }
    }
    assert!(saw_completed);
}

#[tokio::test]
async fn test_lanflare_reverse_file_transfer_browser_download_api() {
    let temp_dir = std::env::temp_dir().join(format!("lanflare_download_api_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();

    let server_config = AppConfig {
        alias: "LanFlare Host".to_string(),
        port: 0,
        download_dir: temp_dir.clone(),
        auto_accept: true,
        pin: Some("556677".to_string()),
    };
    let registry = PeerRegistry::new();
    let (server_ev_tx, _server_ev_rx) = mpsc::channel(10);

    let server_handle = start_server(server_config, None, registry, Some(server_ev_tx))
        .await
        .expect("Server should bind");
    let port = server_handle.bound_port();
    let base_url = format!("http://127.0.0.1:{port}");

    // Create files offered for download
    let file1_path = temp_dir.join("photo.jpg");
    let file2_path = temp_dir.join("dataset.csv");

    let file1_bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46]; // JPEG magic header
    let file2_bytes = b"id,name,value\n1,alpha,42\n2,beta,100\n".to_vec();

    tokio::fs::write(&file1_path, &file1_bytes).await.unwrap();
    tokio::fs::write(&file2_path, &file2_bytes).await.unwrap();

    let f1_hash = sha256_hex(&file1_bytes);
    let f2_hash = sha256_hex(&file2_bytes);

    let offered_files = vec![
        DownloadFile {
            dto: FileDto {
                id: "file-photo-1".to_string(),
                file_name: "photo.jpg".to_string(),
                size: file1_bytes.len() as u64,
                file_type: "image/jpeg".to_string(),
                sha256: Some(f1_hash.clone()),
                preview: None,
                metadata: None,
            },
            file_path: file1_path,
        },
        DownloadFile {
            dto: FileDto {
                id: "file-dataset-2".to_string(),
                file_name: "dataset.csv".to_string(),
                size: file2_bytes.len() as u64,
                file_type: "text/csv".to_string(),
                sha256: Some(f2_hash.clone()),
                preview: None,
                metadata: None,
            },
            file_path: file2_path,
        },
    ];

    server_handle.download_manager().set_files(offered_files).await;

    let client = LsClient::new(LsClientConfig::default()).unwrap();

    // 1. GET /api/localsend/v2/info advertises download: true
    let info = client.get_info(&base_url).await.unwrap();
    assert_eq!(info.alias, "LanFlare Host");
    assert!(info.download, "Info should advertise download: true when files offered");

    // 2. Prepare download with wrong PIN -> UnauthorizedPin
    let wrong_pin_res = client
        .prepare_download(&base_url, None, Some("wrong-pin"))
        .await;
    assert!(matches!(wrong_pin_res, Err(ClientError::UnauthorizedPin)));

    // 3. Prepare download with correct PIN -> Returns session ID and file metadata map
    let prep_download = client
        .prepare_download(&base_url, None, Some("556677"))
        .await
        .expect("prepare_download should succeed with correct PIN");

    assert_eq!(prep_download.info.alias, "LanFlare Host");
    assert!(prep_download.info.download);
    let session_id = prep_download.session_id;
    assert!(!session_id.is_empty());
    assert_eq!(prep_download.files.len(), 2);
    assert!(prep_download.files.contains_key("file-photo-1"));
    assert!(prep_download.files.contains_key("file-dataset-2"));

    // 4. Download file 1 stream
    let resp1 = client
        .download_file(&base_url, &session_id, "file-photo-1")
        .await
        .expect("download photo.jpg should succeed");
    assert_eq!(resp1.status(), reqwest::StatusCode::OK);
    let downloaded_f1 = resp1.bytes().await.unwrap();
    assert_eq!(downloaded_f1.as_ref(), file1_bytes.as_slice());
    assert_eq!(sha256_hex(&downloaded_f1), f1_hash);

    // 5. Download file 2 stream
    let resp2 = client
        .download_file(&base_url, &session_id, "file-dataset-2")
        .await
        .expect("download dataset.csv should succeed");
    assert_eq!(resp2.status(), reqwest::StatusCode::OK);
    let downloaded_f2 = resp2.bytes().await.unwrap();
    assert_eq!(downloaded_f2.as_ref(), file2_bytes.as_slice());
    assert_eq!(sha256_hex(&downloaded_f2), f2_hash);

    // 6. Test session re-fetch / page reload with existing sessionId (PIN not required once session established)
    let refetch = client
        .prepare_download(&base_url, Some(&session_id), None)
        .await
        .expect("Re-fetch of accepted session without PIN should succeed");
    assert_eq!(refetch.session_id, session_id);
    assert_eq!(refetch.files.len(), 2);

    // 7. Test invalid file ID or invalid session ID
    let err_file = client
        .download_file(&base_url, &session_id, "nonexistent-file-id")
        .await;
    assert!(err_file.is_err());

    let err_session = client
        .download_file(&base_url, "fake-session-999", "file-photo-1")
        .await;
    assert!(err_session.is_err());

    // 8. Clear files and verify download is disabled
    server_handle.download_manager().clear_files().await;
    let info_after = client.get_info(&base_url).await.unwrap();
    assert!(!info_after.download);
}

#[tokio::test]
async fn test_https_mtls_fingerprint_validation_parity() {
    let server_ident = generate_self_signed().expect("Server identity should generate");
    let client_ident = generate_self_signed().expect("Client identity should generate");
    let server_fp = server_ident.fingerprint.clone();
    let client_fp = client_ident.fingerprint.clone();

    let temp_dir = std::env::temp_dir().join(format!("lanflare_mtls_parity_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_dir).await.unwrap();

    let server_config = AppConfig {
        alias: "Secure LanFlare Node".to_string(),
        port: 0,
        download_dir: temp_dir.clone(),
        auto_accept: true,
        pin: None,
    };
    let registry = PeerRegistry::new();

    let (server_ev_tx, mut server_ev_rx) = mpsc::channel(10);
    let server_handle = start_server(
        server_config,
        Some(server_ident),
        registry,
        Some(server_ev_tx),
    )
    .await
    .expect("HTTPS server should bind");
    let port = server_handle.bound_port();
    let base_url = format!("https://127.0.0.1:{port}");

    // Client with pinned server fingerprint & presenting its own client identity
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(10),
        connect_timeout: Duration::from_secs(3),
        tls_identity: Some(client_ident),
        target_fingerprint: Some(server_fp.clone()),
    };
    let client = LsClient::new(client_config).expect("Client should build");

    let reg_dto = RegisterDtoV2 {
        alias: "mTLS Client".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Rust".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: client_fp.clone(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
    };

    let reg_resp = client.register(&base_url, &reg_dto).await.unwrap();
    assert_eq!(reg_resp.alias, "Secure LanFlare Node");

    // Verify server event captured client cert fingerprint
    let ev = server_ev_rx.recv().await.unwrap();
    if let ServerTransferEvent::DeviceRegistered { info, .. } = ev {
        assert_eq!(info.fingerprint, client_fp);
    } else {
        panic!("Expected DeviceRegistered event");
    }

    // Rogue client with wrong target fingerprint must fail handshake immediately
    let rogue_config = LsClientConfig {
        timeout: Duration::from_secs(5),
        connect_timeout: Duration::from_secs(3),
        tls_identity: None,
        target_fingerprint: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string()),
    };
    let rogue_client = LsClient::new(rogue_config).unwrap();
    let res = rogue_client.get_info(&base_url).await;
    assert!(res.is_err(), "Rogue client with mismatched pinned fingerprint must be rejected by TLS handshake");
}
