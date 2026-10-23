//! Hyper v1 HTTP & HTTPS server implementing LocalSend v2.2 specification.

pub mod dufs;
pub mod handlers;
pub mod router;
pub mod session;

use std::net::SocketAddr;
use std::sync::Arc;
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub use dufs::{DufsConfig, DufsManager, DufsServerHandle, ServerResponseBody};
pub use handlers::{
    extract_password_from_url, percent_decode, ServerState, ServerTransferEvent,
};
pub use router::{route_request, ClientConnectionInfo};
pub use session::{
    DownloadFile, DownloadManager, DownloadSession, FileTransferStatus, PrepareUploadDecision,
    SessionEndReason, SessionFile, SessionManager,
};

use crate::config::AppConfig;
use crate::crypto::cert::TlsIdentity;
use crate::crypto::verifier::PinnedClientCertVerifier;
use crate::discovery::registry::PeerRegistry;

/// Handle representing a running HTTP/HTTPS server instance.
pub struct ServerHandle {
    /// The actual port the server bound to.
    bound_port: u16,
    /// Cancellation token used to signal graceful shutdown.
    cancel_token: CancellationToken,
    /// Join handle of the background server task.
    join_handle: tokio::task::JoinHandle<()>,
    /// Download manager for registering/unregistering reverse transfer files.
    download_manager: Arc<DownloadManager>,
    /// Dufs manager for serving shared folders to local peers.
    dufs_manager: Arc<DufsManager>,
    /// Chat engine for 1:1 chat and resumable media.
    chat_engine: Option<Arc<crate::chat::engine::ChatEngine>>,
    /// Resume manager for resumable streaming.
    resume_manager: Option<Arc<crate::storage::resume::ResumeManager>>,
    /// Device ring for personal self-trust and universal clipboard sync.
    device_ring: Option<Arc<crate::toolbox::clipboard::DeviceRing>>,
    /// Federated search engine for local catalog and mesh search.
    federated_search: Option<Arc<crate::toolbox::search::FederatedSearchEngine>>,
}

impl ServerHandle {
    /// Returns the port the server is listening on.
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }

    /// Returns a reference to the download manager.
    pub fn download_manager(&self) -> &Arc<DownloadManager> {
        &self.download_manager
    }

    /// Returns a reference to the Dufs folder manager.
    pub fn dufs_manager(&self) -> &Arc<DufsManager> {
        &self.dufs_manager
    }

    /// Returns a reference to the Chat engine if active.
    pub fn chat_engine(&self) -> Option<&Arc<crate::chat::engine::ChatEngine>> {
        self.chat_engine.as_ref()
    }

    /// Returns a reference to the Resume manager if active.
    pub fn resume_manager(&self) -> Option<&Arc<crate::storage::resume::ResumeManager>> {
        self.resume_manager.as_ref()
    }

    /// Returns a reference to the Device ring if active.
    pub fn device_ring(&self) -> Option<&Arc<crate::toolbox::clipboard::DeviceRing>> {
        self.device_ring.as_ref()
    }

    /// Returns a reference to the Federated search engine if active.
    pub fn federated_search(&self) -> Option<&Arc<crate::toolbox::search::FederatedSearchEngine>> {
        self.federated_search.as_ref()
    }

    /// Signals the server to stop accepting connections and terminate.
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }

    /// Waits for the server task to finish completely.
    pub async fn wait(self) {
        let _ = self.join_handle.await;
    }
}

/// Helper wrapper allowing start_server to accept either AppConfig or Arc<RwLock<AppConfig>>.
#[derive(Clone)]
pub struct ServerConfigHolder(pub Arc<tokio::sync::RwLock<AppConfig>>);

impl From<AppConfig> for ServerConfigHolder {
    fn from(c: AppConfig) -> Self {
        ServerConfigHolder(Arc::new(tokio::sync::RwLock::new(c)))
    }
}

impl From<Arc<tokio::sync::RwLock<AppConfig>>> for ServerConfigHolder {
    fn from(a: Arc<tokio::sync::RwLock<AppConfig>>) -> Self {
        ServerConfigHolder(a)
    }
}

