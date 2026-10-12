//! HTTP request handlers for LocalSend v2.2 endpoints.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
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
use crate::server::session::{
    DownloadManager, FileTransferStatus, PrepareUploadDecision, SessionEndReason, SessionFile,
    SessionManager,
};
use crate::storage::path::{resolve_unique_destination_path, sanitize_filename};
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
}

/// Shared application state injected into hyper request handlers.
#[derive(Clone)]
pub struct ServerState {
    pub config: Arc<AppConfig>,
    pub cert_fingerprint: String,
    pub session_manager: Arc<SessionManager>,
    pub download_manager: Arc<DownloadManager>,
    pub registry: Arc<PeerRegistry>,
    pub event_tx: Option<mpsc::Sender<ServerTransferEvent>>,
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
pub async fn handle_info(state: &ServerState) -> Response<Full<Bytes>> {
    let files = state.download_manager.get_file_dtos().await;
    let resp = InfoResponseDtoV2 {
        alias: state.config.alias.clone(),
        version: PROTOCOL_VERSION_V2.to_string(),
        device_model: Some(std::env::consts::OS.to_string()),
        device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
        fingerprint: state.cert_fingerprint.clone(),
        port: Some(state.config.port),
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
    let whole_body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };

    let dto: RegisterDtoV2 = match serde_json::from_slice(&whole_body) {
        Ok(d) => d,
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
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
    let resp = RegisterResponseDtoV2 {
        alias: state.config.alias.clone(),
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

    // Validate PIN
    if let Err(status) = state
        .session_manager
        .pin_tracker()
        .check_pin(state.config.pin.as_deref(), provided_pin, remote_ip)
        .await
    {
        return empty_response(status);
    }

    let whole_body = match req.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };

    let payload: PrepareUploadRequestDtoV2 = match serde_json::from_slice(&whole_body) {
        Ok(p) => p,
        Err(_) => return empty_response(StatusCode::BAD_REQUEST),
    };

    if payload.files.is_empty() {
        return empty_response(StatusCode::BAD_REQUEST);
    }

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
    let decision = if state.config.auto_accept {
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
        let safe_name = match sanitize_filename(&file_dto.file_name) {
            Ok(name) => name,
            Err(_) => format!("file_{}.bin", Uuid::new_v4().simple()),
        };

        let target_path = match resolve_unique_destination_path(&state.config.download_dir, &safe_name)
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
                alias: state.config.alias.clone(),
                version: PROTOCOL_VERSION_V2.to_string(),
                device_model: Some(std::env::consts::OS.to_string()),
                device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
                fingerprint: state.cert_fingerprint.clone(),
                port: Some(state.config.port),
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
        .check_pin(state.config.pin.as_deref(), provided_pin, remote_ip)
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
            alias: state.config.alias.clone(),
            version: PROTOCOL_VERSION_V2.to_string(),
            device_model: Some(std::env::consts::OS.to_string()),
            device_type: Some(lanflare_proto::discovery::DeviceType::Desktop),
            fingerprint: state.cert_fingerprint.clone(),
            port: Some(state.config.port),
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
    let (Some(session_id), Some(file_id)) = (query.get("sessionId"), query.get("fileId")) else {
        return empty_response(StatusCode::BAD_REQUEST);
    };

    let file_path = match state
        .download_manager
        .get_file_path(session_id, file_id, remote_ip)
        .await
    {
        Ok(p) => p,
        Err(status) => return empty_response(status),
    };

    let content = match tokio::fs::read(&file_path).await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("Failed to read download file {:?}: {e}", file_path);
            return empty_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download.bin");

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
