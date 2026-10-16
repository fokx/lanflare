//! Tauri IPC commands bridging GUI frontend to lanflare-core.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{ClientError, LsClient, LsClientConfig};
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

/// DTO for DiscoveredPeer matching frontend types.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPeerDto {
    pub fingerprint: String,
    pub alias: String,
    pub version: String,
    pub device_model: Option<String>,
    pub device_type: Option<DeviceType>,
    pub endpoints: Vec<String>,
    pub protocol: ProtocolType,
    pub port: u16,
    pub download: bool,
    pub sources: usize,
    pub last_seen: u64,
}

impl From<&DiscoveredPeer> for DiscoveredPeerDto {
    fn from(p: &DiscoveredPeer) -> Self {
        let mut endpoints = Vec::new();
        for ep in &p.endpoints {
            endpoints.push(format!("{}:{}", ep.ip, p.port));
        }
        if endpoints.is_empty()
            && let Some(ip) = p.primary_ip()
        {
            endpoints.push(format!("{}:{}", ip, p.port));
        }
        Self {
            fingerprint: p.fingerprint.clone(),
            alias: p.alias.clone(),
            version: p.version.clone(),
            device_model: p.device_model.clone(),
            device_type: p.device_type,
            endpoints,
            protocol: p.protocol,
            port: p.port,
            download: p.download,
            sources: p.discovery_sources.len(),
            last_seen: p.last_seen_ms,
        }
    }
}

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
    pub save_to_history: bool,
    pub multicast_address: String,
    pub share_link_auto_accept: bool,
    pub share_link_require_pin: bool,
    pub share_link_pin: String,
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
    pub save_to_history: Option<bool>,
    pub multicast_address: Option<String>,
    pub share_link_auto_accept: Option<bool>,
    pub share_link_require_pin: Option<bool>,
    pub share_link_pin: Option<String>,
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
    pub target_endpoint: Option<String>,
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
        save_to_history: cfg.save_to_history,
        multicast_address: cfg.multicast_address,
        share_link_auto_accept: cfg.share_link_auto_accept,
        share_link_require_pin: cfg.share_link_require_pin,
        share_link_pin: cfg.share_link_pin,
        interfaces: ifaces,
    })
}

/// Updates application configuration (alias, port, download directory, PIN, history, multicast).
#[tauri::command]
pub async fn update_config(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    payload: UpdateConfigPayload,
) -> Result<NodeStatusDto, String> {
    let mut cfg = state.config.write().await;
    let mut restart_discovery = false;

    if let Some(alias) = payload.alias {
        cfg.alias = alias;
    }
    if let Some(port) = payload.port {
        if cfg.port != port {
            restart_discovery = true;
        }
        cfg.port = port;
    }
    if let Some(dir) = payload.download_dir {
        cfg.download_dir = PathBuf::from(dir);
    }
    if let Some(auto_accept) = payload.auto_accept {
        cfg.auto_accept = auto_accept;
    }
    if let Some(pin_opt) = payload.pin {
        cfg.pin = pin_opt.map(|p| lanflare_core::server::extract_password_from_url(&p));
    }
    if let Some(save_hist) = payload.save_to_history {
        cfg.save_to_history = save_hist;
    }
    if let Some(sla) = payload.share_link_auto_accept {
        cfg.share_link_auto_accept = sla;
    }
    if let Some(slrp) = payload.share_link_require_pin {
        cfg.share_link_require_pin = slrp;
    }
    if let Some(slpin) = payload.share_link_pin {
        cfg.share_link_pin = lanflare_core::server::extract_password_from_url(&slpin);
    }
    if let Some(mc_addr_str) = payload.multicast_address {
        let trimmed = mc_addr_str.trim();
        let parsed: std::net::Ipv4Addr = trimmed
            .parse()
            .map_err(|e| format!("Invalid IPv4 multicast address: {e}"))?;
        if !parsed.is_multicast() {
            return Err("Multicast address must be within 224.0.0.0 to 239.255.255.255".to_string());
        }
        if cfg.multicast_address != trimmed {
            restart_discovery = true;
        }
        cfg.multicast_address = trimmed.to_string();
    }

    if let Err(e) = state.save_config_current(&cfg).await {
        tracing::warn!("Failed to persist configuration to disk: {e}");
    }

    drop(cfg);

    if restart_discovery {
        let _ = start_discovery(app, state.clone()).await;
    }

    get_node_status(state).await
}