/// Starts an HTTP/HTTPS server listening on the configured address.
///
/// If `tls_identity` is `Some`, the server terminates TLS using the identity
/// and validates client certificates if present (mTLS).
/// If `tls_identity` is `None`, standard plaintext HTTP/1.1 is served.
pub async fn start_server(
    config: impl Into<ServerConfigHolder>,
    tls_identity: Option<TlsIdentity>,
    registry: Arc<PeerRegistry>,
    event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
) -> anyhow::Result<ServerHandle> {
    start_server_ext(config, tls_identity, registry, event_tx, None, None).await
}

/// Extended server launcher permitting injected download and dufs managers.
pub async fn start_server_ext(
    config: impl Into<ServerConfigHolder>,
    tls_identity: Option<TlsIdentity>,
    registry: Arc<PeerRegistry>,
    event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
    download_manager: Option<Arc<DownloadManager>>,
    dufs_manager: Option<Arc<DufsManager>>,
) -> anyhow::Result<ServerHandle> {
    start_server_full(
        config,
        tls_identity,
        registry,
        event_tx,
        download_manager,
        dufs_manager,
        None,
    )
    .await
}

/// Full server launcher permitting injected download, dufs, and mesh routers.
pub async fn start_server_full(
    config: impl Into<ServerConfigHolder>,
    tls_identity: Option<TlsIdentity>,
    registry: Arc<PeerRegistry>,
    event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
    download_manager: Option<Arc<DownloadManager>>,
    dufs_manager: Option<Arc<DufsManager>>,
    mesh_router: Option<Arc<crate::mesh::router::MeshRouter>>,
) -> anyhow::Result<ServerHandle> {
    start_server_all(
        config,
        tls_identity,
        registry,
        event_tx,
        download_manager,
        dufs_manager,
        mesh_router,
        None,
    )
    .await
}

/// Comprehensive server launcher with chat engine and resumable streaming support.
#[allow(clippy::too_many_arguments)]
pub async fn start_server_all(
    config: impl Into<ServerConfigHolder>,
    tls_identity: Option<TlsIdentity>,
    registry: Arc<PeerRegistry>,
    event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
    download_manager: Option<Arc<DownloadManager>>,
    dufs_manager: Option<Arc<DufsManager>>,
    mesh_router: Option<Arc<crate::mesh::router::MeshRouter>>,
    chat_engine: Option<Arc<crate::chat::engine::ChatEngine>>,
) -> anyhow::Result<ServerHandle> {
    start_server_toolbox(
        config,
        tls_identity,
        registry,
        event_tx,
        download_manager,
        dufs_manager,
        mesh_router,
        chat_engine,
        None,
        None,
    )
    .await
}

