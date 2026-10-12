//! Tauri IPC commands bridging GUI frontend to lanflare-core.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, State};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{LsClient, LsClientConfig};
use lanflare_core::discovery::interface::{local_interfaces, InterfaceFilter};
use lanflare_core::discovery::mdns::{start_mdns_engine, MdnsConfig, MdnsDiscoveryEvent, MdnsNodeInfo};
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastNodeInfo,
};
use lanflare_core::discovery::registry::DiscoveredPeer;
use lanflare_core::server::{
    start_server, PrepareUploadDecision, ServerTransferEvent,
};
use lanflare_proto::discovery::{DeviceType, ProtocolType, RegisterDtoV2};

use crate::state::{
    ActiveOutgoingSession, DesktopState, IncomingFileInfo, PendingIncomingSession,
};

/// DTO for returning current app configuration and node identity.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatusDto {
    pub alias: String,
    pub port: u16,
    pub download_dir: String,
    pub auto_accept: bool,
    pub pin_enabled: bool,
    pub fingerprint: String,
    pub interfaces: Vec<NetworkInterfaceDto>,
}

/// DTO for reporting network interface details.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterfaceDto {
    pub name: String,
    pub address: String,
    pub is_ipv6: bool,
}

/// Payload for updating settings from GUI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConfigPayload {
    pub alias: Option<String>,
    pub port: Option<u16>,
    pub download_dir: Option<String>,
    pub auto_accept: Option<bool>,
    pub pin: Option<Option<String>>,
}

/// Payload emitted over Tauri events when an incoming transfer request arrives.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingTransferEventPayload {
    pub session_id: String,
    pub sender_alias: String,
    pub sender_ip: String,
    pub files: Vec<IncomingFileInfo>,
    pub total_bytes: u64,
}

/// Payload for initiating a file send transfer.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFilesPayload {
    pub target_fingerprint: String,
    pub files: Vec<String>,
    pub pin: Option<String>,
}

/// Payload emitted to report file transfer progress to GUI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgressPayload {
    pub session_id: String,
    pub event_type: String, // "started", "file_progress", "file_finished", "file_failed", "session_completed", "session_cancelled"
    pub file_id: Option<String>,
    pub file_name: Option<String>,
    pub file_bytes_sent: Option<u64>,
    pub file_total_bytes: Option<u64>,
    pub session_bytes_sent: Option<u64>,
    pub session_total_bytes: Option<u64>,
    pub bytes_per_second: Option<u64>,
    pub eta_seconds: Option<u64>,
    pub error: Option<String>,
}

// ----------------------------------------------------------------------------
// Tauri IPC Commands
// ----------------------------------------------------------------------------

/// Retrieves current node status, configuration, and available network interfaces.
#[tauri::command]
pub async fn get_node_status(state: State<'_, Arc<DesktopState>>) -> Result<NodeStatusDto, String> {
    let cfg = state.config.read().await.clone();
    let fingerprint = state.identity.fingerprint.clone();

    let mut ifaces = Vec::new();
    let filter = InterfaceFilter::default();
    if let Ok(interfaces) = local_interfaces(&filter) {
        for v4 in interfaces.v4 {
            ifaces.push(NetworkInterfaceDto {
                name: v4.name,
                address: v4.address.to_string(),
                is_ipv6: false,
            });
        }
        for v6 in interfaces.v6 {
            ifaces.push(NetworkInterfaceDto {
                name: v6.name,
                address: format!("index {}", v6.index),
                is_ipv6: true,
            });
        }
    }

    Ok(NodeStatusDto {
        alias: cfg.alias,
        port: cfg.port,
        download_dir: cfg.download_dir.to_string_lossy().to_string(),
        auto_accept: cfg.auto_accept,
        pin_enabled: cfg.pin.is_some(),
        fingerprint,
        interfaces: ifaces,
    })
}

/// Updates application configuration (alias, port, download directory, PIN).
#[tauri::command]
pub async fn update_config(
    state: State<'_, Arc<DesktopState>>,
    payload: UpdateConfigPayload,
) -> Result<NodeStatusDto, String> {
    let mut cfg = state.config.write().await;

    if let Some(alias) = payload.alias {
        cfg.alias = alias;
    }
    if let Some(port) = payload.port {
        cfg.port = port;
    }
    if let Some(dir) = payload.download_dir {
        cfg.download_dir = PathBuf::from(dir);
    }
    if let Some(auto_accept) = payload.auto_accept {
        cfg.auto_accept = auto_accept;
    }
    if let Some(pin_opt) = payload.pin {
        cfg.pin = pin_opt;
    }

    drop(cfg);
    get_node_status(state).await
}

