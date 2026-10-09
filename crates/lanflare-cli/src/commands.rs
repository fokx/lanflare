//! CLI command implementations.

use anyhow::Context;
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::TlsIdentity;
use lanflare_core::discovery::interface::{local_interfaces, InterfaceFilter};
use lanflare_core::discovery::mdns::{start_mdns_engine, MdnsConfig, MdnsDiscoveryEvent, MdnsNodeInfo};
use lanflare_core::discovery::multicast::{
    start_multicast_engine, MulticastConfig, MulticastDiscoveryEvent, MulticastNodeInfo,
};
use lanflare_core::discovery::registry::{DiscoveredPeer, PeerRegistry};
use lanflare_proto::discovery::{DeviceType, ProtocolType};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

use crate::tui::render_peer_table;

/// Handles the `lanflare status` command.
///
/// Prints local node configuration, identity (certificate fingerprint), and available network interfaces.
pub async fn status(config: &AppConfig, identity: &TlsIdentity) -> anyhow::Result<()> {
    println!("LanFlare Node Status:");
    println!("  Alias:       {}", config.alias);
    println!("  Port:        {}", config.port);
    println!("  Download Dir: {}", config.download_dir.display());
    println!("  Auto Accept: {}", config.auto_accept);
    println!(
        "  PIN Protect: {}",
        if config.pin.is_some() { "Enabled" } else { "Disabled" }
    );
    println!("  Fingerprint: {}", identity.fingerprint);
    println!();

    println!("Local Network Interfaces:");
    let filter = InterfaceFilter::default();
    match local_interfaces(&filter) {
        Ok(interfaces) => {
            if interfaces.v4.is_empty() && interfaces.v6.is_empty() {
                println!("  (No active non-loopback network interfaces found)");
            } else {
                for iface in &interfaces.v4 {
                    println!("  - IPv4: {} ({})", iface.address, iface.name);
                }
                for iface in &interfaces.v6 {
                    println!("  - IPv6: index {} ({})", iface.index, iface.name);
                }
            }
        }
        Err(err) => {
            println!("  Failed to enumerate network interfaces: {err}");
        }
    }

    Ok(())
}

/// Options for the `lanflare scan` command.
#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Scan timeout in seconds.
    pub duration_secs: u64,
    /// Whether to enable UDP Multicast discovery.
    pub multicast: bool,
    /// Whether to enable mDNS discovery.
    pub mdns: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            duration_secs: 3,
            multicast: true,
            mdns: true,
        }
    }
}