/// Comprehensive server launcher with chat engine, mesh router, and toolbox support.
#[allow(clippy::too_many_arguments)]
pub async fn start_server_toolbox(
    config: impl Into<ServerConfigHolder>,
    tls_identity: Option<TlsIdentity>,
    registry: Arc<PeerRegistry>,
    event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
    download_manager: Option<Arc<DownloadManager>>,
    dufs_manager: Option<Arc<DufsManager>>,
    mesh_router: Option<Arc<crate::mesh::router::MeshRouter>>,
    chat_engine: Option<Arc<crate::chat::engine::ChatEngine>>,
    device_ring: Option<Arc<crate::toolbox::clipboard::DeviceRing>>,
    federated_search: Option<Arc<crate::toolbox::search::FederatedSearchEngine>>,
) -> anyhow::Result<ServerHandle> {
    let cancel_token = CancellationToken::new();
    let config_holder = config.into();
    let port = config_holder.0.read().await.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    let bound_port = listener.local_addr()?.port();

    let cert_fingerprint = match tls_identity.as_ref() {
        Some(ident) => ident.fingerprint.clone(),
        None => String::new(),
    };

    let tls_acceptor = match tls_identity.as_ref() {
        Some(ident) => {
            let cert_der = CertificateDer::from_pem_slice(ident.cert_pem.as_bytes())?;
            let key_der = PrivateKeyDer::from_pem_slice(ident.key_pem.as_bytes())?;

            // LocalSend spec: Client cert is optional in HTTP v2 server, but verified if provided
            let client_verifier = PinnedClientCertVerifier::new(&ident.cert_pem, false, None)?;

            let server_config = rustls::ServerConfig::builder()
                .with_client_cert_verifier(Arc::new(client_verifier))
                .with_single_cert(vec![cert_der], key_der)?;

            Some(tokio_rustls::TlsAcceptor::from(Arc::new(server_config)))
        }
        None => None,
    };

    let download_manager = download_manager.unwrap_or_else(|| Arc::new(DownloadManager::new()));
    let dufs_manager = dufs_manager.unwrap_or_else(|| Arc::new(DufsManager::new()));
    let resume_manager = chat_engine.as_ref().map(|c| c.resume_manager().clone());

    let state = ServerState {
        config: config_holder.0,
        cert_fingerprint,
        session_manager: Arc::new(SessionManager::new()),
        download_manager: download_manager.clone(),
        registry,
        event_tx,
        dufs_manager: dufs_manager.clone(),
        tls_identity: tls_identity.clone(),
        mesh_router,
        chat_engine: chat_engine.clone(),
        resume_manager: resume_manager.clone(),
        device_ring: device_ring.clone(),
        federated_search: federated_search.clone(),
    };

    let cancel = cancel_token.clone();
    let join_handle = tokio::spawn(async move {
        let tracker = TaskTracker::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("Server shutdown signaled, closing listener");
                    break;
                }
                accept_res = listener.accept() => {
                    let (tcp_stream, remote_addr) = match accept_res {
                        Ok(res) => res,
                        Err(err) => {
                            tracing::warn!("Failed to accept incoming TCP connection: {err}");
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            continue;
                        }
                    };

                    let _ = tcp_stream.set_nodelay(true);
                    let tls_acceptor = tls_acceptor.clone();
                    let state = state.clone();
                    let conn_cancel = cancel.clone();

                    tracker.spawn(async move {
                        tokio::select! {
                            _ = conn_cancel.cancelled() => {}
                            _ = serve_connection(tcp_stream, remote_addr, tls_acceptor, state) => {}
                        }
                    });
                }
            }
        }

        tracker.close();
        tracker.wait().await;
        tracing::info!("Server terminated all active connections");
    });

    Ok(ServerHandle {
        bound_port,
        cancel_token,
        join_handle,
        download_manager,
        dufs_manager,
        chat_engine,
        resume_manager,
        device_ring,
        federated_search,
    })
}

async fn serve_connection(
    tcp_stream: tokio::net::TcpStream,
    remote_addr: SocketAddr,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    state: ServerState,
) {
    if let Some(acceptor) = tls_acceptor {
        // Peek at the first byte to detect whether the incoming connection is a TLS handshake
        // (TLS record ContentType 0x16 = 22) or a plaintext HTTP request (starts with ASCII e.g. 'G', 'P', 'H', etc.)
        let mut peek_buf = [0u8; 1];
        match tcp_stream.peek(&mut peek_buf).await {
            Ok(0) => return, // Connection closed by peer
            Ok(_) => {
                if peek_buf[0] != 0x16 {
                    serve_plaintext_http_redirect(tcp_stream, remote_addr, state).await;
                    return;
                }
            }
            Err(err) => {
                tracing::debug!("Failed to peek incoming connection from {remote_addr}: {err}");
                return;
            }
        }

        match acceptor.accept(tcp_stream).await {
            Ok(tls_stream) => {
                let (_, conn) = tls_stream.get_ref();
                let client_cert_fingerprint = conn
                    .peer_certificates()
                    .and_then(|certs| certs.first())
                    .map(|c| crate::crypto::cert::fingerprint_from_cert_der(c.as_ref()));

                let client_info = ClientConnectionInfo {
                    remote_ip: remote_addr.ip(),
                    cert_fingerprint: client_cert_fingerprint,
                };

                serve_http_stream(tls_stream, client_info, state).await;
            }
            Err(err) => {
                tracing::debug!("TLS handshake failed with {remote_addr}: {err}");
            }
        }
    } else {
        let client_info = ClientConnectionInfo {
            remote_ip: remote_addr.ip(),
            cert_fingerprint: None,
        };
        serve_http_stream(tcp_stream, client_info, state).await;
    }
}

