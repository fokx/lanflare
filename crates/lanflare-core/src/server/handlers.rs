//! HTTP request handlers for LocalSend v2.2 endpoints.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use lanflare_proto::discovery::{InfoResponseDtoV2, ProtocolType, RegisterDtoV2, RegisterResponseDtoV2, PROTOCOL_VERSION_V2};
use lanflare_proto::transfer::{
    PrepareDownloadResponseDtoV2, PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::AppConfig;
use crate::discovery::registry::PeerRegistry;
use crate::server::dufs::DufsManager;
use crate::server::session::{
    DownloadManager, FileTransferStatus, PrepareUploadDecision, SessionEndReason, SessionFile,
    SessionManager,
};
use crate::storage::path::resolve_unique_destination_path;
use crate::storage::writer::{StorageCommitResult, StreamingFileWriter};

/// Application events emitted by the HTTP server for GUI/TUI integration.
#[derive(Debug)]
pub enum ServerTransferEvent {
    /// Peer registration received.
    DeviceRegistered {
        ip: IpAddr,
        info: RegisterDtoV2,
    },
    /// An incoming upload preparation request.
    PrepareUpload {
        session_id: String,
        sender_ip: IpAddr,
        sender_info: RegisterDtoV2,
        cert_fingerprint: Option<String>,
        files: HashMap<String, lanflare_proto::transfer::FileDto>,
        decision_tx: oneshot::Sender<PrepareUploadDecision>,
    },
    /// A file upload started.
    FileStarted {
        session_id: String,
        file_id: String,
        file_name: String,
        total_bytes: u64,
    },
    /// A file upload progress update.
    FileProgress {
        session_id: String,
        file_id: String,
        file_name: String,
        bytes_received: u64,
        total_bytes: u64,
    },
    /// A file upload completed and verified.
    FileCompleted {
        session_id: String,
        file_id: String,
        result: StorageCommitResult,
    },
    /// A file upload failed.
    FileFailed {
        session_id: String,
        file_id: String,
        error: String,
    },
    /// The transfer session ended.
    SessionEnded {
        session_id: String,
        reason: SessionEndReason,
    },
    /// A browser requested a file download in share via link mode.
    BrowserDownload {
        download_id: String,
        remote_ip: IpAddr,
        file_name: String,
        file_size: u64,
        decision_tx: oneshot::Sender<bool>,
    },
}

/// Shared application state injected into hyper request handlers.
#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<tokio::sync::RwLock<AppConfig>>,
    pub cert_fingerprint: String,
    pub session_manager: Arc<SessionManager>,
    pub download_manager: Arc<DownloadManager>,
    pub registry: Arc<PeerRegistry>,
    pub event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
    pub dufs_manager: Arc<DufsManager>,
    pub tls_identity: Option<crate::crypto::cert::TlsIdentity>,
}

/// Helper decoding percent-encoded strings (e.g. `%40` -> `@`, `%3A` -> `:`).
pub fn percent_decode(input: &str) -> String {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h1), Some(h2)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            result.push((h1 * 16 + h2) as u8);
            i += 3;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).unwrap_or_else(|_| input.to_string())
}

/// Helper extracting password/PIN from a raw input string or full URL.
///
/// Specifically handles special cases such as:
/// `https://:b7asiojoij@@:e7z3@10.40.1.133:53317`
/// where `b7asiojoij@@:e7z3` is the password.
pub fn extract_password_from_url(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let authority_and_beyond = if let Some(idx) = trimmed.find("://") {
        &trimmed[idx + 3..]
    } else if let Some(rest) = trimmed.strip_prefix("//") {
        rest
    } else if let Some(last_at) = trimmed.rfind('@') {
        let after_at = &trimmed[last_at + 1..];
        // If after the last '@' looks like a host (e.g. 192.168.1.1:53317, localhost:53317, not starting with ':')
        if !after_at.starts_with(':') && (after_at.contains('.') || after_at.contains(':')) {
            trimmed
        } else {
            return trimmed.to_string();
        }
    } else {
        return trimmed.to_string();
    };

    let (authority, query_part) = match authority_and_beyond.find(['/', '?', '#']) {
        Some(idx) => (&authority_and_beyond[..idx], &authority_and_beyond[idx..]),
        None => (authority_and_beyond, ""),
    };

    // The userinfo delimiter is before the LAST '@' in the authority
    if let Some(last_at) = authority.rfind('@') {
        let userinfo = &authority[..last_at];
        let pass = if let Some(stripped) = userinfo.strip_prefix(':') {
            stripped
        } else if let Some((_user, pass)) = userinfo.split_once(':') {
            pass
        } else {
            userinfo
        };
        return percent_decode(pass);
    }

    // If no userinfo in authority, check query string for ?pin=... or &pin=...
    if let Some(q_idx) = query_part.find('?') {
        let q_str = &query_part[q_idx + 1..];
        for pair in q_str.split('&') {
            if let Some(val) = pair.strip_prefix("pin=") {
                let clean_val = val.split('#').next().unwrap_or(val);
                return percent_decode(clean_val);
            }
        }
    }

    trimmed.to_string()
}