/// Handles the `lanflare scan` command.
///
/// Spins up dual discovery engines (Multicast UDP and mDNS), listens for announcements,
/// aggregates them in `PeerRegistry`, and displays a formatted table of active peers.
pub async fn scan(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &ScanOptions,
) -> anyhow::Result<Vec<DiscoveredPeer>> {
    println!(
        "Scanning local network for LanFlare / LocalSend peers (timeout: {}s)...",
        options.duration_secs
    );

    let registry = PeerRegistry::new();

    // 1. Setup UDP multicast discovery if enabled
    let (mc_stop_tx, mc_stop_rx) = oneshot::channel();
    let (mc_event_tx, mut mc_event_rx) = mpsc::channel::<MulticastDiscoveryEvent>(100);

    let mc_handle = if options.multicast {
        let mc_config = MulticastConfig {
            port: config.port,
            node_info: MulticastNodeInfo {
                alias: config.alias.clone(),
                version: "2.0".to_string(),
                device_model: Some("LanFlare Node".to_string()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: identity.fingerprint.clone(),
                port: config.port,
                protocol: ProtocolType::Https,
                download: false,
            },
            ..MulticastConfig::default()
        };

        match start_multicast_engine(mc_config, mc_event_tx, mc_stop_rx).await {
            Ok(handle) => {
                // Send an announcement burst so peers respond/discover us
                let h_clone = handle.clone();
                tokio::spawn(async move {
                    h_clone.announce_burst().await;
                });
                Some(handle)
            }
            Err(e) => {
                tracing::warn!("Failed to start multicast engine: {e}");
                None
            }
        }
    } else {
        None
    };

    // 2. Setup mDNS discovery if enabled
    let (mdns_stop_tx, mdns_stop_rx) = oneshot::channel();
    let (mdns_event_tx, mut mdns_event_rx) = mpsc::channel::<MdnsDiscoveryEvent>(100);

    let mdns_handle = if options.mdns {
        let mdns_config = MdnsConfig {
            node_info: MdnsNodeInfo {
                alias: config.alias.clone(),
                instance_name: String::new(),
                version: "2.0".to_string(),
                device_model: Some("LanFlare Node".to_string()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: identity.fingerprint.clone(),
                port: config.port,
                protocol: ProtocolType::Https,
                download: false,
            },
            enable_responder: true,
            enable_browser: true,
            ..MdnsConfig::default()
        };

        match start_mdns_engine(mdns_config, mdns_event_tx, mdns_stop_rx).await {
            Ok(handle) => Some(handle),
            Err(e) => {
                tracing::warn!("Failed to start mDNS engine: {e}");
                None
            }
        }
    } else {
        None
    };

    // 3. Collector loop for discovery events
    let registry_clone = registry.clone();
    let collector_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = mc_event_rx.recv() => {
                    if let MulticastDiscoveryEvent::PeerDiscovered { remote_ip, scope_id, message } = event {
                        registry_clone.handle_multicast(remote_ip, scope_id, &message).await;
                    }
                }
                Some(event) = mdns_event_rx.recv() => {
                    match event {
                        MdnsDiscoveryEvent::PeerDiscovered(peer) => {
                            registry_clone.handle_mdns(&peer).await;
                        }
                        MdnsDiscoveryEvent::PeerRemoved { fullname, .. } => {
                            // Extract instance name or remove
                            tracing::debug!("mDNS service removed: {fullname}");
                        }
                    }
                }
                else => break,
            }
        }
    });

    // Wait for the scan duration
    tokio::time::sleep(Duration::from_secs(options.duration_secs)).await;

    // Shutdown discovery engines
    let _ = mc_stop_tx.send(());
    let _ = mdns_stop_tx.send(());

    if let Some(h) = mc_handle {
        h.stop().await;
    }
    if let Some(h) = mdns_handle {
        h.stop().await;
    }

    collector_task.abort();

    // 4. Retrieve and print active peers
    let peers = registry.get_all().await;
    print!("{}", render_peer_table(&peers));

    Ok(peers)
}

/// Options for the `lanflare send` command.
#[derive(Clone, Debug)]
pub struct SendOptions {
    pub files: Vec<PathBuf>,
    pub to: Option<String>,
    pub pin: Option<String>,
}

/// Handles the `lanflare send` command.
pub async fn send(
    _config: &AppConfig,
    _identity: &TlsIdentity,
    options: &SendOptions,
) -> anyhow::Result<()> {
    if options.files.is_empty() {
        anyhow::bail!("No files specified to send");
    }

    for f in &options.files {
        if !f.exists() {
            anyhow::bail!("File not found: {}", f.display());
        }
    }

    println!(
        "Preparing to send {} file(s) to target: {:?}",
        options.files.len(),
        options.to.as_deref().unwrap_or("(interactive picker)")
    );
    // Target transfer orchestrator execution will be expanded in Phase 4.2
    Ok(())
}

/// Options for the `lanflare receive` command.
#[derive(Clone, Debug)]
pub struct ReceiveOptions {
    pub dir: Option<PathBuf>,
    pub port: Option<u16>,
    pub auto_accept: bool,
    pub pin: Option<String>,
}

/// Handles the `lanflare receive` command.
pub async fn receive(
    config: &AppConfig,
    _identity: &TlsIdentity,
    options: &ReceiveOptions,
) -> anyhow::Result<()> {
    let dest_dir = options
        .dir
        .as_ref()
        .unwrap_or(&config.download_dir);

    tokio::fs::create_dir_all(dest_dir)
        .await
        .with_context(|| format!("Failed to create download directory: {}", dest_dir.display()))?;

    let port = options.port.unwrap_or(config.port);
    println!("Listening for incoming files on port {port}...");
    println!("Saving files to: {}", dest_dir.display());
    // Daemon server loop will be expanded in Phase 4.2
    Ok(())
}