/// Lists all active peers currently in the registry.
#[tauri::command]
pub async fn get_discovered_peers(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Vec<DiscoveredPeer>, String> {
    Ok(state.registry.get_all().await)
}

/// Starts dual discovery engines (Multicast and mDNS) and emits peer updates to GUI.
#[tauri::command]
pub async fn start_discovery(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<(), String> {
    let mut disc = state.discovery.lock().await;

    // Stop existing if running
    if let Some(h) = disc.multicast_handle.take() {
        h.stop().await;
    }
    if let Some(h) = disc.mdns_handle.take() {
        h.stop().await;
    }

    let cfg = state.config.read().await.clone();
    let fp = state.identity.fingerprint.clone();

    // 1. Multicast Engine
    let (_mc_stop_tx, mc_stop_rx) = oneshot::channel();
    let (mc_event_tx, mut mc_event_rx) = mpsc::channel::<MulticastDiscoveryEvent>(100);

    let mc_config = MulticastConfig {
        port: cfg.port,
        node_info: MulticastNodeInfo {
            alias: cfg.alias.clone(),
            version: "2.0".to_string(),
            device_model: Some("LanFlare Desktop".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: fp.clone(),
            port: cfg.port,
            protocol: ProtocolType::Https,
            download: false,
        },
        ..MulticastConfig::default()
    };

    match start_multicast_engine(mc_config, mc_event_tx, mc_stop_rx).await {
        Ok(handle) => {
            let h_clone = handle.clone();
            tokio::spawn(async move {
                h_clone.announce_burst().await;
            });
            disc.multicast_handle = Some(handle);
        }
        Err(e) => {
            tracing::warn!("Failed to start multicast engine: {e}");
        }
    }

    // 2. mDNS Engine
    let (_mdns_stop_tx, mdns_stop_rx) = oneshot::channel();
    let (mdns_event_tx, mut mdns_event_rx) = mpsc::channel::<MdnsDiscoveryEvent>(100);
    let mdns_config = MdnsConfig {
        node_info: MdnsNodeInfo {
            alias: cfg.alias.clone(),
            instance_name: String::new(),
            version: "2.0".to_string(),
            device_model: Some("LanFlare Desktop".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: fp.clone(),
            port: cfg.port,
            protocol: ProtocolType::Https,
            download: false,
        },
        enable_responder: true,
        enable_browser: true,
        ..MdnsConfig::default()
    };

    match start_mdns_engine(mdns_config, mdns_event_tx, mdns_stop_rx).await {
        Ok(handle) => {
            disc.mdns_handle = Some(handle);
        }
        Err(e) => {
            tracing::warn!("Failed to start mDNS engine: {e}");
        }
    }

    // Ingest events into registry and emit to GUI
    let reg_mc = Arc::clone(&state.registry);
    let app_mc = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = mc_event_rx.recv().await {
            match ev {
                MulticastDiscoveryEvent::PeerDiscovered { remote_ip, scope_id, message } => {
                    reg_mc.handle_multicast(remote_ip, scope_id, &message).await;
                    let _ = app_mc.emit("lanflare://peers-updated", ());
                }
                MulticastDiscoveryEvent::AllSocketsFailed => {}
            }
        }
    });

    let reg_mdns = Arc::clone(&state.registry);
    let app_mdns = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = mdns_event_rx.recv().await {
            match ev {
                MdnsDiscoveryEvent::PeerDiscovered(p) => {
                    reg_mdns.handle_mdns(&p).await;
                    let _ = app_mdns.emit("lanflare://peers-updated", ());
                }
                MdnsDiscoveryEvent::PeerRemoved { fullname, .. } => {
                    tracing::debug!("mDNS service removed: {fullname}");
                    let _ = app_mdns.emit("lanflare://peers-updated", ());
                }
            }
        }
    });

    Ok(())
}

/// Triggers an immediate announcement burst across discovery engines.
#[tauri::command]
pub async fn announce_presence(state: State<'_, Arc<DesktopState>>) -> Result<(), String> {
    let disc = state.discovery.lock().await;
    if let Some(ref h) = disc.multicast_handle {
        h.announce_burst().await;
    }
    Ok(())
}