/// Helper verifying HTTP Basic Auth or pin query param against share_link_pin.
pub fn check_share_link_auth(req: &Request<Incoming>, config: &AppConfig) -> bool {
    if !config.share_link_require_pin {
        return true;
    }

    let expected_pin = config.share_link_pin.trim();
    if expected_pin.is_empty() {
        return true;
    }
    let expected_decoded = percent_decode(expected_pin);

    // 1. Check Authorization header: Basic <base64>
    if let Some(auth_val) = req.headers().get(hyper::header::AUTHORIZATION)
        && let Ok(auth_str) = auth_val.to_str()
        && let Some(token) = auth_str.strip_prefix("Basic ").or_else(|| auth_str.strip_prefix("basic "))
        && let Ok(decoded_bytes) = base64::engine::general_purpose::STANDARD.decode(token.trim())
        && let Ok(decoded_str) = std::str::from_utf8(&decoded_bytes)
    {
        // Decoded could be ":pass", "user:pass", "pass", or full URL
        let pass_candidates = if let Some(stripped) = decoded_str.strip_prefix(':') {
            vec![stripped, decoded_str]
        } else if let Some((user, pass)) = decoded_str.split_once(':') {
            vec![pass, user, decoded_str]
        } else {
            vec![decoded_str]
        };

        for cand in pass_candidates {
            let cand_trimmed = cand.trim();
            if cand_trimmed == expected_pin
                || cand_trimmed == expected_decoded
                || percent_decode(cand_trimmed) == expected_decoded
                || extract_password_from_url(cand_trimmed) == expected_pin
                || extract_password_from_url(cand_trimmed) == expected_decoded
            {
                return true;
            }
        }
    }

    // 2. Check query param: ?pin=...
    if let Some(q) = req.uri().query() {
        let query_map = parse_query(Some(q));
        if let Some(pin) = query_map.get("pin") {
            let pin_trimmed = pin.trim();
            if pin_trimmed == expected_pin
                || pin_trimmed == expected_decoded
                || percent_decode(pin_trimmed) == expected_decoded
                || extract_password_from_url(pin_trimmed) == expected_pin
                || extract_password_from_url(pin_trimmed) == expected_decoded
            {
                return true;
            }
        }
    }

    false
}

/// Helper creating HTTP 401 response with WWW-Authenticate header.
pub fn unauthorized_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            hyper::header::WWW_AUTHENTICATE,
            "Basic realm=\"Enter Password (Leave Username Blank / Not Required) - LanFlare Share\", charset=\"UTF-8\"",
        )
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from("401 Unauthorized - Enter password only (no username required)\n")))
        .unwrap()
}

/// Helper creating an empty hyper response with given status code.
pub fn empty_response(status: StatusCode) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .body(Full::new(Bytes::new()))
        .unwrap()
}