async fn serve_plaintext_http_redirect(
    stream: tokio::net::TcpStream,
    remote_addr: SocketAddr,
    state: ServerState,
) {
    let io = TokioIo::new(stream);
    let state_for_svc = state.clone();
    let service = hyper::service::service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
        let state = state_for_svc.clone();
        async move {
            let host_header = req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");

            let host_part = if !host_header.is_empty() {
                host_header.to_string()
            } else {
                let port = state.config.read().await.port;
                format!("localhost:{}", port)
            };

            let uri = req.uri();
            let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");
            let target_url = format!("https://{}{}", host_part, path_and_query);

            let status = match *req.method() {
                hyper::Method::GET | hyper::Method::HEAD => hyper::StatusCode::MOVED_PERMANENTLY,
                _ => hyper::StatusCode::TEMPORARY_REDIRECT,
            };

            let res = hyper::Response::builder()
                .status(status)
                .header(hyper::header::LOCATION, target_url)
                .header(hyper::header::CONNECTION, "close")
                .header(hyper::header::CONTENT_LENGTH, "0")
                .body(dufs::full_box_body(bytes::Bytes::new()));

            Ok::<_, std::convert::Infallible>(res.unwrap())
        }
    });

    if let Err(err) = hyper::server::conn::http1::Builder::new()
        .max_buf_size(16 * 1024)
        .serve_connection(io, service)
        .await
    {
        tracing::debug!("Plaintext HTTP redirect failed with {remote_addr}: {err}");
    }
}