/// Stops active discovery engines.
#[tauri::command]
pub async fn stop_discovery(state: State<'_, Arc<DesktopState>>) -> Result<(), String> {
    let mut disc = state.discovery.lock().await;
    if let Some(h) = disc.multicast_handle.take() {
        h.stop().await;
    }
    if let Some(h) = disc.mdns_handle.take() {
        h.stop().await;
    }
    Ok(())
}

/// Starts the Hyper HTTP/HTTPS receiver server daemon.
#[tauri::command]
pub async fn start_receiver_server(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<u16, String> {
    let mut srv = state.server_handle.write().await;
    if let Some(ref h) = *srv {
        return Ok(h.bound_port());
    }

    let cfg = state.config.read().await.clone();
    let (event_tx, mut event_rx) = mpsc::channel::<ServerTransferEvent>(100);

    let handle = start_server(
        cfg.clone(),
        Some(state.identity.clone()),
        Arc::clone(&state.registry),
        Some(event_tx),
    )
    .await
    .map_err(|e| format!("Failed to start server: {e}"))?;

    let port = handle.bound_port();
    *srv = Some(handle);

    // Event pump for incoming transfer sessions
    let state_clone = Arc::clone(&state);
    let app_clone = app.clone();

    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                ServerTransferEvent::DeviceRegistered { ip, info } => {
                    tracing::info!("Registered device: {} from {ip}", info.alias);
                }
                ServerTransferEvent::PrepareUpload {
                    session_id,
                    sender_ip,
                    sender_info,
                    files,
                    decision_tx,
                    ..
                } => {
                    let mut file_infos = Vec::new();
                    let mut total_bytes = 0u64;

                    for (id, f) in &files {
                        total_bytes += f.size;
                        file_infos.push(IncomingFileInfo {
                            id: id.clone(),
                            file_name: f.file_name.clone(),
                            size: f.size,
                            file_type: f.file_type.clone(),
                        });
                    }

                    let auto_accept = state_clone.config.read().await.auto_accept;
                    if auto_accept {
                        let all_ids = files.keys().cloned().collect();
                        let _ = decision_tx.send(PrepareUploadDecision::Accept(all_ids));
                        let _ = app_clone.emit(
                            "lanflare://incoming-transfer",
                            IncomingTransferEventPayload {
                                session_id,
                                sender_alias: sender_info.alias,
                                sender_ip: sender_ip.to_string(),
                                files: file_infos,
                                total_bytes,
                            },
                        );
                    } else {
                        // Store pending decision and notify GUI to prompt user
                        let mut pending = state_clone.pending_incoming.lock().await;
                        *pending = Some(PendingIncomingSession {
                            session_id: session_id.clone(),
                            sender_alias: sender_info.alias.clone(),
                            sender_ip: sender_ip.to_string(),
                            files: file_infos.clone(),
                            decision_tx: Some(decision_tx),
                        });

                        let _ = app_clone.emit(
                            "lanflare://incoming-transfer-request",
                            IncomingTransferEventPayload {
                                session_id,
                                sender_alias: sender_info.alias,
                                sender_ip: sender_ip.to_string(),
                                files: file_infos,
                                total_bytes,
                            },
                        );
                    }
                }
                ServerTransferEvent::FileStarted {
                    session_id,
                    file_id,
                    file_name,
                    total_bytes,
                } => {
                    let _ = app_clone.emit(
                        "lanflare://file-started",
                        serde_json::json!({
                            "sessionId": session_id,
                            "fileId": file_id,
                            "fileName": file_name,
                            "totalBytes": total_bytes,
                        }),
                    );
                }
                ServerTransferEvent::FileCompleted {
                    session_id,
                    file_id,
                    result,
                } => {
                    let file_name = result
                        .target_path
                        .file_name()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let _ = app_clone.emit(
                        "lanflare://file-received",
                        serde_json::json!({
                            "sessionId": session_id,
                            "fileId": file_id,
                            "fileName": file_name,
                            "size": result.bytes_written,
                            "savedPath": result.target_path.to_string_lossy(),
                            "sha256": result.sha256,
                        }),
                    );
                }
                ServerTransferEvent::FileFailed {
                    session_id,
                    file_id,
                    error,
                } => {
                    let _ = app_clone.emit(
                        "lanflare://file-failed",
                        serde_json::json!({
                            "sessionId": session_id,
                            "fileId": file_id,
                            "error": error,
                        }),
                    );
                }
                ServerTransferEvent::SessionEnded {
                    session_id,
                    reason,
                } => {
                    let _ = app_clone.emit(
                        "lanflare://session-finished",
                        serde_json::json!({
                            "sessionId": session_id,
                            "reason": format!("{reason:?}"),
                        }),
                    );
                }
            }
        }
    });

    Ok(port)
}