/// Helper creating a JSON hyper response with given status code.
pub fn json_response<T: serde::Serialize>(status: StatusCode, data: &T) -> Response<Full<Bytes>> {
    let body = match serde_json::to_vec(data) {
        Ok(bytes) => bytes,
        Err(_) => {
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

/// Maximum allowed size for JSON control bodies (10 MB).
pub const MAX_JSON_BODY_SIZE: usize = 10 * 1024 * 1024;

/// Helper to read and deserialize a bounded JSON request body, preventing memory exhaustion attacks.
pub async fn read_json_body<T: serde::de::DeserializeOwned>(
    body: Incoming,
) -> Result<T, StatusCode> {
    let limited = Limited::new(body, MAX_JSON_BODY_SIZE);
    let collected = match limited.collect().await {
        Ok(c) => c.to_bytes(),
        Err(err) => {
            if err.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
            return Err(StatusCode::BAD_REQUEST);
        }
    };
    serde_json::from_slice(&collected).map_err(|_| StatusCode::BAD_REQUEST)
}

/// Helper parsing query parameters into a HashMap.
pub fn parse_query(query_str: Option<&str>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(query) = query_str else {
        return map;
    };

    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("").to_string();
        let value = parts.next().unwrap_or("").to_string();
        if !key.is_empty() {
            map.insert(key, value);
        }
    }
    map
}

/// Handler for `GET /api/localsend/v2/info` and `GET /api/localsend/v1/info`.
pub async fn handle_info(remote_ip: IpAddr, state: &ServerState) -> Response<Full<Bytes>> {
    // Reverse probe: if an external peer probes us (e.g. Android manual scan),
    // immediately probe them back on port 53317 so we discover them symmetrically.
    if !remote_ip.is_loopback() {
        let registry = Arc::clone(&state.registry);
        let event_tx = state.event_tx.clone();
        let tls_identity = state.tls_identity.clone();
        let own_fp = state.cert_fingerprint.clone();
        tokio::spawn(async move {
            let client_config = crate::client::LsClientConfig {
                timeout: std::time::Duration::from_millis(2500),
                connect_timeout: std::time::Duration::from_millis(1200),
                tls_identity,
                target_fingerprint: None,
            };
            if let Ok(client) = crate::client::LsClient::new(client_config) {
                for proto in [
                    lanflare_proto::discovery::ProtocolType::Https,
                    lanflare_proto::discovery::ProtocolType::Http,
                ] {
                    let proto_str = proto.as_str();
                    let port = 53317;
                    let base_url = format!("{proto_str}://{remote_ip}:{port}");
                    if let Ok(info) = client.get_info(&base_url).await {
                        if !info.fingerprint.is_empty()
                            && info.fingerprint.eq_ignore_ascii_case(&own_fp)
                        {
                            return;
                        }
                        let peer = registry.handle_info(remote_ip, port, proto, &info).await;
                        if peer.is_some() {
                            if let Some(ref tx) = event_tx {
                                let _ = tx
                                    .send(ServerTransferEvent::DeviceRegistered {
                                        ip: remote_ip,
                                        info: lanflare_proto::discovery::RegisterDtoV2 {
                                            alias: info.alias,
                                            version: info.version,
                                            device_model: info.device_model,
                                            device_type: info.device_type,
                                            fingerprint: info.fingerprint,
                                            port: info.port.unwrap_or(port),
                                            protocol: info.protocol.unwrap_or(proto),
                                            download: info.download,
                                        },
                                    })
                                    .await;
                            }
                        }
                        break;
                    }
                }
            }
        });
    }

    let files = state.download_manager.get_file_dtos().await;
    let cfg = state.config.read().await;
    let resp = InfoResponseDtoV2 {
        alias: cfg.alias.clone(),
        version: PROTOCOL_VERSION_V2.to_string(),
        device_model: Some(std::env::consts::OS.to_string()),
        device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
        fingerprint: state.cert_fingerprint.clone(),
        port: Some(cfg.port),
        protocol: Some(ProtocolType::Https),
        download: !files.is_empty(),
    };
    json_response(StatusCode::OK, &resp)
}

/// Handler for `POST /api/localsend/v2/register`.
pub async fn handle_register(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    client_cert_fingerprint: Option<String>,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let dto: RegisterDtoV2 = match read_json_body(req.into_body()).await {
        Ok(d) => d,
        Err(status) => return empty_response(status),
    };

    // If client cert fingerprint is present (mTLS), ensure claimed fingerprint matches or log warning
    if let Some(ref cert_fp) = client_cert_fingerprint.filter(|fp| !dto.fingerprint.eq_ignore_ascii_case(fp)) {
        tracing::warn!(
            "Claimed fingerprint {} does not match client certificate fingerprint {}",
            dto.fingerprint,
            cert_fp
        );
    }

    // Ingest into PeerRegistry
    state.registry.handle_http_register(remote_ip, &dto).await;

    // Emit event if configured
    if let Some(ref tx) = state.event_tx {
        let _ = tx
            .send(ServerTransferEvent::DeviceRegistered {
                ip: remote_ip,
                info: dto,
            })
            .await;
    }

    let files = state.download_manager.get_file_dtos().await;
    let cfg = state.config.read().await;
    let resp = RegisterResponseDtoV2 {
        alias: cfg.alias.clone(),
        version: PROTOCOL_VERSION_V2.to_string(),
        device_model: Some(std::env::consts::OS.to_string()),
        device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
        fingerprint: state.cert_fingerprint.clone(),
        download: !files.is_empty(),
    };
    json_response(StatusCode::OK, &resp)
}

/// Handler for `POST /api/localsend/v2/prepare-upload`.
pub async fn handle_prepare_upload(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    client_cert_fingerprint: Option<String>,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query());
    let provided_pin = query.get("pin").map(|s| s.as_str());

    let (auto_accept, pin_requirement) = {
        let cfg = state.config.read().await;
        (cfg.auto_accept, cfg.pin.clone())
    };

    // Validate PIN
    if let Err(status) = state
        .session_manager
        .pin_tracker()
        .check_pin(pin_requirement.as_deref(), provided_pin, remote_ip)
        .await
    {
        return empty_response(status);
    }

    let payload: PrepareUploadRequestDtoV2 = match read_json_body(req.into_body()).await {
        Ok(p) => p,
        Err(status) => return empty_response(status),
    };

    if payload.files.is_empty() {
        return empty_response(StatusCode::BAD_REQUEST);
    }

    // Ingest the sender into PeerRegistry upon incoming transfer request
    state.registry.handle_http_register(remote_ip, &payload.info).await;

    let session_id = Uuid::new_v4().to_string();
    let cancel = CancellationToken::new();

    // Claim pending session slot
    if let Err(status) = state
        .session_manager
        .claim_pending(
            session_id.clone(),
            remote_ip,
            client_cert_fingerprint.clone(),
            cancel.clone(),
        )
        .await
    {
        return empty_response(status);
    }

    // Determine decision: if auto_accept is enabled in AppConfig, accept all files immediately
    let decision = if auto_accept {
        PrepareUploadDecision::Accept(payload.files.keys().cloned().collect())
    } else if let Some(ref tx) = state.event_tx {
        let (decision_tx, decision_rx) = oneshot::channel();
        let event = ServerTransferEvent::PrepareUpload {
            session_id: session_id.clone(),
            sender_ip: remote_ip,
            sender_info: payload.info.clone(),
            cert_fingerprint: client_cert_fingerprint.clone(),
            files: payload.files.clone(),
            decision_tx,
        };

        if tx.send(event).await.is_err() {
            state.session_manager.clear_pending(&session_id).await;
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }

        tokio::select! {
            res = decision_rx => {
                match res {
                    Ok(d) => d,
                    Err(_) => {
                        state.session_manager.clear_pending(&session_id).await;
                        return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
                    }
                }
            }
            _ = cancel.cancelled() => {
                state.session_manager.clear_pending(&session_id).await;
                return empty_response(StatusCode::FORBIDDEN);
            }
        }
    } else {
        // Without an event channel or auto_accept, accept all files by default for headless operation
        PrepareUploadDecision::Accept(payload.files.keys().cloned().collect())
    };

    let accepted_file_ids = match decision {
        PrepareUploadDecision::Decline => {
            state.session_manager.clear_pending(&session_id).await;
            return empty_response(StatusCode::FORBIDDEN);
        }
        PrepareUploadDecision::Accept(ids) => ids,
    };

    if accepted_file_ids.is_empty() {
        state.session_manager.clear_pending(&session_id).await;
        // LocalSend spec Section 4.1: If empty or no files accepted, return 204 No Content
        return empty_response(StatusCode::NO_CONTENT);
    }

    // Prepare session files with tokens and safe target paths
    let mut session_files = HashMap::new();
    let mut response_tokens = HashMap::new();

    for (file_id, file_dto) in payload.files {
        if !accepted_file_ids.contains(&file_id) {
            continue;
        }

        let token = Uuid::new_v4().to_string();
        let download_dir = state.config.read().await.download_dir.clone();
        let target_path = match resolve_unique_destination_path(&download_dir, &file_dto.file_name)
            .or_else(|_| {
                let fallback = format!("file_{}.bin", Uuid::new_v4().simple());
                resolve_unique_destination_path(&download_dir, &fallback)
            })
        {
            Ok(p) => p,
            Err(_) => {
                state.session_manager.clear_pending(&session_id).await;
                return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        };

        session_files.insert(
            file_id.clone(),
            SessionFile {
                dto: file_dto,
                token: token.clone(),
                status: FileTransferStatus::Pending,
                attempts: 0,
                target_path,
            },
        );
        response_tokens.insert(file_id, token);
    }

    if let Err(status) = state
        .session_manager
        .activate_session(
            &session_id,
            remote_ip,
            client_cert_fingerprint,
            session_files,
        )
        .await
    {
        state.session_manager.clear_pending(&session_id).await;
        return empty_response(status);
    }

    let resp = PrepareUploadResponseDtoV2 {
        session_id,
        files: response_tokens,
    };
    json_response(StatusCode::OK, &resp)
}

/// Handler for `POST /api/localsend/v2/upload`.
pub async fn handle_upload(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query());
    let (Some(session_id), Some(file_id), Some(token)) = (
        query.get("sessionId"),
        query.get("fileId"),
        query.get("token"),
    ) else {
        return empty_response(StatusCode::BAD_REQUEST);
    };

    // Validate authorization and mark file InProgress
    let session_file = match state
        .session_manager
        .validate_and_start_upload(session_id, file_id, token, remote_ip)
        .await
    {
        Ok(f) => f,
        Err(status) => return empty_response(status),
    };

    if let Some(ref tx) = state.event_tx {
        let _ = tx
            .send(ServerTransferEvent::FileStarted {
                session_id: session_id.clone(),
                file_id: file_id.clone(),
                file_name: session_file.dto.file_name.clone(),
                total_bytes: session_file.dto.size,
            })
            .await;
    }

    // Create streaming disk writer
    let expected_size = Some(session_file.dto.size);
    let expected_sha256 = session_file.dto.sha256.clone();

    let mut writer = match StreamingFileWriter::create(
        &session_file.target_path,
        expected_size,
        expected_sha256.clone(),
    )
    .await
    {
        Ok(w) => w,
        Err(err) => {
            tracing::error!("Failed to initialize streaming writer: {err}");
            state
                .session_manager
                .mark_file_status(session_id, file_id, FileTransferStatus::Failed)
                .await;
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Stream chunks directly from request body into disk writer
    let mut body = req.into_body();
    let mut stream_error = None;
    let mut last_progress_emit = std::time::Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    while let Some(chunk_res) = body.frame().await {
        match chunk_res {
            Ok(frame) => {
                let data = match frame.into_data() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                if let Err(err) = writer.write_chunk(&data).await {
                    stream_error = Some(err);
                    break;
                }

                let now = std::time::Instant::now();
                if now.duration_since(last_progress_emit) >= progress_interval {
                    last_progress_emit = now;
                    if let Some(ref tx) = state.event_tx {
                        let _ = tx.try_send(ServerTransferEvent::FileProgress {
                            session_id: session_id.clone(),
                            file_id: file_id.clone(),
                            file_name: session_file.dto.file_name.clone(),
                            bytes_received: writer.bytes_written(),
                            total_bytes: session_file.dto.size,
                        });
                    }
                }
            }
            Err(e) => {
                stream_error = Some(crate::storage::writer::StorageError::StreamError(e.to_string()));
                break;
            }
        }
    }

    if let Some(err) = stream_error {
        tracing::warn!("Upload stream failed for file {file_id}: {err}");
        let _ = writer.abort().await;
        state
            .session_manager
            .mark_file_status(session_id, file_id, FileTransferStatus::Failed)
            .await;
        if let Some(ref tx) = state.event_tx {
            let _ = tx
                .send(ServerTransferEvent::FileFailed {
                    session_id: session_id.clone(),
                    file_id: file_id.clone(),
                    error: err.to_string(),
                })
                .await;
        }

        return match err {
            crate::storage::writer::StorageError::ChecksumMismatch { .. } => {
                empty_response(StatusCode::UNPROCESSABLE_ENTITY)
            }
            crate::storage::writer::StorageError::SizeExceeded { .. } => {
                empty_response(StatusCode::BAD_REQUEST)
            }
            _ => empty_response(StatusCode::INTERNAL_SERVER_ERROR),
        };
    }

    // Commit file verification (validates length and expected SHA-256)
    match writer.commit().await {
        Ok(commit_result) => {
            let session_finished = state
                .session_manager
                .mark_file_status(session_id, file_id, FileTransferStatus::Completed)
                .await;

            if let Some(ref tx) = state.event_tx {
                let _ = tx
                    .send(ServerTransferEvent::FileCompleted {
                        session_id: session_id.clone(),
                        file_id: file_id.clone(),
                        result: commit_result,
                    })
                    .await;

                if session_finished {
                    let _ = tx
                        .send(ServerTransferEvent::SessionEnded {
                            session_id: session_id.clone(),
                            reason: SessionEndReason::Finished,
                        })
                        .await;
                }
            }

            empty_response(StatusCode::OK)
        }
        Err(err) => {
            tracing::warn!("Commit verification failed for file {file_id}: {err}");
            state
                .session_manager
                .mark_file_status(session_id, file_id, FileTransferStatus::Failed)
                .await;

            if let Some(ref tx) = state.event_tx {
                let _ = tx
                    .send(ServerTransferEvent::FileFailed {
                        session_id: session_id.clone(),
                        file_id: file_id.clone(),
                        error: err.to_string(),
                    })
                    .await;
            }

            match err {
                crate::storage::writer::StorageError::ChecksumMismatch { .. } => {
                    empty_response(StatusCode::UNPROCESSABLE_ENTITY)
                }
                crate::storage::writer::StorageError::SizeMismatch { .. } => {
                    empty_response(StatusCode::BAD_REQUEST)
                }
                _ => empty_response(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
    }
}

/// Handler for `POST /api/localsend/v2/cancel`.
pub async fn handle_cancel(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query());
    let session_id = query.get("sessionId").map(|s| s.as_str());

    let reason = state
        .session_manager
        .cancel_session(session_id, remote_ip)
        .await;

    if let (Some(reason), Some(tx)) = (reason, &state.event_tx) {
        let _ = tx
            .send(ServerTransferEvent::SessionEnded {
                session_id: session_id.unwrap_or("unknown").to_string(),
                reason,
            })
            .await;
    }

    empty_response(StatusCode::OK)
}

/// Handler for `POST /api/localsend/v2/prepare-download` (Reverse file transfer / browser download metadata).
pub async fn handle_prepare_download(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query());
    let provided_pin = query.get("pin").map(|s| s.as_str());

    let (alias, port, pin_req) = {
        let cfg = state.config.read().await;
        (cfg.alias.clone(), cfg.port, cfg.pin.clone())
    };

    // Check if client is already accepted for this session
    if let Some(session_id) = query.get("sessionId")
        && state
            .download_manager
            .is_session_valid(session_id, remote_ip)
            .await
    {
        let files = state.download_manager.get_file_dtos().await;
        let resp = PrepareDownloadResponseDtoV2 {
            info: InfoResponseDtoV2 {
                alias,
                version: PROTOCOL_VERSION_V2.to_string(),
                device_model: Some(std::env::consts::OS.to_string()),
                device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
                fingerprint: state.cert_fingerprint.clone(),
                port: Some(port),
                protocol: Some(ProtocolType::Https),
                download: true,
            },
            session_id: session_id.clone(),
            files,
        };
        return json_response(StatusCode::OK, &resp);
    }

    // Validate PIN
    if let Err(status) = state
        .session_manager
        .pin_tracker()
        .check_pin(pin_req.as_deref(), provided_pin, remote_ip)
        .await
    {
        return empty_response(status);
    }

    let files = state.download_manager.get_file_dtos().await;
    if files.is_empty() {
        return empty_response(StatusCode::FORBIDDEN);
    }

    let session_id = state
        .download_manager
        .prepare_session(query.get("sessionId").map(|s| s.as_str()), remote_ip, true)
        .await;

    let resp = PrepareDownloadResponseDtoV2 {
        info: InfoResponseDtoV2 {
            alias,
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some(std::env::consts::OS.to_string()),
            device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
            fingerprint: state.cert_fingerprint.clone(),
            port: Some(port),
            protocol: Some(ProtocolType::Https),
            download: true,
        },
        session_id,
        files,
    };
    json_response(StatusCode::OK, &resp)
}

/// Handler for `GET /api/localsend/v2/download?sessionId=...&fileId=...`.
pub async fn handle_download(
    req: Request<Incoming>,
    remote_ip: IpAddr,
    state: &ServerState,
) -> Response<Full<Bytes>> {
    let query = parse_query(req.uri().query());
    let file_id = match query.get("fileId") {
        Some(id) => id,
        None => return empty_response(StatusCode::BAD_REQUEST),
    };

    let session_id = match query.get("sessionId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "web".to_string(),
    };

    let (is_web, auto_accept) = {
        let is_web = session_id == "web";
        let cfg = state.config.read().await;
        if is_web && !check_share_link_auth(&req, &cfg) {
            return unauthorized_response();
        }
        (is_web, cfg.share_link_auto_accept)
    };

    // For browser direct downloads, ensure session is prepared
    if session_id == "web" {
        state
            .download_manager
            .prepare_session(Some("web"), remote_ip, true)
            .await;
    }

    let file_path = match state
        .download_manager
        .get_file_path(&session_id, file_id, remote_ip)
        .await
    {
        Ok(p) => p,
        Err(status) => return empty_response(status),
    };

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download.bin")
        .to_string();

    let file_size = tokio::fs::metadata(&file_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    // If share_link_auto_accept is true and this is a browser download, user has to click to accept!
    if is_web && auto_accept
        && let Some(event_tx) = &state.event_tx
    {
        let (decision_tx, decision_rx) = oneshot::channel::<bool>();
            let download_id = Uuid::new_v4().to_string();

            let event = ServerTransferEvent::BrowserDownload {
                download_id: download_id.clone(),
                remote_ip,
                file_name: file_name.clone(),
                file_size,
                decision_tx,
            };

            if event_tx.send(event).await.is_err() {
                return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
            }

            match tokio::time::timeout(std::time::Duration::from_secs(60), decision_rx).await {
                Ok(Ok(true)) => {
                    tracing::info!("Browser download approved for {file_name} from {remote_ip}");
                }
                Ok(Ok(false)) => {
                    tracing::info!("Browser download declined for {file_name} from {remote_ip}");
                    return empty_response(StatusCode::FORBIDDEN);
                }
                _ => {
                    tracing::warn!("Browser download timed out or cancelled for {file_name}");
                    return empty_response(StatusCode::REQUEST_TIMEOUT);
                }
            }
        }

    let content = match tokio::fs::read(&file_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to read download file {:?}: {e}", file_path);
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
        .header(
            hyper::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{file_name}\""),
        )
        .header(hyper::header::CONTENT_LENGTH, content.len())
        .body(Full::new(Bytes::from(content)))
        .unwrap()
}

/// Helper converting byte size into human-readable representation.
fn format_file_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Handler serving the web portal for browser downloads (Share via link).
pub async fn handle_web_portal(req: &Request<Incoming>, state: &ServerState) -> Response<Full<Bytes>> {
    let alias = {
        let cfg = state.config.read().await;
        if !check_share_link_auth(req, &cfg) {
            return unauthorized_response();
        }
        escape_html(&cfg.alias)
    };

    let files = state.download_manager.get_file_dtos().await;

    let mut files_html = String::new();
    if files.is_empty() {
        files_html.push_str(
            r#"<div class="empty-state">
                <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                <p>No files are currently being shared.</p>
                <span>Keep this page open and wait for the sender to share files.</span>
            </div>"#,
        );
    } else {
        for (id, file) in &files {
            let encoded_name = escape_html(&file.file_name);
            let size_str = format_file_size(file.size);
            files_html.push_str(&format!(
                r#"<div class="file-item">
                    <div class="file-info">
                        <div class="file-icon">
                            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>
                        </div>
                        <div class="file-details">
                            <div class="file-name" title="{encoded_name}">{encoded_name}</div>
                            <div class="file-size">{size_str}</div>
                        </div>
                    </div>
                    <a class="download-btn" href="/api/localsend/v2/download?sessionId=web&fileId={id}" download="{encoded_name}">
                        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
                        Download
                    </a>
                </div>"#
            ));
        }
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <meta name="color-scheme" content="light dark">
    <title>LanFlare - Download Files</title>
    <script>
        (function() {{
            try {{
                var saved = localStorage.getItem('lanflare_theme');
                if (saved === 'dark' || saved === 'light') {{
                    document.documentElement.setAttribute('data-theme', saved);
                }}
            }} catch(e) {{}}
        }})();
    </script>
    <style>
        :root {{
            color-scheme: light dark;
            --bg-body: #f1f5f9;
            --bg-card: #ffffff;
            --border-card: #e2e8f0;
            --shadow-card: 0 20px 40px -15px rgba(0, 0, 0, 0.08), 0 0 0 1px rgba(0, 0, 0, 0.04);
            --border-header: #f1f5f9;
            --text-title: #0f172a;
            --text-muted: #64748b;
            --text-bold: #1e293b;
            --file-item-bg: #f8fafc;
            --file-item-border: #e2e8f0;
            --file-item-hover-bg: #f1f5f9;
            --file-item-hover-border: #6366f1;
            --file-name: #0f172a;
            --file-size: #64748b;
            --file-icon-bg: rgba(99, 102, 241, 0.1);
            --file-icon-color: #4f46e5;
            --btn-bg: #4f46e5;
            --btn-hover: #4338ca;
            --btn-text: #ffffff;
            --empty-icon: #cbd5e1;
            --empty-title: #334155;
            --empty-desc: #64748b;
            --footer-text: #94a3b8;
            --toggle-bg: #f1f5f9;
            --toggle-border: #e2e8f0;
            --toggle-color: #475569;
            --toggle-hover-bg: #e2e8f0;
        }}
        @media (prefers-color-scheme: dark) {{
            :root:not([data-theme="light"]) {{
                --bg-body: #090d16;
                --bg-card: #111827;
                --border-card: #1f2937;
                --shadow-card: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
                --border-header: #1f2937;
                --text-title: #f8fafc;
                --text-muted: #94a3b8;
                --text-bold: #e2e8f0;
                --file-item-bg: #1e293b;
                --file-item-border: #334155;
                --file-item-hover-bg: #243248;
                --file-item-hover-border: #6366f1;
                --file-name: #f1f5f9;
                --file-size: #94a3b8;
                --file-icon-bg: rgba(99, 102, 241, 0.15);
                --file-icon-color: #818cf8;
                --btn-bg: #4f46e5;
                --btn-hover: #4338ca;
                --btn-text: #ffffff;
                --empty-icon: #475569;
                --empty-title: #94a3b8;
                --empty-desc: #64748b;
                --footer-text: #64748b;
                --toggle-bg: #1e293b;
                --toggle-border: #334155;
                --toggle-color: #94a3b8;
                --toggle-hover-bg: #2d3d54;
            }}
        }}
        :root[data-theme="dark"] {{
            --bg-body: #090d16;
            --bg-card: #111827;
            --border-card: #1f2937;
            --shadow-card: 0 25px 50px -12px rgba(0, 0, 0, 0.5);
            --border-header: #1f2937;
            --text-title: #f8fafc;
            --text-muted: #94a3b8;
            --text-bold: #e2e8f0;
            --file-item-bg: #1e293b;
            --file-item-border: #334155;
            --file-item-hover-bg: #243248;
            --file-item-hover-border: #6366f1;
            --file-name: #f1f5f9;
            --file-size: #94a3b8;
            --file-icon-bg: rgba(99, 102, 241, 0.15);
            --file-icon-color: #818cf8;
            --btn-bg: #4f46e5;
            --btn-hover: #4338ca;
            --btn-text: #ffffff;
            --empty-icon: #475569;
            --empty-title: #94a3b8;
            --empty-desc: #64748b;
            --footer-text: #64748b;
            --toggle-bg: #1e293b;
            --toggle-border: #334155;
            --toggle-color: #94a3b8;
            --toggle-hover-bg: #2d3d54;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: var(--bg-body);
            color: var(--text-title);
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 20px;
            transition: background 0.2s ease, color 0.2s ease;
        }}
        .card {{
            width: 100%;
            max-width: 520px;
            background: var(--bg-card);
            border: 1px solid var(--border-card);
            border-radius: 20px;
            padding: 28px;
            box-shadow: var(--shadow-card);
            transition: background 0.2s ease, border-color 0.2s ease, box-shadow 0.2s ease;
        }}
        .header {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            gap: 14px;
            margin-bottom: 24px;
            padding-bottom: 18px;
            border-bottom: 1px solid var(--border-header);
        }}
        .header-main {{
            display: flex;
            align-items: center;
            gap: 14px;
            min-width: 0;
            flex: 1;
        }}
        .logo {{
            width: 44px;
            height: 44px;
            border-radius: 12px;
            background: linear-gradient(135deg, #6366f1, #8b5cf6);
            display: flex;
            align-items: center;
            justify-content: center;
            color: white;
            box-shadow: 0 4px 14px rgba(99, 102, 241, 0.35);
            flex-shrink: 0;
        }}
        .title h1 {{ font-size: 18px; font-weight: 700; color: var(--text-title); }}
        .title p {{ font-size: 13px; color: var(--text-muted); margin-top: 2px; }}
        .title p strong {{ color: var(--text-bold); }}
        .theme-toggle {{
            width: 36px;
            height: 36px;
            border-radius: 10px;
            background: var(--toggle-bg);
            border: 1px solid var(--toggle-border);
            color: var(--toggle-color);
            display: flex;
            align-items: center;
            justify-content: center;
            cursor: pointer;
            transition: all 0.15s ease;
            flex-shrink: 0;
        }}
        .theme-toggle:hover {{
            background: var(--toggle-hover-bg);
            color: var(--text-title);
        }}
        .file-list {{ display: flex; flex-direction: column; gap: 10px; }}
        .file-item {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            padding: 12px 14px;
            background: var(--file-item-bg);
            border: 1px solid var(--file-item-border);
            border-radius: 12px;
            gap: 12px;
            transition: all 0.15s ease;
        }}
        .file-item:hover {{
            background: var(--file-item-hover-bg);
            border-color: var(--file-item-hover-border);
        }}
        .file-info {{ display: flex; align-items: center; gap: 12px; min-width: 0; flex: 1; }}
        .file-icon {{
            width: 38px;
            height: 38px;
            border-radius: 8px;
            background: var(--file-icon-bg);
            color: var(--file-icon-color);
            display: flex;
            align-items: center;
            justify-content: center;
            flex-shrink: 0;
        }}
        .file-details {{ min-width: 0; flex: 1; }}
        .file-name {{
            font-size: 13px;
            font-weight: 600;
            color: var(--file-name);
            white-space: nowrap;
            overflow: hidden;
            text-overflow: ellipsis;
        }}
        .file-size {{ font-size: 11px; color: var(--file-size); font-family: monospace; margin-top: 2px; }}
        .download-btn {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            padding: 8px 14px;
            border-radius: 10px;
            background: var(--btn-bg);
            color: var(--btn-text);
            text-decoration: none;
            font-size: 12px;
            font-weight: 600;
            flex-shrink: 0;
            transition: background 0.15s ease;
        }}
        .download-btn:hover {{ background: var(--btn-hover); }}
        .empty-state {{
            padding: 40px 20px;
            text-align: center;
            color: var(--empty-desc);
        }}
        .empty-state svg {{ margin: 0 auto 12px; stroke: var(--empty-icon); }}
        .empty-state p {{ font-size: 14px; font-weight: 500; color: var(--empty-title); }}
        .empty-state span {{ font-size: 12px; color: var(--empty-desc); margin-top: 4px; display: block; }}
        .footer {{
            margin-top: 20px;
            text-align: center;
            font-size: 11px;
            color: var(--footer-text);
        }}
    </style>
</head>
<body>
    <div class="card">
        <div class="header">
            <div class="header-main">
                <div class="logo">
                    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z"/></svg>
                </div>
                <div class="title">
                    <h1>LanFlare Web Share</h1>
                    <p>Shared by <strong>{alias}</strong></p>
                </div>
            </div>
            <button class="theme-toggle" id="themeToggle" onclick="toggleTheme()" title="Toggle theme (System / Light / Dark)" aria-label="Toggle theme">
                <svg id="iconSun" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="display:none;"><circle cx="12" cy="12" r="5"/><line x1="12" y1="1" x2="12" y2="3"/><line x1="12" y1="21" x2="12" y2="23"/><line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/><line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/><line x1="1" y1="12" x2="3" y2="12"/><line x1="21" y1="12" x2="23" y2="12"/><line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/><line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/></svg>
                <svg id="iconMoon" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="display:none;"><path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/></svg>
            </button>
        </div>
        <div class="file-list">
            {files_html}
        </div>
        <div class="footer">
            LocalSend Protocol v2.2 Compatible &bull; Fast Local Wi-Fi Transfer
        </div>
    </div>
    <script>
        function isDark() {{
            var t = document.documentElement.getAttribute('data-theme');
            if (t === 'dark') return true;
            if (t === 'light') return false;
            return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
        }}
        function updateIcons() {{
            var dark = isDark();
            var sun = document.getElementById('iconSun');
            var moon = document.getElementById('iconMoon');
            if (sun && moon) {{
                sun.style.display = dark ? 'block' : 'none';
                moon.style.display = dark ? 'none' : 'block';
            }}
        }}
        function toggleTheme() {{
            var next = isDark() ? 'light' : 'dark';
            document.documentElement.setAttribute('data-theme', next);
            try {{ localStorage.setItem('lanflare_theme', next); }} catch(e) {{}}
            updateIcons();
        }}
        updateIcons();
        if (window.matchMedia) {{
            window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', function() {{
                if (!localStorage.getItem('lanflare_theme')) {{
                    updateIcons();
                }}
            }});
        }}
    </script>
</body>
</html>"#
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Full::new(Bytes::from(html)))
        .unwrap()
}