async fn serve_http_stream<IO>(stream: IO, client_info: ClientConnectionInfo, state: ServerState)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let io = TokioIo::new(stream);
    let service = hyper::service::service_fn(move |req| {
        let client_info = client_info.clone();
        let state = state.clone();
        async move { route_request(req, client_info, &state).await }
    });

    if let Err(err) = hyper::server::conn::http1::Builder::new()
        .max_buf_size(128 * 1024)
        .serve_connection(io, service)
        .await
    {
        tracing::debug!("HTTP/1.1 connection error: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use sha2::{Digest, Sha256};
    use lanflare_proto::discovery::{InfoResponseDtoV2, ProtocolType, RegisterDtoV2, RegisterResponseDtoV2};
    use lanflare_proto::transfer::{FileDto, PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2};

    #[tokio::test]
    async fn test_server_plain_http_lifecycle_and_endpoints() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = AppConfig {
            alias: "Test Node 1".to_string(),
            port: 0, // OS assigns random port
            download_dir: temp_dir.clone(),
            auto_accept: true,
            pin: None,
            ..Default::default()
        };

        let registry = PeerRegistry::new();
        let (event_tx, mut event_rx) = mpsc::channel(16);

        let server_handle = start_server(config, None, registry, Some(event_tx))
            .await
            .expect("Server should bind");

        let port = server_handle.bound_port();
        assert!(port > 0);

        let client = reqwest::Client::new();
        let base_url = format!("http://127.0.0.1:{port}");

        // 1. GET /api/localsend/v2/info
        let info_resp = client
            .get(format!("{base_url}/api/localsend/v2/info"))
            .send()
            .await
            .expect("Info request should succeed");
        assert_eq!(info_resp.status(), reqwest::StatusCode::OK);
        let info_dto: InfoResponseDtoV2 = info_resp.json().await.unwrap();
        assert_eq!(info_dto.alias, "Test Node 1");

        // 2. POST /api/localsend/v2/register
        let reg_payload = RegisterDtoV2 {
            alias: "Remote Peer".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Linux".to_string()),
            device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
            fingerprint: "PEER_FINGERPRINT_123".to_string(),
            port: 53317,
            protocol: ProtocolType::Http,
            download: false,
        };

        let reg_resp = client
            .post(format!("{base_url}/api/localsend/v2/register"))
            .json(&reg_payload)
            .send()
            .await
            .expect("Register request should succeed");
        assert_eq!(reg_resp.status(), reqwest::StatusCode::OK);
        let reg_res_dto: RegisterResponseDtoV2 = reg_resp.json().await.unwrap();
        assert_eq!(reg_res_dto.alias, "Test Node 1");

        // Verify DeviceRegistered event
        match event_rx.recv().await {
            Some(ServerTransferEvent::DeviceRegistered { ip, info }) => {
                assert_eq!(ip.to_string(), "127.0.0.1");
                assert_eq!(info.alias, "Remote Peer");
            }
            other => panic!("Expected DeviceRegistered event, got: {other:?}"),
        }

        // 3. POST /api/localsend/v2/prepare-upload
        let file_content = b"Hello, LanFlare LocalSend Transfer Engine!";
        let mut hasher = Sha256::new();
        hasher.update(file_content);
        let expected_sha256 = format!("{:x}", hasher.finalize());

        let mut files = HashMap::new();
        files.insert(
            "file_1".to_string(),
            FileDto {
                id: "file_1".to_string(),
                file_name: "greeting.txt".to_string(),
                size: file_content.len() as u64,
                file_type: "text/plain".to_string(),
                sha256: Some(expected_sha256.clone()),
                preview: None,
                metadata: None,
            },
        );

        let prep_req = PrepareUploadRequestDtoV2 {
            info: reg_payload.clone(),
            files,
        };

        let prep_resp = client
            .post(format!("{base_url}/api/localsend/v2/prepare-upload"))
            .json(&prep_req)
            .send()
            .await
            .expect("Prepare upload should succeed");
        assert_eq!(prep_resp.status(), reqwest::StatusCode::OK);

        let prep_res_dto: PrepareUploadResponseDtoV2 = prep_resp.json().await.unwrap();
        assert!(!prep_res_dto.session_id.is_empty());
        let file_token = prep_res_dto.files.get("file_1").expect("Token for file_1");

        // 4. POST /api/localsend/v2/upload (Streaming upload)
        let upload_url = format!(
            "{base_url}/api/localsend/v2/upload?sessionId={}&fileId=file_1&token={}",
            prep_res_dto.session_id, file_token
        );

        let upload_resp = client
            .post(upload_url)
            .body(file_content.to_vec())
            .send()
            .await
            .expect("Upload request should succeed");
        assert_eq!(upload_resp.status(), reqwest::StatusCode::OK);

        // Check disk file content
        let saved_file = temp_dir.join("greeting.txt");
        assert!(saved_file.exists(), "File must exist on disk");
        let saved_bytes = tokio::fs::read(&saved_file).await.unwrap();
        assert_eq!(saved_bytes, file_content);

        // Verify FileStarted, FileCompleted, and SessionEnded events
        let started = event_rx.recv().await.unwrap();
        assert!(matches!(started, ServerTransferEvent::FileStarted { .. }));

        let completed = event_rx.recv().await.unwrap();
        assert!(matches!(completed, ServerTransferEvent::FileCompleted { .. }));

        let ended = event_rx.recv().await.unwrap();
        assert!(matches!(ended, ServerTransferEvent::SessionEnded { .. }));

        // 5. Clean server shutdown
        server_handle.stop();
        server_handle.wait().await;

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_server_pin_protection_and_cancellation() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_pin_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = AppConfig {
            alias: "Protected Node".to_string(),
            port: 0,
            download_dir: temp_dir.clone(),
            auto_accept: true,
            pin: Some("54321".to_string()),
            ..Default::default()
        };

        let registry = PeerRegistry::new();
        let server_handle = start_server(config, None, registry, None)
            .await
            .expect("Server should bind");

        let port = server_handle.bound_port();
        let client = reqwest::Client::new();
        let base_url = format!("http://127.0.0.1:{port}");

        let mut files = HashMap::new();
        files.insert(
            "file_1".to_string(),
            FileDto {
                id: "file_1".to_string(),
                file_name: "test.txt".to_string(),
                size: 10,
                file_type: "text/plain".to_string(),
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
                device_type: None,
                fingerprint: "FP".to_string(),
                port: 53317,
                protocol: ProtocolType::Http,
                download: false,
            },
            files,
        };

        // Prepare upload without PIN -> 401
        let resp_no_pin = client
            .post(format!("{base_url}/api/localsend/v2/prepare-upload"))
            .json(&prep_req)
            .send()
            .await
            .unwrap();
        assert_eq!(resp_no_pin.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Prepare upload with wrong PIN -> 401
        let resp_wrong_pin = client
            .post(format!("{base_url}/api/localsend/v2/prepare-upload?pin=00000"))
            .json(&prep_req)
            .send()
            .await
            .unwrap();
        assert_eq!(resp_wrong_pin.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Prepare upload with correct PIN -> 200
        let resp_ok_pin = client
            .post(format!("{base_url}/api/localsend/v2/prepare-upload?pin=54321"))
            .json(&prep_req)
            .send()
            .await
            .unwrap();
        assert_eq!(resp_ok_pin.status(), reqwest::StatusCode::OK);

        let prep_dto: PrepareUploadResponseDtoV2 = resp_ok_pin.json().await.unwrap();

        // Cancel session via POST /api/localsend/v2/cancel?sessionId=...
        let cancel_resp = client
            .post(format!(
                "{base_url}/api/localsend/v2/cancel?sessionId={}",
                prep_dto.session_id
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(cancel_resp.status(), reqwest::StatusCode::OK);

        // Subsequent upload on cancelled session should fail with 403
        let token = prep_dto.files.get("file_1").unwrap();
        let upload_resp = client
            .post(format!(
                "{base_url}/api/localsend/v2/upload?sessionId={}&fileId=file_1&token={}",
                prep_dto.session_id, token
            ))
            .body(vec![0u8; 10])
            .send()
            .await
            .unwrap();
        assert_eq!(upload_resp.status(), reqwest::StatusCode::FORBIDDEN);

        server_handle.stop();
        server_handle.wait().await;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_server_upload_checksum_mismatch_returns_422() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_chk_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let config = AppConfig {
            alias: "Checksum Node".to_string(),
            port: 0,
            download_dir: temp_dir.clone(),
            auto_accept: true,
            pin: None,
            ..Default::default()
        };

        let registry = PeerRegistry::new();
        let server_handle = start_server(config, None, registry, None)
            .await
            .expect("Server should bind");

        let port = server_handle.bound_port();
        let client = reqwest::Client::new();
        let base_url = format!("http://127.0.0.1:{port}");

        let mut files = HashMap::new();
        files.insert(
            "file_chk".to_string(),
            FileDto {
                id: "file_chk".to_string(),
                file_name: "corrupt.txt".to_string(),
                size: 5,
                file_type: "text/plain".to_string(),
                // Wrong expected sha256
                sha256: Some("0000000000000000000000000000000000000000000000000000000000000000".to_string()),
                preview: None,
                metadata: None,
            },
        );

        let prep_req = PrepareUploadRequestDtoV2 {
            info: RegisterDtoV2 {
                alias: "Sender".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: None,
                fingerprint: "FP".to_string(),
                port: 53317,
                protocol: ProtocolType::Http,
                download: false,
            },
            files,
        };

        let prep_resp = client
            .post(format!("{base_url}/api/localsend/v2/prepare-upload"))
            .json(&prep_req)
            .send()
            .await
            .unwrap();
        assert_eq!(prep_resp.status(), reqwest::StatusCode::OK);

        let prep_dto: PrepareUploadResponseDtoV2 = prep_resp.json().await.unwrap();
        let token = prep_dto.files.get("file_chk").unwrap();

        // Send 5 bytes
        let upload_resp = client
            .post(format!(
                "{base_url}/api/localsend/v2/upload?sessionId={}&fileId=file_chk&token={}",
                prep_dto.session_id, token
            ))
            .body(b"hello".to_vec())
            .send()
            .await
            .unwrap();

        // LocalSend spec Section 4.2: 422 Checksum mismatch
        assert_eq!(upload_resp.status(), reqwest::StatusCode::UNPROCESSABLE_ENTITY);

        server_handle.stop();
        server_handle.wait().await;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_server_https_mtls() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_tls_test_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();

        let server_ident = crate::crypto::cert::generate_self_signed().unwrap();
        let client_ident = crate::crypto::cert::generate_self_signed().unwrap();

        let config = AppConfig {
            alias: "Secure TLS Node".to_string(),
            port: 0,
            download_dir: temp_dir.clone(),
            auto_accept: true,
            pin: None,
            ..Default::default()
        };

        let registry = PeerRegistry::new();
        let server_handle = start_server(config, Some(server_ident.clone()), registry, None)
            .await
            .expect("HTTPS Server should bind");

        let port = server_handle.bound_port();

        // Build reqwest client with client cert and trusting server cert
        let client_cert_pkcs8 = reqwest::Identity::from_pem(
            format!("{}\n{}", client_ident.cert_pem, client_ident.key_pem).as_bytes(),
        )
        .unwrap();

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true) // Self-signed in test
            .identity(client_cert_pkcs8)
            .build()
            .unwrap();

        let base_url = format!("https://127.0.0.1:{port}");

        let info_resp = client
            .get(format!("{base_url}/api/localsend/v2/info"))
            .send()
            .await
            .expect("TLS request should succeed");
        assert_eq!(info_resp.status(), reqwest::StatusCode::OK);

        let info_dto: InfoResponseDtoV2 = info_resp.json().await.unwrap();
        assert_eq!(info_dto.alias, "Secure TLS Node");
        assert_eq!(info_dto.fingerprint, server_ident.fingerprint);

        server_handle.stop();
        server_handle.wait().await;
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