/// Responds to a pending incoming transfer request (accept or decline).
#[tauri::command]
pub async fn respond_incoming_transfer(
    state: State<'_, Arc<DesktopState>>,
    session_id: String,
    accept: bool,
) -> Result<(), String> {
    let mut pending = state.pending_incoming.lock().await;
    if let Some(mut sess) = pending.take() {
        if sess.session_id == session_id {
            if let Some(tx) = sess.decision_tx.take() {
                let decision = if accept {
                    let file_ids = sess.files.iter().map(|f| f.id.clone()).collect();
                    PrepareUploadDecision::Accept(file_ids)
                } else {
                    PrepareUploadDecision::Decline
                };
                let _ = tx.send(decision);
                return Ok(());
            }
        } else {
            // Restore if different session
            *pending = Some(sess);
        }
    }
    Err("No matching pending transfer session found".to_string())
}

/// Stops the running receiver server daemon.
#[tauri::command]
pub async fn stop_receiver_server(state: State<'_, Arc<DesktopState>>) -> Result<(), String> {
    let mut srv = state.server_handle.write().await;
    if let Some(h) = srv.take() {
        h.stop();
    }
    Ok(())
}

/// Initiates an outgoing file transfer to a target peer.
#[tauri::command]
pub async fn send_files(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    payload: SendFilesPayload,
) -> Result<String, String> {
    if payload.files.is_empty() {
        return Err("No files specified for transfer".to_string());
    }

    // 1. Find peer in registry by fingerprint
    let target_peer = state
        .registry
        .get(&payload.target_fingerprint)
        .await
        .ok_or_else(|| format!("Peer with fingerprint {} not found", payload.target_fingerprint))?;

    let base_url = target_peer
        .primary_base_url()
        .ok_or_else(|| "Target peer has no valid IP address".to_string())?;

    // 2. Prepare outgoing files
    let mut outgoing = Vec::with_capacity(payload.files.len());
    let mut total_bytes = 0u64;

    for path_str in &payload.files {
        let path = PathBuf::from(path_str);
        if !path.exists() {
            return Err(format!("File does not exist: {}", path.display()));
        }
        let out_file = OutgoingFile::from_path(&path)
            .await
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?
            .with_computed_sha256()
            .await
            .map_err(|e| format!("Failed to hash {}: {e}", path.display()))?;

        total_bytes += out_file.size;
        outgoing.push(out_file);
    }

    // 3. Create client with pinned certificate verifier
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(5),
        tls_identity: Some(state.identity.clone()),
        target_fingerprint: Some(target_peer.fingerprint.clone()),
    };
    let client = LsClient::new(client_config)
        .map_err(|e| format!("Failed to build transfer client: {e}"))?;

    let cfg = state.config.read().await.clone();
    let sender_info = RegisterDtoV2 {
        alias: cfg.alias.clone(),
        version: "2.0".to_string(),
        device_model: Some("LanFlare Desktop".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: state.identity.fingerprint.clone(),
        port: cfg.port,
        protocol: ProtocolType::Https,
        download: false,
    };
    let orchestrator = TransferOrchestrator::new(client, sender_info);

    let cancel_token = CancellationToken::new();
    let transfer_opts = TransferOptions {
        max_concurrency: 4,
        pin: payload.pin,
        cancel_token: Some(cancel_token.clone()),
        progress_interval: Duration::from_millis(100),
        ..Default::default()
    };

    let session_uuid = uuid::Uuid::new_v4().to_string();

    // Store active outgoing session
    {
        let mut active = state.active_outgoing.lock().await;
        *active = Some(ActiveOutgoingSession {
            session_id: session_uuid.clone(),
            target_alias: target_peer.alias.clone(),
            target_ip: base_url.clone(),
            total_files: outgoing.len(),
            total_bytes,
            cancel_token,
        });
    }

    // 4. Spawn background transfer task with progress channel
    let (progress_tx, mut progress_rx) = mpsc::channel::<TransferProgress>(200);
    let app_clone = app.clone();
    let sess_id_clone = session_uuid.clone();

    tokio::spawn(async move {
        while let Some(prog) = progress_rx.recv().await {
            let payload = match prog {
                TransferProgress::Started {
                    session_id,
                    total_bytes,
                    ..
                } => TransferProgressPayload {
                    session_id,
                    event_type: "started".to_string(),
                    file_id: None,
                    file_name: None,
                    file_bytes_sent: None,
                    file_total_bytes: None,
                    session_bytes_sent: Some(0),
                    session_total_bytes: Some(total_bytes),
                    bytes_per_second: Some(0),
                    eta_seconds: None,
                    error: None,
                },
                TransferProgress::FileProgress {
                    session_id,
                    file_id,
                    file_name,
                    file_bytes_sent,
                    file_total_bytes,
                    session_bytes_sent,
                    session_total_bytes,
                    bytes_per_second,
                    eta,
                } => TransferProgressPayload {
                    session_id,
                    event_type: "file_progress".to_string(),
                    file_id: Some(file_id),
                    file_name: Some(file_name),
                    file_bytes_sent: Some(file_bytes_sent),
                    file_total_bytes: Some(file_total_bytes),
                    session_bytes_sent: Some(session_bytes_sent),
                    session_total_bytes: Some(session_total_bytes),
                    bytes_per_second: Some(bytes_per_second),
                    eta_seconds: eta.map(|d| d.as_secs()),
                    error: None,
                },
                TransferProgress::FileFinished {
                    session_id,
                    file_id,
                    file_name,
                } => TransferProgressPayload {
                    session_id,
                    event_type: "file_finished".to_string(),
                    file_id: Some(file_id),
                    file_name: Some(file_name),
                    file_bytes_sent: None,
                    file_total_bytes: None,
                    session_bytes_sent: None,
                    session_total_bytes: None,
                    bytes_per_second: None,
                    eta_seconds: None,
                    error: None,
                },
                TransferProgress::FileFailed {
                    session_id,
                    file_id,
                    file_name,
                    error,
                } => TransferProgressPayload {
                    session_id,
                    event_type: "file_failed".to_string(),
                    file_id: Some(file_id),
                    file_name: Some(file_name),
                    file_bytes_sent: None,
                    file_total_bytes: None,
                    session_bytes_sent: None,
                    session_total_bytes: None,
                    bytes_per_second: None,
                    eta_seconds: None,
                    error: Some(error),
                },
                TransferProgress::SessionCompleted {
                    session_id,
                    total_bytes,
                    ..
                } => TransferProgressPayload {
                    session_id,
                    event_type: "session_completed".to_string(),
                    file_id: None,
                    file_name: None,
                    file_bytes_sent: None,
                    file_total_bytes: None,
                    session_bytes_sent: Some(total_bytes),
                    session_total_bytes: Some(total_bytes),
                    bytes_per_second: None,
                    eta_seconds: None,
                    error: None,
                },
                TransferProgress::SessionCancelled { session_id, reason } => {
                    TransferProgressPayload {
                        session_id,
                        event_type: "session_cancelled".to_string(),
                        file_id: None,
                        file_name: None,
                        file_bytes_sent: None,
                        file_total_bytes: None,
                        session_bytes_sent: None,
                        session_total_bytes: None,
                        bytes_per_second: None,
                        eta_seconds: None,
                        error: Some(reason),
                    }
                }
            };

            let _ = app_clone.emit("lanflare://transfer-progress", payload);
        }
    });

    tokio::spawn(async move {
        let res = orchestrator
            .transfer(&base_url, outgoing, transfer_opts, Some(progress_tx))
            .await;

        if let Err(e) = res {
            tracing::error!("Outgoing transfer session {sess_id_clone} failed: {e}");
        }
    });

    Ok(session_uuid)
}

/// Cancels an active outgoing file transfer session.
#[tauri::command]
pub async fn cancel_outgoing_transfer(
    state: State<'_, Arc<DesktopState>>,
    session_id: String,
) -> Result<(), String> {
    let mut active = state.active_outgoing.lock().await;
    if let Some(sess) = active.as_ref()
        && sess.session_id == session_id
    {
        sess.cancel_token.cancel();
        *active = None;
        return Ok(());
    }
    Err("No active outgoing transfer matches this session ID".to_string())
}