/// Lists all active peers currently in the registry.
#[tauri::command]
pub async fn get_discovered_peers(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Vec<DiscoveredPeerDto>, String> {
    let peers = state.registry.get_all().await;
    Ok(peers.iter().map(DiscoveredPeerDto::from).collect())
}

/// Starts dual discovery engines (Multicast and mDNS) and emits peer updates to GUI.
#[tauri::command]
pub async fn start_discovery(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<(), String> {
    let mut disc = state.discovery.lock().await;

    // Stop existing if running
    if let Some(tx) = disc.mc_stop_tx.take() {
        let _ = tx.send(());
    }
    if let Some(tx) = disc.mdns_stop_tx.take() {
        let _ = tx.send(());
    }
    if let Some(h) = disc.multicast_handle.take() {
        h.stop().await;
    }
    if let Some(h) = disc.mdns_handle.take() {
        h.stop().await;
    }

    let cfg = state.config.read().await.clone();
    let fp = state.identity.fingerprint.clone();

    // Sweep any stale peers on discovery restart
    let _ = state.registry.sweep_stale_peers().await;

    // 1. Multicast Engine
    let (mc_stop_tx, mc_stop_rx) = oneshot::channel();
    let (mc_event_tx, mut mc_event_rx) = mpsc::channel::<MulticastDiscoveryEvent>(100);

    let mc_group = cfg
        .multicast_address
        .parse::<std::net::Ipv4Addr>()
        .unwrap_or(lanflare_core::discovery::multicast::DEFAULT_MULTICAST_IPV4);

    let mc_config = MulticastConfig {
        group_v4: mc_group,
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
            disc.mc_stop_tx = Some(mc_stop_tx);
        }
        Err(e) => {
            tracing::warn!("Failed to start multicast engine: {e}");
        }
    }

    // 2. mDNS Engine
    let (mdns_stop_tx, mdns_stop_rx) = oneshot::channel();
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
            disc.mdns_stop_tx = Some(mdns_stop_tx);
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

    // Subnet scan fallback / escalation (matching CLI behavior)
    let registry_subnet = Arc::clone(&state.registry);
    let identity_subnet = state.identity.clone();
    let port_subnet = cfg.port;
    let app_subnet = app.clone();
    tokio::spawn(async move {
        // Brief grace period for multicast/mDNS responses
        tokio::time::sleep(Duration::from_millis(500)).await;
        let filter = InterfaceFilter::default();
        if let Ok(interfaces) = local_interfaces(&filter) {
            let valid_ifaces: Vec<_> = interfaces
                .v4
                .into_iter()
                .filter(|iface| lanflare_core::discovery::interface::is_sweepable_interface(&iface.name, &iface.address))
                .collect();

            if !valid_ifaces.is_empty() {
                let scan_tasks = valid_ifaces.into_iter().map(|iface| {
                    let reg = registry_subnet.clone();
                    let ident = identity_subnet.clone();
                    async move {
                        lanflare_core::discovery::scanner::scan_subnet(
                            iface.address,
                            port_subnet,
                            &ident,
                            &reg,
                        )
                        .await
                    }
                });
                futures_util::future::join_all(scan_tasks).await;
                let _ = app_subnet.emit("lanflare://peers-updated", ());
            }
        }
    });

    Ok(())
}

/// Triggers an immediate announcement burst across discovery engines and probes LAN subnets.
#[tauri::command]
pub async fn announce_presence(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<(), String> {
    let disc = state.discovery.lock().await;
    if let Some(ref h) = disc.multicast_handle {
        h.announce_burst().await;
    }
    drop(disc);

    // Sweep any stale peers immediately on refresh
    let evicted = state.registry.sweep_stale_peers().await;
    if evicted > 0 {
        let _ = app.emit("lanflare://peers-updated", ());
    }

    // Also run a subnet scan sweep in background on refresh
    let registry_subnet = Arc::clone(&state.registry);
    let identity_subnet = state.identity.clone();
    let port_subnet = state.config.read().await.port;
    let app_subnet = app.clone();
    tokio::spawn(async move {
        let filter = InterfaceFilter::default();
        if let Ok(interfaces) = local_interfaces(&filter) {
            let valid_ifaces: Vec<_> = interfaces
                .v4
                .into_iter()
                .filter(|iface| lanflare_core::discovery::interface::is_sweepable_interface(&iface.name, &iface.address))
                .collect();

            if !valid_ifaces.is_empty() {
                let scan_tasks = valid_ifaces.into_iter().map(|iface| {
                    let reg = registry_subnet.clone();
                    let ident = identity_subnet.clone();
                    async move {
                        lanflare_core::discovery::scanner::scan_subnet(
                            iface.address,
                            port_subnet,
                            &ident,
                            &reg,
                        )
                        .await
                    }
                });
                futures_util::future::join_all(scan_tasks).await;
                let _ = app_subnet.emit("lanflare://peers-updated", ());
            }
        }
    });

    Ok(())
}

/// Stops active discovery engines.
#[tauri::command]
pub async fn stop_discovery(state: State<'_, Arc<DesktopState>>) -> Result<(), String> {
    let mut disc = state.discovery.lock().await;
    if let Some(tx) = disc.mc_stop_tx.take() {
        let _ = tx.send(());
    }
    if let Some(tx) = disc.mdns_stop_tx.take() {
        let _ = tx.send(());
    }
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

    let (event_tx, mut event_rx) = mpsc::channel::<ServerTransferEvent>(100);

    let handle = start_server(
        state.config.clone(),
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
        struct IncomingSessionTracker {
            start_time: std::time::Instant,
            file_bytes: std::collections::HashMap<String, u64>,
        }
        let mut session_trackers: std::collections::HashMap<String, IncomingSessionTracker> = std::collections::HashMap::new();

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

                    session_trackers.insert(
                        session_id.clone(),
                        IncomingSessionTracker {
                            start_time: std::time::Instant::now(),
                            file_bytes: std::collections::HashMap::new(),
                        },
                    );

                    let auto_accept = state_clone.config.read().await.auto_accept;
                    if auto_accept {
                        let all_ids = files.keys().cloned().collect();
                        let _ = decision_tx.send(PrepareUploadDecision::Accept(all_ids));
                        let notif_body = format!(
                            "Auto-accepting {} file(s) ({:.2} MB) from {}",
                            files.len(),
                            total_bytes as f64 / (1024.0 * 1024.0),
                            sender_info.alias
                        );
                        let _ = app_clone
                            .notification()
                            .builder()
                            .title("LanFlare - Receiving Files")
                            .body(&notif_body)
                            .show();
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

                        // Show system notification
                        let notif_body = format!(
                            "{} wants to send {} file(s) ({:.2} MB)",
                            sender_info.alias,
                            files.len(),
                            total_bytes as f64 / (1024.0 * 1024.0)
                        );
                        if let Err(e) = app_clone
                            .notification()
                            .builder()
                            .title("LanFlare - Incoming Transfer Request")
                            .body(&notif_body)
                            .show()
                        {
                            tracing::warn!("Failed to display system notification: {e}");
                        }

                        // Ensure main window is visible, unminimized, and focused
                        if let Some(window) = app_clone.get_webview_window("main") {
                            #[cfg(desktop)]
                            let _ = window.unminimize();
                            let _ = window.show();
                            let _ = window.set_focus();
                            #[cfg(desktop)]
                            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
                        }

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
                ServerTransferEvent::BrowserDownload {
                    download_id,
                    remote_ip,
                    file_name,
                    file_size,
                    decision_tx,
                } => {
                    state_clone
                        .pending_browser_downloads
                        .lock()
                        .await
                        .insert(download_id.clone(), decision_tx);

                    // Show system notification
                    let notif_body = format!(
                        "Browser at {} requested to download \"{}\"",
                        remote_ip, file_name
                    );
                    let _ = app_clone
                        .notification()
                        .builder()
                        .title("LanFlare - Browser Download Request")
                        .body(&notif_body)
                        .show();

                    // Focus main window
                    if let Some(window) = app_clone.get_webview_window("main") {
                        #[cfg(desktop)]
                        let _ = window.unminimize();
                        let _ = window.show();
                        let _ = window.set_focus();
                        #[cfg(desktop)]
                        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
                    }

                    let _ = app_clone.emit(
                        "lanflare://browser-download-request",
                        serde_json::json!({
                            "downloadId": download_id,
                            "remoteIp": remote_ip.to_string(),
                            "fileName": file_name,
                            "fileSize": file_size,
                        }),
                    );
                }
                ServerTransferEvent::FileStarted {
                    session_id,
                    file_id,
                    file_name,
                    total_bytes,
                } => {
                    session_trackers
                        .entry(session_id.clone())
                        .or_insert_with(|| IncomingSessionTracker {
                            start_time: std::time::Instant::now(),
                            file_bytes: std::collections::HashMap::new(),
                        });

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
                ServerTransferEvent::FileProgress {
                    session_id,
                    file_id,
                    file_name,
                    bytes_received,
                    total_bytes,
                } => {
                    let tracker = session_trackers
                        .entry(session_id.clone())
                        .or_insert_with(|| IncomingSessionTracker {
                            start_time: std::time::Instant::now(),
                            file_bytes: std::collections::HashMap::new(),
                        });
                    tracker.file_bytes.insert(file_id.clone(), bytes_received);

                    let session_bytes: u64 = tracker.file_bytes.values().sum();
                    let elapsed = tracker.start_time.elapsed().as_secs_f64();
                    let bytes_per_second = if elapsed > 0.0 {
                        (session_bytes as f64 / elapsed) as u64
                    } else {
                        0
                    };

                    let _ = app_clone.emit(
                        "lanflare://transfer-progress",
                        TransferProgressPayload {
                            session_id: session_id.clone(),
                            event_type: "file_progress".to_string(),
                            file_id: Some(file_id),
                            file_name: Some(file_name),
                            file_bytes_sent: Some(bytes_received),
                            file_total_bytes: Some(total_bytes),
                            session_bytes_sent: Some(session_bytes),
                            session_total_bytes: None,
                            bytes_per_second: Some(bytes_per_second),
                            eta_seconds: None,
                            error: None,
                        },
                    );
                }
                ServerTransferEvent::FileCompleted {
                    session_id,
                    file_id,
                    result,
                } => {
                    if let Some(tracker) = session_trackers.get_mut(&session_id) {
                        tracker.file_bytes.insert(file_id.clone(), result.bytes_written);
                    }
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
                    session_trackers.remove(&session_id);
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

/// Regenerates a new random share link PIN and persists configuration.
#[tauri::command]
pub async fn regenerate_share_link_pin(
    state: State<'_, Arc<DesktopState>>,
) -> Result<String, String> {
    let mut cfg = state.config.write().await;
    let new_pin = lanflare_core::config::generate_share_link_pin();
    cfg.share_link_pin = new_pin.clone();
    if let Err(e) = state.save_config_current(&cfg).await {
        tracing::warn!("Failed to persist config after pin regeneration: {e}");
    }
    Ok(new_pin)
}

/// Responds to an incoming browser download request (accept or decline).
#[tauri::command]
pub async fn respond_browser_download(
    state: State<'_, Arc<DesktopState>>,
    download_id: String,
    accept: bool,
) -> Result<(), String> {
    let mut pending = state.pending_browser_downloads.lock().await;
    if let Some(tx) = pending.remove(&download_id) {
        let _ = tx.send(accept);
        Ok(())
    } else {
        Err("No pending download request with that ID".to_string())
    }
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

    // 1. Find peer in registry by fingerprint (with auto-probe and auto-scan fallback)
    let target_peer = match state.registry.get(&payload.target_fingerprint).await {
        Some(p) => p,
        None => {
            tracing::info!(
                "Peer with fingerprint {} not in registry. Probing target endpoint or network...",
                payload.target_fingerprint
            );

            let mut resolved_peer = None;

            // Attempt A: Direct probe if target_endpoint provided
            if let Some(ref ep_str) = payload.target_endpoint {
                let trimmed = ep_str.trim();
                let (host_str, port) = if let Some(idx) = trimmed.rfind(':') {
                    (&trimmed[..idx], trimmed[idx + 1..].parse::<u16>().unwrap_or(53317))
                } else {
                    (trimmed, 53317)
                };

                if let Ok(ip) = host_str.parse::<std::net::IpAddr>() {
                    let probe_config = LsClientConfig {
                        timeout: Duration::from_secs(3),
                        connect_timeout: Duration::from_secs(2),
                        tls_identity: Some(state.identity.clone()),
                        target_fingerprint: None,
                    };

                    if let Ok(probe_client) = LsClient::new(probe_config) {
                        for proto in [ProtocolType::Https, ProtocolType::Http] {
                            let proto_str = match proto {
                                ProtocolType::Https => "https",
                                ProtocolType::Http => "http",
                            };
                            let base_url = match ip {
                                std::net::IpAddr::V4(v4) => format!("{proto_str}://{v4}:{port}"),
                                std::net::IpAddr::V6(v6) => format!("{proto_str}://[{v6}]:{port}"),
                            };
                            if let Ok(info) = probe_client.get_info(&base_url).await
                                && (info.fingerprint.eq_ignore_ascii_case(&payload.target_fingerprint)
                                    || payload.target_fingerprint.is_empty())
                            {
                                resolved_peer = state.registry.handle_info(ip, port, proto, &info).await;
                                break;
                            }
                        }
                    }
                }
            }

            // Attempt B: If still not found, do a quick network sweep across interfaces
            if resolved_peer.is_none() {
                let reg = Arc::clone(&state.registry);
                let ident = state.identity.clone();
                let port = state.config.read().await.port;
                let filter = InterfaceFilter::default();

                if let Ok(interfaces) = local_interfaces(&filter) {
                    let valid_ifaces: Vec<_> = interfaces
                        .v4
                        .into_iter()
                        .filter(|iface| lanflare_core::discovery::interface::is_sweepable_interface(&iface.name, &iface.address))
                        .collect();

                    let scan_tasks = valid_ifaces.into_iter().map(|iface| {
                        let reg = reg.clone();
                        let ident = ident.clone();
                        async move {
                            lanflare_core::discovery::scanner::scan_subnet(
                                iface.address,
                                port,
                                &ident,
                                &reg,
                            )
                            .await
                        }
                    });
                    futures_util::future::join_all(scan_tasks).await;
                }

                // Check registry again after sweep
                resolved_peer = state.registry.get(&payload.target_fingerprint).await;
            }

            resolved_peer.ok_or_else(|| {
                format!(
                    "Peer with fingerprint {} not found after re-scanning network. Please ensure the target device is on the same Wi-Fi/LAN.",
                    payload.target_fingerprint
                )
            })?
        }
    };

    let base_url = target_peer
        .primary_base_url()
        .ok_or_else(|| "Target peer has no valid IP address".to_string())?;

    // 2. Prepare outgoing files (including recursive expansion of folders)
    let outgoing = collect_outgoing_files(&app, &payload.files).await?;
    let total_bytes: u64 = outgoing.iter().map(|f| f.size).sum();

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
        pin: payload.pin.map(|p| lanflare_core::server::extract_password_from_url(&p)),
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
            cancel_token: cancel_token.clone(),
        });
    }

    // Emit outgoing-transfer-initiated event so GUI immediately displays "Waiting for response..."
    let outgoing_files_dto: Vec<IncomingFileInfo> = outgoing
        .iter()
        .map(|f| IncomingFileInfo {
            id: f.id.clone(),
            file_name: f.file_name.clone(),
            size: f.size,
            file_type: f.file_type.clone(),
        })
        .collect();

    let _ = app.emit(
        "lanflare://outgoing-transfer-initiated",
        serde_json::json!({
            "sessionId": session_uuid,
            "targetAlias": target_peer.alias,
            "targetEndpoint": base_url,
            "files": outgoing_files_dto,
            "totalBytes": total_bytes,
        }),
    );

    // 4. Spawn background transfer task with progress channel
    let (progress_tx, mut progress_rx) = mpsc::channel::<TransferProgress>(200);
    let app_clone = app.clone();
    let sess_id_event = session_uuid.clone();

    tokio::spawn(async move {
        while let Some(prog) = progress_rx.recv().await {
            let payload = match prog {
                TransferProgress::Started {
                    total_bytes,
                    ..
                } => TransferProgressPayload {
                    session_id: sess_id_event.clone(),
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
                    file_id,
                    file_name,
                    file_bytes_sent,
                    file_total_bytes,
                    session_bytes_sent,
                    session_total_bytes,
                    bytes_per_second,
                    eta,
                    ..
                } => TransferProgressPayload {
                    session_id: sess_id_event.clone(),
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
                    file_id,
                    file_name,
                    ..
                } => TransferProgressPayload {
                    session_id: sess_id_event.clone(),
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
                    file_id,
                    file_name,
                    error,
                    ..
                } => TransferProgressPayload {
                    session_id: sess_id_event.clone(),
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
                    total_bytes,
                    ..
                } => TransferProgressPayload {
                    session_id: sess_id_event.clone(),
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
                TransferProgress::SessionCancelled { reason, .. } => {
                    TransferProgressPayload {
                        session_id: sess_id_event.clone(),
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

    let app_err_clone = app.clone();
    let state_task_clone = Arc::clone(&state);
    let sess_id_err = session_uuid.clone();
    let cancel_token_err = cancel_token.clone();

    tokio::spawn(async move {
        let res = orchestrator
            .transfer(&base_url, outgoing, transfer_opts, Some(progress_tx))
            .await;

        // Clean up active outgoing session if this session is still active
        {
            let mut active = state_task_clone.active_outgoing.lock().await;
            if let Some(ref s) = *active
                && s.session_id == sess_id_err
            {
                *active = None;
            }
        }

        if let Err(e) = res {
            tracing::error!("Outgoing transfer session {sess_id_err} failed: {e}");
            let (event_type, error_msg) = if cancel_token_err.is_cancelled() {
                ("session_cancelled".to_string(), "Transfer cancelled by user.".to_string())
            } else {
                let msg = match &e {
                    ClientError::SessionRejected => "Transfer request was declined by the recipient.".to_string(),
                    ClientError::UnauthorizedPin => "PIN required or invalid PIN provided.".to_string(),
                    ClientError::PinRateLimited => "Too many PIN attempts. Recipient rate-limited requests.".to_string(),
                    ClientError::SessionConflict => "Recipient device is busy with another transfer.".to_string(),
                    ClientError::Reqwest(re) if re.is_timeout() => "Transfer request timed out waiting for recipient response.".to_string(),
                    ClientError::Reqwest(re) if re.is_connect() => "Could not connect to recipient. Please verify network connection.".to_string(),
                    _ => format!("Transfer failed: {e}"),
                };
                ("session_failed".to_string(), msg)
            };

            let _ = app_err_clone.emit(
                "lanflare://transfer-progress",
                TransferProgressPayload {
                    session_id: sess_id_err,
                    event_type,
                    file_id: None,
                    file_name: None,
                    file_bytes_sent: None,
                    file_total_bytes: None,
                    session_bytes_sent: None,
                    session_total_bytes: None,
                    bytes_per_second: None,
                    eta_seconds: None,
                    error: Some(error_msg),
                },
            );
        }
    });

    Ok(session_uuid)
}

/// Cancels an active outgoing file transfer session.
#[tauri::command]
pub async fn cancel_outgoing_transfer(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    session_id: String,
) -> Result<(), String> {
    let mut active = state.active_outgoing.lock().await;
    if let Some(sess) = active.as_ref()
        && sess.session_id == session_id
    {
        sess.cancel_token.cancel();
        *active = None;
        let _ = app.emit(
            "lanflare://transfer-progress",
            TransferProgressPayload {
                session_id: session_id.clone(),
                event_type: "session_cancelled".to_string(),
                file_id: None,
                file_name: None,
                file_bytes_sent: None,
                file_total_bytes: None,
                session_bytes_sent: None,
                session_total_bytes: None,
                bytes_per_second: None,
                eta_seconds: None,
                error: Some("Transfer cancelled by user.".to_string()),
            },
        );
        return Ok(());
    }
    Err("No active outgoing transfer matches this session ID".to_string())
}

/// Helper to asynchronously collect files, recursively traversing directories and recording relative paths.
/// Supports both filesystem paths and Android content:// URIs.
async fn collect_outgoing_files(
    #[allow(unused)] app: &AppHandle,
    paths: &[String],
) -> Result<Vec<OutgoingFile>, String> {
    let mut collected = Vec::new();

    for path_str in paths {
        if path_str.starts_with("content://") {
            #[cfg(target_os = "android")]
            {
                use tauri_plugin_fs::FsExt;
                let file_name = app
                    .path()
                    .file_name(path_str)
                    .unwrap_or_else(|| {
                        path_str.rsplit('/').next().unwrap_or("file").to_string()
                    });

                let cache_dir = app
                    .path()
                    .cache_dir()
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join("lanflare_outgoing");
                tokio::fs::create_dir_all(&cache_dir)
                    .await
                    .map_err(|e| format!("Failed to create outgoing cache dir: {e}"))?;

                let dest_path = cache_dir.join(&file_name);
                let url = url::Url::parse(path_str)
                    .map_err(|e| format!("Invalid content URI {path_str}: {e}"))?;
                let file_path = tauri_plugin_fs::FilePath::Url(url);
                let mut opts = tauri_plugin_fs::OpenOptions::default();
                opts.read(true);
                let mut src_file = app
                    .fs()
                    .open(file_path, opts)
                    .map_err(|e| format!("Failed to open content URI {path_str}: {e}"))?;

                let mut dst_file = std::fs::File::create(&dest_path)
                    .map_err(|e| format!("Failed to create cached outgoing file: {e}"))?;

                std::io::copy(&mut src_file, &mut dst_file)
                    .map_err(|e| format!("Failed to copy content URI data: {e}"))?;

                let out_file = OutgoingFile::from_path_with_name(&dest_path, file_name)
                    .await
                    .map_err(|e| format!("Failed to read {}: {e}", dest_path.display()))?
                    .with_computed_sha256()
                    .await
                    .map_err(|e| format!("Failed to hash {}: {e}", dest_path.display()))?;
                collected.push(out_file);
                continue;
            }
            #[cfg(not(target_os = "android"))]
            {
                return Err(format!("Content URIs are only supported on Android: {path_str}"));
            }
        }

        let path = PathBuf::from(path_str);
        if !path.exists() {
            return Err(format!("File does not exist: {}", path.display()));
        }

        if path.is_file() {
            let out_file = OutgoingFile::from_path(&path)
                .await
                .map_err(|e| format!("Failed to read {}: {e}", path.display()))?
                .with_computed_sha256()
                .await
                .map_err(|e| format!("Failed to hash {}: {e}", path.display()))?;
            collected.push(out_file);
        } else if path.is_dir() {
            let parent_dir = path.parent().unwrap_or(&path);
            let mut dir_entries: Vec<(PathBuf, String)> = Vec::new();
            collect_dir_entries_recursive(&path, parent_dir, &mut dir_entries)
                .await
                .map_err(|e| format!("Failed to scan directory {}: {e}", path.display()))?;

            for (file_path, rel_name) in dir_entries {
                let out_file = OutgoingFile::from_path_with_name(&file_path, rel_name)
                    .await
                    .map_err(|e| format!("Failed to read {}: {e}", file_path.display()))?
                    .with_computed_sha256()
                    .await
                    .map_err(|e| format!("Failed to hash {}: {e}", file_path.display()))?;
                collected.push(out_file);
            }
        }
    }

    if collected.is_empty() {
        return Err("No files found in selected items".to_string());
    }

    Ok(collected)
}

fn collect_dir_entries_recursive<'a>(
    dir: &'a Path,
    base_prefix: &'a Path,
    out: &'a mut Vec<(PathBuf, String)>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send + 'a>> {
    Box::pin(async move {
        let mut reader = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = reader.next_entry().await? {
            let entry_path = entry.path();
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                collect_dir_entries_recursive(&entry_path, base_prefix, out).await?;
            } else if file_type.is_file() {
                let rel = entry_path
                    .strip_prefix(base_prefix)
                    .unwrap_or(&entry_path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((entry_path, rel));
            }
        }
        Ok(())
    })
}

/// Creates a temporary text message file on disk so it can be sent via standard transfer protocols.
#[tauri::command]
pub async fn create_temp_text_file(
    app: AppHandle,
    content: String,
    file_name: Option<String>,
) -> Result<String, String> {
    let base_dir = app.path().cache_dir().unwrap_or_else(|_| std::env::temp_dir());
    let dir = base_dir.join("lanflare_messages");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create temporary directory: {e}"))?;

    let name = file_name.unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!("message_{}.txt", now.format("%Y%m%d_%H%M%S"))
    });
    let file_path = dir.join(name);
    tokio::fs::write(&file_path, content.as_bytes())
        .await
        .map_err(|e| format!("Failed to write text message file: {e}"))?;

    Ok(file_path.to_string_lossy().to_string())
}

/// Registers files with the download manager for "Share via link" browser downloads.
#[tauri::command]
pub async fn offer_download_files(
    #[allow(unused)] app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
    files: Vec<String>,
) -> Result<(), String> {
    let srv_guard = state.server_handle.read().await;
    let srv = srv_guard
        .as_ref()
        .ok_or_else(|| "Server is not currently running".to_string())?;

    let mut download_files = Vec::new();
    for path_str in files {
        if path_str.starts_with("content://") {
            #[cfg(target_os = "android")]
            {
                use tauri_plugin_fs::FsExt;
                let file_name = app
                    .path()
                    .file_name(&path_str)
                    .unwrap_or_else(|| "file".to_string());
                let cache_dir = app
                    .path()
                    .cache_dir()
                    .unwrap_or_else(|_| std::env::temp_dir())
                    .join("lanflare_download_offer");
                tokio::fs::create_dir_all(&cache_dir)
                    .await
                    .map_err(|e| format!("Failed to create cache dir: {e}"))?;

                let dest_path = cache_dir.join(&file_name);
                let url = url::Url::parse(&path_str)
                    .map_err(|e| format!("Invalid content URI {path_str}: {e}"))?;
                let file_path = tauri_plugin_fs::FilePath::Url(url);
                let mut opts = tauri_plugin_fs::OpenOptions::default();
                opts.read(true);
                let mut src_file = app
                    .fs()
                    .open(file_path, opts)
                    .map_err(|e| format!("Failed to open content URI {path_str}: {e}"))?;
                let mut dst_file = std::fs::File::create(&dest_path)
                    .map_err(|e| format!("Failed to create cached file: {e}"))?;
                std::io::copy(&mut src_file, &mut dst_file)
                    .map_err(|e| format!("Failed to copy content URI data: {e}"))?;

                let meta = tokio::fs::metadata(&dest_path)
                    .await
                    .map_err(|e| format!("Failed to stat {}: {e}", dest_path.display()))?;
                let file_type = lanflare_core::client::transfer::mime_guess_simple(&file_name);
                let file_id = uuid::Uuid::new_v4().to_string();

                download_files.push(lanflare_core::server::DownloadFile {
                    dto: lanflare_proto::transfer::FileDto {
                        id: file_id,
                        file_name,
                        size: meta.len(),
                        file_type,
                        sha256: None,
                        preview: None,
                        metadata: None,
                    },
                    file_path: dest_path,
                });
                continue;
            }
            #[cfg(not(target_os = "android"))]
            {
                return Err(format!("Content URIs are only supported on Android: {path_str}"));
            }
        }

        let path = PathBuf::from(&path_str);
        if !path.exists() {
            return Err(format!("File does not exist: {}", path.display()));
        }

        if path.is_file() {
            let meta = tokio::fs::metadata(&path)
                .await
                .map_err(|e| format!("Failed to stat {}: {e}", path.display()))?;
            let file_name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("file")
                .to_string();
            let file_type = lanflare_core::client::transfer::mime_guess_simple(&file_name);
            let file_id = uuid::Uuid::new_v4().to_string();

            download_files.push(lanflare_core::server::DownloadFile {
                dto: lanflare_proto::transfer::FileDto {
                    id: file_id,
                    file_name,
                    size: meta.len(),
                    file_type,
                    sha256: None,
                    preview: None,
                    metadata: None,
                },
                file_path: path,
            });
        } else if path.is_dir() {
            let parent_dir = path.parent().unwrap_or(&path);
            let mut dir_entries = Vec::new();
            collect_dir_entries_recursive(&path, parent_dir, &mut dir_entries)
                .await
                .map_err(|e| format!("Failed to scan folder {}: {e}", path.display()))?;

            for (file_path, rel_name) in dir_entries {
                let meta = tokio::fs::metadata(&file_path)
                    .await
                    .map_err(|e| format!("Failed to stat {}: {e}", file_path.display()))?;
                let file_type = lanflare_core::client::transfer::mime_guess_simple(&rel_name);
                let file_id = uuid::Uuid::new_v4().to_string();

                download_files.push(lanflare_core::server::DownloadFile {
                    dto: lanflare_proto::transfer::FileDto {
                        id: file_id,
                        file_name: rel_name,
                        size: meta.len(),
                        file_type,
                        sha256: None,
                        preview: None,
                        metadata: None,
                    },
                    file_path,
                });
            }
        }
    }

    srv.download_manager().set_files(download_files).await;
    Ok(())
}

/// Clears currently offered download files in the download manager.
#[tauri::command]
pub async fn clear_download_files(state: State<'_, Arc<DesktopState>>) -> Result<(), String> {
    let srv_guard = state.server_handle.read().await;
    if let Some(srv) = srv_guard.as_ref() {
        srv.download_manager().clear_files().await;
    }
    Ok(())
}

/// Retrieves list of file DTOs currently offered for browser download.
#[tauri::command]
pub async fn get_download_files(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Vec<lanflare_proto::transfer::FileDto>, String> {
    let srv_guard = state.server_handle.read().await;
    if let Some(srv) = srv_guard.as_ref() {
        let dtos = srv.download_manager().get_file_dtos().await;
        Ok(dtos.into_values().collect())
    } else {
        Ok(Vec::new())
    }
}

/// Loads persistent transfer history entries from disk.
#[tauri::command]
pub async fn get_transfer_history(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Vec<serde_json::Value>, String> {
    Ok(state.load_history_current().await)
}

/// Saves persistent transfer history entries to disk.
#[tauri::command]
pub async fn save_transfer_history(
    state: State<'_, Arc<DesktopState>>,
    items: Vec<serde_json::Value>,
) -> Result<(), String> {
    state.save_history_current(&items).await.map_err(|e| format!("Failed to save history: {e}"))
}

/// Clears persistent transfer history from disk.
#[tauri::command]
pub async fn clear_transfer_history(
    state: State<'_, Arc<DesktopState>>,
) -> Result<(), String> {
    state.save_history_current(&[]).await.map_err(|e| format!("Failed to clear history: {e}"))
}

/// DTO for clipboard content (files, image, or text).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum ClipboardContentDto {
    Files(Vec<String>),
    Image {
        path: String,
        name: String,
        size: usize,
    },
    Text(String),
}

/// Reads image, file paths, or text directly from the OS system clipboard.
#[tauri::command]
pub async fn read_clipboard(
    #[allow(unused)] app: AppHandle,
) -> Result<ClipboardContentDto, String> {
    #[cfg(not(target_os = "android"))]
    {
        tokio::task::spawn_blocking(move || {
            let mut clipboard = arboard::Clipboard::new()
                .map_err(|e| format!("Failed to access clipboard: {e}"))?;

            // 1. Try reading an image
            if let Ok(img) = clipboard.get_image()
                && img.width > 0 && img.height > 0 && !img.bytes.is_empty()
            {
                let base_dir = app.path().cache_dir().unwrap_or_else(|_| std::env::temp_dir());
                let dir = base_dir.join("lanflare_clipboard");
                let _ = std::fs::create_dir_all(&dir);
                let now = chrono::Local::now();
                let file_name = format!("clipboard_{}.png", now.format("%Y%m%d_%H%M%S"));
                let path = dir.join(&file_name);

                if let Ok(file) = std::fs::File::create(&path) {
                    let mut encoder = png::Encoder::new(file, img.width as u32, img.height as u32);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    if let Ok(mut writer) = encoder.write_header()
                        && writer.write_image_data(&img.bytes).is_ok()
                    {
                        let size = std::fs::metadata(&path)
                            .map(|m| m.len() as usize)
                            .unwrap_or(img.bytes.len());
                        return Ok(ClipboardContentDto::Image {
                            path: path.to_string_lossy().to_string(),
                            name: file_name,
                            size,
                        });
                    }
                }
            }

            // 2. Try reading text or file URLs
            if let Ok(text) = clipboard.get_text() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    // Check if the text contains copied file paths / file:// URLs
                    let mut existing_files = Vec::new();
                    for line in trimmed.lines() {
                        let line = line.trim();
                        if line.is_empty() || line == "copy" || line == "cut" || line.starts_with("x-special/") {
                            continue;
                        }

                        // Try url parsing (file://...)
                        if let Ok(url) = url::Url::parse(line)
                            && url.scheme() == "file"
                            && let Ok(p) = url.to_file_path()
                            && p.exists()
                        {
                            existing_files.push(p.to_string_lossy().to_string());
                            continue;
                        }

                        // Try direct filesystem path
                        let p = std::path::Path::new(line);
                        if p.exists() {
                            existing_files.push(p.to_string_lossy().to_string());
                        }
                    }

                    if !existing_files.is_empty() {
                        return Ok(ClipboardContentDto::Files(existing_files));
                    }

                    // Plain text
                    return Ok(ClipboardContentDto::Text(trimmed.to_string()));
                }
            }

            Err("Clipboard is empty or contains unsupported content".to_string())
        })
        .await
        .map_err(|e| format!("Clipboard task failed: {e}"))?
    }
    #[cfg(target_os = "android")]
    {
        use tauri_plugin_clipboard_manager::ClipboardExt;
        if let Ok(text) = app.clipboard().read_text() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Ok(ClipboardContentDto::Text(trimmed.to_string()));
            }
        }
        Err("Clipboard is empty or contains unsupported content".to_string())
    }
}

/// Saves an RGBA image buffer (from clipboard) to a temporary PNG file.
#[tauri::command]
pub async fn save_clipboard_image(
    app: AppHandle,
    rgba: Vec<u8>,
    width: u32,
    height: u32,
) -> Result<String, String> {
    let base_dir = app.path().cache_dir().unwrap_or_else(|_| std::env::temp_dir());
    let dir = base_dir.join("lanflare_clipboard");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("Failed to create clipboard temp dir: {e}"))?;

    let now = chrono::Local::now();
    let file_name = format!("clipboard_{}.png", now.format("%Y%m%d_%H%M%S"));
    let path = dir.join(&file_name);

    let file = std::fs::File::create(&path)
        .map_err(|e| format!("Failed to create file: {e}"))?;

    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()
        .map_err(|e| format!("Failed to write PNG header: {e}"))?;
    writer.write_image_data(&rgba)
        .map_err(|e| format!("Failed to write PNG data: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

