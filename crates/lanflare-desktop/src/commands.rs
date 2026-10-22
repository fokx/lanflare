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
    PrepareUploadDecision, ServerTransferEvent,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_count: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_path: Option<Vec<String>>,
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
            hop_count: p.mesh_hop_count(),
            route_path: p.mesh_route_path(),
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
    pub port_conflict: bool,
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
    let fingerprint = state.identity.read().await.fingerprint.clone();

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

    let port_conflict = state.port_conflict.load(std::sync::atomic::Ordering::Relaxed);

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
        port_conflict,
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
    let mut restart_server = false;

    if let Some(alias) = payload.alias {
        cfg.alias = alias;
    }
    if let Some(port) = payload.port {
        if cfg.port != port {
            restart_server = true;
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

    if restart_server {
        let mut srv = state.server_handle.write().await;
        if let Some(h) = srv.take() {
            h.stop();
        }
        drop(srv);
        let _ = start_receiver_server(app.clone(), state.clone()).await;
    }

    if restart_discovery {
        let _ = start_discovery(app, state.clone()).await;
    }

    get_node_status(state).await
}

/// Regenerates the self-signed TLS certificate and fingerprint, persists it, restarts server & discovery.
#[tauri::command]
pub async fn regenerate_identity(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<NodeStatusDto, String> {
    let new_identity = lanflare_core::crypto::cert::generate_self_signed()
        .map_err(|e| format!("Failed to generate new TLS certificate: {e}"))?;

    let id_file = state.identity_file.read().await.clone();
    lanflare_core::crypto::cert::save_identity(&id_file, &new_identity)
        .map_err(|e| format!("Failed to save new identity: {e}"))?;

    *state.identity.write().await = new_identity;

    // Restart server handle to bind with new certificate
    {
        let mut srv = state.server_handle.write().await;
        if let Some(h) = srv.take() {
            h.stop();
        }
    }
    let _ = start_receiver_server(app.clone(), state.clone()).await;

    // Restart discovery engines and broadcast new fingerprint
    let _ = start_discovery(app.clone(), state.clone()).await;
    let _ = announce_presence(app.clone(), state.clone()).await;

    get_node_status(state).await
}

/// Generates a friendly, two-word random device alias (e.g. "Golden Mango").
#[tauri::command]
pub async fn generate_random_alias() -> Result<String, String> {
    Ok(lanflare_core::config::generate_random_alias())
}

/// Lists all active peers currently in the registry, suppressing self and deduplicating by fingerprint.
#[tauri::command]
pub async fn get_discovered_peers(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Vec<DiscoveredPeerDto>, String> {
    let own_fp = lanflare_core::discovery::registry::canonicalize_fingerprint(&state.identity.read().await.fingerprint);
    let peers = state.registry.get_all().await;
    let mut unique_by_fp = std::collections::HashMap::new();

    for p in peers {
        if p.fingerprint.eq_ignore_ascii_case(&own_fp) {
            continue;
        }
        let canon = lanflare_core::discovery::registry::canonicalize_fingerprint(&p.fingerprint);
        if canon.is_empty() {
            continue;
        }
        unique_by_fp
            .entry(canon)
            .and_modify(|existing: &mut lanflare_core::discovery::registry::DiscoveredPeer| {
                if p.last_seen_ms > existing.last_seen_ms {
                    *existing = p.clone();
                }
            })
            .or_insert(p);
    }

    let mut dtos: Vec<DiscoveredPeerDto> = unique_by_fp
        .into_values()
        .map(|p| DiscoveredPeerDto::from(&p))
        .collect();
    dtos.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then_with(|| a.alias.cmp(&b.alias)));
    Ok(dtos)
}

/// Manually removes a peer from the discovery registry.
#[tauri::command]
pub async fn remove_discovered_peer(
    fingerprint: String,
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<bool, String> {
    let removed = state.registry.remove(&fingerprint).await.is_some();
    if removed {
        let _ = app.emit("lanflare://peers-updated", ());
    }
    Ok(removed)
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
    let fp = state.identity.read().await.fingerprint.clone();

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
    let identity_subnet = state.identity.read().await.clone();
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
                    let app_cb = app_subnet.clone();
                    async move {
                        lanflare_core::discovery::scanner::scan_subnet_with_callback(
                            iface.address,
                            port_subnet,
                            &ident,
                            &reg,
                            move |_| {
                                let _ = app_cb.emit("lanflare://peers-updated", ());
                            },
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

    // Actively verify existing registered peers: ping each peer via HTTP info; evict if unreachable or cert changed
    let existing_peers = state.registry.get_all().await;
    let own_fp = lanflare_core::discovery::registry::canonicalize_fingerprint(&state.identity.read().await.fingerprint);
    let app_verify = app.clone();
    let reg_verify = Arc::clone(&state.registry);
    let identity_verify = state.identity.read().await.clone();

    tokio::spawn(async move {
        let client_cfg = lanflare_core::client::LsClientConfig {
            timeout: std::time::Duration::from_millis(1500),
            connect_timeout: std::time::Duration::from_millis(1000),
            tls_identity: Some(identity_verify),
            target_fingerprint: None,
        };
        if let Ok(client) = lanflare_core::client::LsClient::new(client_cfg) {
            let mut evicted_any = false;
            for peer in existing_peers {
                if peer.fingerprint.eq_ignore_ascii_case(&own_fp) {
                    reg_verify.remove(&peer.fingerprint).await;
                    evicted_any = true;
                    continue;
                }
                if let Some(base_url) = peer.primary_base_url() {
                    match client.get_info(&base_url).await {
                        Ok(info) => {
                            let info_fp = lanflare_core::discovery::registry::canonicalize_fingerprint(&info.fingerprint);
                            if !info_fp.eq_ignore_ascii_case(&peer.fingerprint) {
                                reg_verify.remove(&peer.fingerprint).await;
                                evicted_any = true;
                            }
                        }
                        Err(_) => {
                            reg_verify.remove(&peer.fingerprint).await;
                            evicted_any = true;
                        }
                    }
                }
            }
            if evicted_any {
                let _ = app_verify.emit("lanflare://peers-updated", ());
            }
        }
    });

    // Also run a subnet scan sweep in background on refresh
    let registry_subnet = Arc::clone(&state.registry);
    let identity_subnet = state.identity.read().await.clone();
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
                    let app_cb = app_subnet.clone();
                    async move {
                        lanflare_core::discovery::scanner::scan_subnet_with_callback(
                            iface.address,
                            port_subnet,
                            &ident,
                            &reg,
                            move |_| {
                                let _ = app_cb.emit("lanflare://peers-updated", ());
                            },
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

/// Internal helper attempting to bind and start the receiver server.
/// Returns Ok(bound_port) or Err((is_port_conflict, error_message)).
pub async fn try_start_receiver_server_internal(
    app: &AppHandle,
    state: &Arc<DesktopState>,
) -> Result<u16, (bool, String)> {
    let mut srv = state.server_handle.write().await;
    if let Some(ref h) = *srv {
        state.port_conflict.store(false, std::sync::atomic::Ordering::SeqCst);
        return Ok(h.bound_port());
    }

    let (event_tx, mut event_rx) = mpsc::channel::<ServerTransferEvent>(100);
    let tls_id = state.identity.read().await.clone();

    let start_res = lanflare_core::server::start_server_ext(
        state.config.clone(),
        Some(tls_id),
        Arc::clone(&state.registry),
        Some(event_tx),
        None,
        Some(Arc::clone(&state.dufs_manager)),
    )
    .await;

    let handle = match start_res {
        Ok(h) => h,
        Err(e) => {
            let err_str = e.to_string();
            let is_conflict = if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                io_err.kind() == std::io::ErrorKind::AddrInUse
            } else {
                let lower = err_str.to_lowercase();
                lower.contains("address already in use")
                    || lower.contains("os error 98")
                    || lower.contains("os error 10048")
                    || lower.contains("os error 48")
            };
            return Err((is_conflict, err_str));
        }
    };

    let port = handle.bound_port();
    *srv = Some(handle);
    state.port_conflict.store(false, std::sync::atomic::Ordering::SeqCst);

    // Event pump for incoming transfer sessions
    let state_clone = Arc::clone(state);
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
                    state_clone.registry.handle_http_register(ip, &info).await;
                    let _ = app_clone.emit("lanflare://peers-updated", ());
                }
                ServerTransferEvent::PrepareUpload {
                    session_id,
                    sender_ip,
                    sender_info,
                    files,
                    decision_tx,
                    ..
                } => {
                    state_clone.registry.handle_http_register(sender_ip, &sender_info).await;
                    let _ = app_clone.emit("lanflare://peers-updated", ());

                    let mut file_infos = Vec::new();
                    let mut total_bytes = 0u64;

                    for (id, f) in &files {
                        total_bytes += f.size;
                        file_infos.push(IncomingFileInfo {
                            id: id.clone(),
                            file_name: f.file_name.clone(),
                            size: f.size,
                            file_type: f.file_type.clone(),
                            path: None,
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

/// Starts the Hyper HTTP/HTTPS receiver server daemon, or begins auto-retry if port is occupied.
#[tauri::command]
pub async fn start_receiver_server(
    app: AppHandle,
    state: State<'_, Arc<DesktopState>>,
) -> Result<u16, String> {
    let state_arc = state.inner().clone();
    match try_start_receiver_server_internal(&app, &state_arc).await {
        Ok(port) => {
            state.port_conflict.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = app.emit("lanflare://port-conflict", false);
            let _ = app.emit("lanflare://server-started", port);
            Ok(port)
        }
        Err((is_conflict, err)) => {
            if is_conflict {
                state.port_conflict.store(true, std::sync::atomic::Ordering::SeqCst);
                let _ = app.emit("lanflare://port-conflict", true);

                if !state.retry_running.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    let app_retry = app.clone();
                    let state_retry = Arc::clone(&state_arc);
                    tokio::spawn(async move {
                        tracing::warn!("Port conflict detected. Starting background retry watcher...");
                        loop {
                            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                            {
                                let srv = state_retry.server_handle.read().await;
                                if srv.is_some() {
                                    state_retry.retry_running.store(false, std::sync::atomic::Ordering::SeqCst);
                                    break;
                                }
                            }
                            match try_start_receiver_server_internal(&app_retry, &state_retry).await {
                                Ok(port) => {
                                    tracing::info!("Port {port} became available! Receiver server started successfully.");
                                    state_retry.port_conflict.store(false, std::sync::atomic::Ordering::SeqCst);
                                    state_retry.retry_running.store(false, std::sync::atomic::Ordering::SeqCst);
                                    let _ = app_retry.emit("lanflare://port-conflict", false);
                                    let _ = app_retry.emit("lanflare://server-started", port);
                                    let _ = app_retry.emit("lanflare://peers-updated", ());
                                    break;
                                }
                                Err((conflict, _)) if conflict => {
                                    // Still occupied, continue polling
                                }
                                Err((_, other_err)) => {
                                    tracing::error!("Unexpected error during port retry: {other_err}");
                                    state_retry.retry_running.store(false, std::sync::atomic::Ordering::SeqCst);
                                    break;
                                }
                            }
                        }
                    });
                }

                Err(format!("Port is occupied by another LocalSend or LanFlare instance. Auto-retry is active: {err}"))
            } else {
                Err(format!("Failed to start server: {err}"))
            }
        }
    }
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
                    let probe_ident = state.identity.read().await.clone();
                    let probe_config = LsClientConfig {
                        timeout: Duration::from_secs(3),
                        connect_timeout: Duration::from_secs(2),
                        tls_identity: Some(probe_ident),
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
                let ident = state.identity.read().await.clone();
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

    let own_identity = state.identity.read().await.clone();

    // 3. Create client with pinned certificate verifier
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(5),
        tls_identity: Some(own_identity.clone()),
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
        fingerprint: own_identity.fingerprint.clone(),
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
            path: Some(f.path.to_string_lossy().to_string()),
        })
        .collect();

    let _ = app.emit(
        "lanflare://outgoing-transfer-initiated",
        serde_json::json!({
            "sessionId": session_uuid,
            "targetAlias": target_peer.alias,
            "targetEndpoint": base_url,
            "targetFingerprint": target_peer.fingerprint,
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

/// Open a file using system's default application.
#[tauri::command]
pub async fn open_path_in_system(#[allow(unused)] app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("File does not exist: {path}"));
    }

    #[cfg(desktop)]
    {
        if let Ok(()) = tauri_plugin_opener::open_path(&path, None::<&str>) {
            return Ok(());
        }

        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", "", &path])
                .status()
                .map_err(|e| format!("Failed to run cmd start: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }

        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open")
                .arg(&path)
                .status()
                .map_err(|e| format!("Failed to run open: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }

        #[cfg(target_os = "linux")]
        {
            let status = std::process::Command::new("xdg-open")
                .arg(&path)
                .status()
                .map_err(|e| format!("Failed to run xdg-open: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }

        Err("Failed to open file with default application".to_string())
    }

    #[cfg(target_os = "android")]
    {
        if let Some(window) = app.get_webview_window("main") {
            let escaped = serde_json::to_string(&path).unwrap_or_else(|_| format!("\"{}\"", path));
            let script = format!("if (window.AndroidStorage && window.AndroidStorage.openFile) {{ window.AndroidStorage.openFile({}); }}", escaped);
            let _ = window.eval(&script);
            return Ok(());
        }
        Err("No webview window found".to_string())
    }

    #[cfg(not(any(desktop, target_os = "android")))]
    {
        Err("Direct file opening not supported on this platform".to_string())
    }
}

/// Reveal a file or folder in the system's file manager, marking the file as selected.
#[tauri::command]
pub async fn reveal_in_folder(#[allow(unused)] app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err(format!("Path does not exist: {path}"));
    }

    #[cfg(desktop)]
    {
        if let Ok(()) = tauri_plugin_opener::reveal_item_in_dir(&path) {
            return Ok(());
        }

        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("explorer")
                .arg(format!("/select,\"{}\"", p.display()))
                .status()
                .map_err(|e| format!("Failed to launch explorer: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }

        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open")
                .args(["-R", &path])
                .status()
                .map_err(|e| format!("Failed to launch macOS finder: {e}"))?;
            if status.success() {
                return Ok(());
            }
        }

        #[cfg(target_os = "linux")]
        {
            for fm in &["nautilus", "dolphin", "nemo", "caja", "thunar"] {
                if let Ok(mut child) = std::process::Command::new(fm)
                    .args(["--select", &path])
                    .spawn()
                {
                    let _ = child.wait();
                    return Ok(());
                }
            }

            if let Some(parent) = p.parent() {
                let _ = std::process::Command::new("xdg-open")
                    .arg(parent)
                    .spawn();
                return Ok(());
            }
        }

        Err("Failed to reveal item in file manager".to_string())
    }

    #[cfg(target_os = "android")]
    {
        if let Some(window) = app.get_webview_window("main") {
            let escaped = serde_json::to_string(&path).unwrap_or_else(|_| format!("\"{}\"", path));
            let script = format!("if (window.AndroidStorage && window.AndroidStorage.openFolder) {{ window.AndroidStorage.openFolder({}); }}", escaped);
            let _ = window.eval(&script);
            return Ok(());
        }
        Err("No webview window found".to_string())
    }

    #[cfg(not(any(desktop, target_os = "android")))]
    {
        Err("Revealing item not supported on this platform".to_string())
    }
}

// ----------------------------------------------------------------------------
// Dufs-alike Folder Sharing & Directory Navigation Commands
// ----------------------------------------------------------------------------

/// Common directory preset item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickDirDto {
    pub label: String,
    pub path: String,
    pub exists: bool,
}

/// Directory entry item returned by directory browser.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

/// Directory listing result for folder navigation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirListingDto {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirEntryDto>,
    pub is_android: bool,
    pub common_paths: Vec<QuickDirDto>,
}

/// Retrieves list of common directories on the platform.
#[tauri::command]
pub async fn get_common_directories() -> Result<Vec<QuickDirDto>, String> {
    let mut dirs_list = Vec::new();

    #[cfg(target_os = "android")]
    {
        let standard_android_dirs = [
            ("Downloads", "/storage/emulated/0/Download"),
            ("Documents", "/storage/emulated/0/Documents"),
            ("DCIM (Camera)", "/storage/emulated/0/DCIM"),
            ("Pictures", "/storage/emulated/0/Pictures"),
            ("Music", "/storage/emulated/0/Music"),
            ("Movies", "/storage/emulated/0/Movies"),
            ("Internal Storage", "/storage/emulated/0"),
        ];

        for (label, p) in standard_android_dirs {
            let path_buf = PathBuf::from(p);
            let exists = path_buf.is_dir();
            dirs_list.push(QuickDirDto {
                label: label.to_string(),
                path: p.to_string(),
                exists,
            });
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        if let Some(p) = dirs::download_dir() {
            dirs_list.push(QuickDirDto {
                label: "Downloads".to_string(),
                path: p.to_string_lossy().to_string(),
                exists: p.is_dir(),
            });
        }
        if let Some(p) = dirs::document_dir() {
            dirs_list.push(QuickDirDto {
                label: "Documents".to_string(),
                path: p.to_string_lossy().to_string(),
                exists: p.is_dir(),
            });
        }
        if let Some(p) = dirs::picture_dir() {
            dirs_list.push(QuickDirDto {
                label: "Pictures".to_string(),
                path: p.to_string_lossy().to_string(),
                exists: p.is_dir(),
            });
        }
        if let Some(p) = dirs::home_dir() {
            dirs_list.push(QuickDirDto {
                label: "Home".to_string(),
                path: p.to_string_lossy().to_string(),
                exists: p.is_dir(),
            });
        }
    }

    Ok(dirs_list)
}

/// Lists directories and files at the given path for custom folder browsing (essential for Android).
#[tauri::command]
pub async fn list_directory(
    path: Option<String>,
) -> Result<DirListingDto, String> {
    let is_android = cfg!(target_os = "android");

    let target_path = match path.as_deref() {
        Some(p) if !p.trim().is_empty() => PathBuf::from(p.trim()),
        _ => {
            #[cfg(target_os = "android")]
            {
                let dl = PathBuf::from("/storage/emulated/0/Download");
                if dl.is_dir() {
                    dl
                } else {
                    PathBuf::from("/storage/emulated/0")
                }
            }
            #[cfg(not(target_os = "android"))]
            {
                dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
            }
        }
    };

    if !target_path.is_dir() {
        return Err(format!("Directory does not exist or is not accessible: {}", target_path.display()));
    }

    let parent_path = target_path.parent().map(|p| p.to_string_lossy().to_string());

    let mut entries = Vec::new();
    let mut reader = tokio::fs::read_dir(&target_path)
        .await
        .map_err(|e| format!("Failed to read directory {}: {e}", target_path.display()))?;

    while let Ok(Some(entry)) = reader.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }

        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let modified = meta
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        entries.push(DirEntryDto {
            name,
            path: entry.path().to_string_lossy().to_string(),
            is_dir,
            size,
            modified,
        });
    }

    // Sort: directories first, then alphabetical by name
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    let common_paths = get_common_directories().await.unwrap_or_default();

    Ok(DirListingDto {
        current_path: target_path.to_string_lossy().to_string(),
        parent_path,
        entries,
        is_android,
        common_paths,
    })
}

fn default_true() -> bool {
    true
}

/// DTO for requesting to start a Dufs folder share.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartDufsShareDto {
    pub folder_path: String,
    pub allow_upload: bool,
    pub allow_delete: bool,
    #[serde(default = "default_true")]
    pub require_pin: bool,
    pub pin: Option<String>,
}

/// Status of the active Dufs folder server.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DufsShareStatusDto {
    pub is_active: bool,
    pub folder_path: String,
    pub folder_name: String,
    pub port: u16,
    pub urls: Vec<String>,
    pub allow_upload: bool,
    pub allow_delete: bool,
    pub auth_pin: Option<String>,
    pub file_count: usize,
    pub total_size: u64,
}

/// Starts serving an entire selected folder as a Dufs file server.
#[tauri::command]
pub async fn start_dufs_share(
    state: State<'_, Arc<DesktopState>>,
    options: StartDufsShareDto,
) -> Result<DufsShareStatusDto, String> {
    let folder = PathBuf::from(&options.folder_path);
    if !folder.is_dir() {
        return Err(format!("Selected path is not a valid directory: {}", folder.display()));
    }

    let folder_name = folder
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Shared Folder")
        .to_string();

    let mut file_count = 0;
    let mut total_size = 0u64;
    if let Ok(mut reader) = tokio::fs::read_dir(&folder).await {
        while let Ok(Some(entry)) = reader.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                if meta.is_file() {
                    file_count += 1;
                    total_size += meta.len();
                } else if meta.is_dir() {
                    file_count += 1;
                }
            }
        }
    }

    let pin = if options.require_pin {
        let p = match options.pin {
            Some(ref s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => state.config.read().await.share_link_pin.clone(),
        };
        if p.trim().is_empty() {
            Some(lanflare_core::config::generate_share_link_pin())
        } else {
            Some(p.trim().to_string())
        }
    } else {
        None
    };

    let title = {
        let cfg = state.config.read().await;
        format!("{} ({})", folder_name, cfg.alias)
    };

    let dufs_config = lanflare_core::server::DufsConfig {
        root_path: folder.clone(),
        path_prefix: String::new(),
        allow_upload: options.allow_upload,
        allow_delete: options.allow_delete,
        auth_pin: pin.clone(),
        title,
    };

    state.dufs_manager.set_config(dufs_config).await;

    let port = {
        let guard = state.server_handle.read().await;
        if let Some(ref srv) = *guard {
            srv.bound_port()
        } else {
            state.config.read().await.port
        }
    };

    let auth_prefix = match &pin {
        Some(p) if !p.is_empty() => format!(":{p}@"),
        _ => String::new(),
    };

    let scheme = "https";
    let filter = InterfaceFilter::default();
    let mut urls = Vec::new();
    if let Ok(interfaces) = local_interfaces(&filter) {
        for v4 in interfaces.v4 {
            let addr_str = v4.address.to_string();
            if addr_str != "127.0.0.1" {
                urls.push(format!("{scheme}://{}{}:{port}/", auth_prefix, addr_str));
            }
        }
    }
    if urls.is_empty() {
        urls.push(format!("{scheme}://{}127.0.0.1:{port}/", auth_prefix));
    }

    Ok(DufsShareStatusDto {
        is_active: true,
        folder_path: folder.to_string_lossy().to_string(),
        folder_name,
        port,
        urls,
        allow_upload: options.allow_upload,
        allow_delete: options.allow_delete,
        auth_pin: pin,
        file_count,
        total_size,
    })
}

/// Stops serving the folder.
#[tauri::command]
pub async fn stop_dufs_share(
    state: State<'_, Arc<DesktopState>>,
) -> Result<(), String> {
    state.dufs_manager.clear_config().await;
    Ok(())
}

/// Retrieves the current status of the Dufs folder share.
#[tauri::command]
pub async fn get_dufs_share_status(
    state: State<'_, Arc<DesktopState>>,
) -> Result<Option<DufsShareStatusDto>, String> {
    let cfg = match state.dufs_manager.get_config().await {
        Some(c) => c,
        None => return Ok(None),
    };

    let folder_name = cfg
        .root_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Shared Folder")
        .to_string();

    let mut file_count = 0;
    let mut total_size = 0u64;
    if let Ok(mut reader) = tokio::fs::read_dir(&cfg.root_path).await {
        while let Ok(Some(entry)) = reader.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                if meta.is_file() {
                    file_count += 1;
                    total_size += meta.len();
                } else if meta.is_dir() {
                    file_count += 1;
                }
            }
        }
    }

    let port = {
        let guard = state.server_handle.read().await;
        if let Some(ref srv) = *guard {
            srv.bound_port()
        } else {
            state.config.read().await.port
        }
    };

    let auth_prefix = match &cfg.auth_pin {
        Some(p) if !p.is_empty() => format!(":{p}@"),
        _ => String::new(),
    };

    let scheme = "https";
    let filter = InterfaceFilter::default();
    let mut urls = Vec::new();
    if let Ok(interfaces) = local_interfaces(&filter) {
        for v4 in interfaces.v4 {
            let addr_str = v4.address.to_string();
            if addr_str != "127.0.0.1" {
                urls.push(format!("{scheme}://{}{}:{port}/", auth_prefix, addr_str));
            }
        }
    }
    if urls.is_empty() {
        urls.push(format!("{scheme}://{}127.0.0.1:{port}/", auth_prefix));
    }

    Ok(Some(DufsShareStatusDto {
        is_active: true,
        folder_path: cfg.root_path.to_string_lossy().to_string(),
        folder_name,
        port,
        urls,
        allow_upload: cfg.allow_upload,
        allow_delete: cfg.allow_delete,
        auth_pin: cfg.auth_pin,
        file_count,
        total_size,
    }))
}

