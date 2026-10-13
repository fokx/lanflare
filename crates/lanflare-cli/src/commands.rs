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
    /// Force active /24 subnet IP scan.
    pub subnet: bool,
    /// Fall back to /24 subnet scan if multicast/mDNS yield 0 peers.
    pub fallback_subnet: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            duration_secs: 3,
            multicast: true,
            mdns: true,
            subnet: false,
            fallback_subnet: true,
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

    // 4. Retrieve active peers
    let mut peers = registry.get_all().await;

    // 5. If subnet scan requested OR (fallback enabled and no peers found via multicast/mDNS)
    if options.subnet || (options.fallback_subnet && peers.is_empty()) {
        let filter = InterfaceFilter::default();
        if let Ok(interfaces) = local_interfaces(&filter) {
            for iface in interfaces.v4 {
                // Skip loopback unless allowed
                if iface.address.is_loopback() {
                    continue;
                }
                println!(
                    "Probing subnet {}/24 via {}...",
                    iface.address, iface.name
                );
                let _ = lanflare_core::discovery::scanner::scan_subnet(
                    iface.address,
                    config.port,
                    identity,
                    &registry,
                )
                .await;
            }
        }
        peers = registry.get_all().await;
    }

    print!("{}", render_peer_table(&peers));

    Ok(peers)
}

use std::io::Write;
use std::net::IpAddr;
use lanflare_core::client::transfer::{
    OutgoingFile, TransferOptions, TransferOrchestrator, TransferProgress,
};
use lanflare_core::client::{LsClient, LsClientConfig};
use lanflare_core::server::{start_server, PrepareUploadDecision, ServerTransferEvent};
use lanflare_proto::discovery::RegisterDtoV2;

use crate::tui::{shorten_fingerprint, TransferProgressTracker};

/// Prompt user in terminal synchronously or read from reader.
pub fn prompt_user_input(prompt: &str) -> anyhow::Result<String> {
    print!("{prompt}");
    std::io::stdout().flush().context("Failed to flush stdout")?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read user input from stdin")?;
    Ok(input.trim().to_string())
}

/// Resolves target peer from query string (IP, alias, or fingerprint) against registry.
pub fn match_target_peer<'a>(
    query: &str,
    peers: &'a [DiscoveredPeer],
) -> Option<&'a DiscoveredPeer> {
    let q = query.trim();
    if q.is_empty() {
        return None;
    }

    // 1. Exact canonical fingerprint match (or prefix)
    let q_upper = q.to_ascii_uppercase();
    if let Some(p) = peers.iter().find(|p| p.fingerprint == q_upper || p.fingerprint.starts_with(&q_upper)) {
        return Some(p);
    }

    // 2. Exact IP match
    if let Ok(ip) = q.parse::<IpAddr>()
        && let Some(p) = peers.iter().find(|p| p.endpoints.iter().any(|ep| ep.ip == ip))
    {
        return Some(p);
    }

    // 3. Exact alias match (case-insensitive)
    if let Some(p) = peers.iter().find(|p| p.alias.eq_ignore_ascii_case(q)) {
        return Some(p);
    }

    // 4. Substring alias match
    peers.iter().find(|p| p.alias.to_ascii_lowercase().contains(&q.to_ascii_lowercase()))
}

/// Discovers peers and presents an interactive selection if multiple are found.
pub async fn discover_or_select_target(
    config: &AppConfig,
    identity: &TlsIdentity,
    query: Option<&str>,
) -> anyhow::Result<DiscoveredPeer> {
    if let Some(q) = query {
        // Fast-path: Check if user provided direct IP address (e.g. 192.168.1.50)
        let trimmed = q.trim();
        if let Ok(ip) = trimmed.parse::<IpAddr>() {
            println!("Probing direct IP address {ip}...");
            let probe_config = LsClientConfig {
                timeout: Duration::from_secs(3),
                connect_timeout: Duration::from_secs(2),
                tls_identity: Some(identity.clone()),
                target_fingerprint: None,
            };
            if let Ok(probe_client) = LsClient::new(probe_config) {
                for proto in [ProtocolType::Https, ProtocolType::Http] {
                    let proto_str = match proto {
                        ProtocolType::Https => "https",
                        ProtocolType::Http => "http",
                    };
                    let base_url = match ip {
                        IpAddr::V4(v4) => format!("{proto_str}://{v4}:{}", config.port),
                        IpAddr::V6(v6) => format!("{proto_str}://[{v6}]:{}", config.port),
                    };
                    if let Ok(info) = probe_client.get_info(&base_url).await {
                        let peer_port = info.port.unwrap_or(config.port);
                        let peer_proto = info.protocol.unwrap_or(proto);
                        let ep = lanflare_core::discovery::registry::PeerEndpoint::new(
                            ip,
                            None,
                            lanflare_core::discovery::DiscoverySource::ManualScan,
                            0,
                        );
                        return Ok(DiscoveredPeer {
                            fingerprint: info.fingerprint,
                            alias: info.alias,
                            version: info.version,
                            device_model: info.device_model,
                            device_type: info.device_type,
                            port: peer_port,
                            protocol: peer_proto,
                            download: info.download,
                            endpoints: vec![ep],
                            discovery_sources: std::collections::HashSet::new(),
                            first_seen_ms: 0,
                            last_seen_ms: 0,
                            metadata: std::collections::HashMap::new(),
                        });
                    }
                }
            }

            // Fallback unprobed direct peer
            let ep = lanflare_core::discovery::registry::PeerEndpoint::new(
                ip,
                None,
                lanflare_core::discovery::DiscoverySource::ManualScan,
                0,
            );
            return Ok(DiscoveredPeer {
                fingerprint: String::new(),
                alias: format!("Direct Peer ({ip})"),
                version: "2.0".to_string(),
                device_model: None,
                device_type: None,
                port: config.port,
                protocol: ProtocolType::Https,
                download: false,
                endpoints: vec![ep],
                discovery_sources: std::collections::HashSet::new(),
                first_seen_ms: 0,
                last_seen_ms: 0,
                metadata: std::collections::HashMap::new(),
            });
        }
    }

    // Run network scan
    println!("Scanning network to find target peer...");
    let scan_opts = ScanOptions {
        duration_secs: 2,
        multicast: true,
        mdns: true,
        subnet: false,
        fallback_subnet: true,
    };
    let peers = scan(config, identity, &scan_opts).await?;

    if peers.is_empty() {
        if let Some(q) = query {
            // If query is an IP or host, fall back to direct attempt
            anyhow::bail!("No peers found matching query '{q}'. Use explicit IP if known.");
        } else {
            anyhow::bail!("No peers discovered on local network.");
        }
    }

    if let Some(q) = query {
        if let Some(matched) = match_target_peer(q, &peers) {
            return Ok(matched.clone());
        }
        anyhow::bail!("No discovered peer matched query '{q}'.");
    }

    // Interactive picker when query is None
    if peers.len() == 1 {
        let p = &peers[0];
        println!("Single peer found: {} ({})", p.alias, p.primary_ip().map(|ip| ip.to_string()).unwrap_or_default());
        return Ok(p.clone());
    }

    println!("\nSelect a peer to send files to:");
    for (idx, p) in peers.iter().enumerate() {
        let ip_str = p.primary_ip().map(|ip| ip.to_string()).unwrap_or_else(|| "-".to_string());
        let fp_str = shorten_fingerprint(&p.fingerprint);
        println!("  [{}] {} | IP: {} | Port: {} | FP: {}", idx + 1, p.alias, ip_str, p.port, fp_str);
    }

    loop {
        let choice_str = prompt_user_input(&format!("Enter peer number (1-{}): ", peers.len()))?;
        if let Ok(choice) = choice_str.parse::<usize>()
            && (1..=peers.len()).contains(&choice)
        {
            return Ok(peers[choice - 1].clone());
        }
        println!("Invalid choice. Please enter a number between 1 and {}.", peers.len());
    }
}

/// Options for the `lanflare send` command.
#[derive(Clone, Debug)]
pub struct SendOptions {
    pub files: Vec<PathBuf>,
    pub to: Option<String>,
    pub pin: Option<String>,
}

/// Handles the `lanflare send` command.
///
/// Discovers or resolves target peer, calculates file sizes & SHA-256 checksums,
/// establishes transfer session with `TransferOrchestrator`, and displays real-time
/// multi-progress bars.
pub async fn send(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &SendOptions,
) -> anyhow::Result<()> {
    if options.files.is_empty() {
        anyhow::bail!("No files specified to send");
    }

    // 1. Verify and prepare all files
    let mut outgoing_files = Vec::with_capacity(options.files.len());
    let mut total_bytes = 0u64;

    for f in &options.files {
        if !f.exists() {
            anyhow::bail!("File not found: {}", f.display());
        }
        println!("Preparing file: {}...", f.display());
        let outgoing = OutgoingFile::from_path(f)
            .await
            .with_context(|| format!("Failed to read file metadata for: {}", f.display()))?
            .with_computed_sha256()
            .await
            .with_context(|| format!("Failed to compute SHA-256 for: {}", f.display()))?;

        total_bytes += outgoing.size;
        outgoing_files.push(outgoing);
    }

    println!(
        "Prepared {} file(s) ({:.2} MB total).",
        outgoing_files.len(),
        total_bytes as f64 / (1024.0 * 1024.0)
    );

    // 2. Resolve destination peer
    let target = discover_or_select_target(config, identity, options.to.as_deref()).await?;
    let base_url = target
        .primary_base_url()
        .context("Target peer does not have a valid IP endpoint")?;

    println!("Target resolved: {} at {}", target.alias, base_url);

    // 3. Configure HTTP client
    let target_fp = if target.fingerprint.is_empty() {
        None
    } else {
        Some(target.fingerprint.clone())
    };

    let client_config = LsClientConfig {
        timeout: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(5),
        tls_identity: Some(identity.clone()),
        target_fingerprint: target_fp,
    };
    let client = LsClient::new(client_config)
        .context("Failed to initialize LocalSend client with TLS identity")?;

    let sender_info = RegisterDtoV2 {
        alias: config.alias.clone(),
        version: "2.0".to_string(),
        device_model: Some("LanFlare CLI".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: identity.fingerprint.clone(),
        port: config.port,
        protocol: ProtocolType::Https,
        download: false,
    };

    let orchestrator = TransferOrchestrator::new(client, sender_info);

    // 4. Setup progress channel and progress tracker
    let (progress_tx, mut progress_rx) = mpsc::channel::<TransferProgress>(100);
    let mut tracker = TransferProgressTracker::new(true);

    let transfer_opts = TransferOptions {
        max_concurrency: 4,
        pin: options.pin.clone().or_else(|| config.pin.clone()),
        cancel_token: None,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    // Spawn orchestrator task
    let orchestrator_task = tokio::spawn(async move {
        orchestrator
            .transfer(&base_url, outgoing_files, transfer_opts, Some(progress_tx))
            .await
    });

    // Event pump for progress bars
    while let Some(ev) = progress_rx.recv().await {
        tracker.handle_event(&ev);
    }

    match orchestrator_task.await {
        Ok(Ok(())) => {
            println!("All files transferred successfully!");
            Ok(())
        }
        Ok(Err(err)) => {
            anyhow::bail!("Transfer failed: {err}");
        }
        Err(join_err) => {
            anyhow::bail!("Transfer task panicked or aborted: {join_err}");
        }
    }
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
///
/// Starts Hyper HTTP/HTTPS server daemon with optional PIN protection,
/// receives file upload sessions, supports auto-accept or interactive approval,
/// and saves incoming files directly to the download directory.
pub async fn receive(
    config: &AppConfig,
    identity: &TlsIdentity,
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
    let auto_accept = options.auto_accept || config.auto_accept;
    let pin = options.pin.clone().or_else(|| config.pin.clone());

    let server_config = AppConfig {
        alias: config.alias.clone(),
        port,
        download_dir: dest_dir.clone(),
        auto_accept,
        pin: pin.clone(),
    };

    let registry = PeerRegistry::new();
    let (event_tx, mut event_rx) = mpsc::channel::<ServerTransferEvent>(100);

    let server_handle = start_server(
        server_config,
        Some(identity.clone()),
        registry.clone(),
        Some(event_tx),
    )
    .await
    .with_context(|| format!("Failed to start receive server on port {port}"))?;

    let bound_port = server_handle.bound_port();
    println!("Receiver active and listening on port {bound_port}");
    println!("Saving files to: {}", dest_dir.display());
    println!("Auto-accept transfers: {}", if auto_accept { "Enabled" } else { "Disabled (interactive prompt)" });
    if let Some(ref p) = pin {
        println!("PIN protection enabled: required PIN is '{p}'");
    }
    println!("Certificate Fingerprint: {}", identity.fingerprint);
    println!("Press Ctrl+C to terminate receiver daemon.\n");

    // Event loop handling transfer events
    while let Some(event) = event_rx.recv().await {
        match event {
            ServerTransferEvent::DeviceRegistered { ip, info } => {
                println!("Device registered: {} ({}) at {}", info.alias, info.version, ip);
            }
            ServerTransferEvent::PrepareUpload {
                session_id,
                sender_ip,
                sender_info,
                files,
                decision_tx,
                ..
            } => {
                println!("\n[Incoming Transfer Request]");
                println!("  From:       {} ({})", sender_info.alias, sender_ip);
                println!("  Session ID: {session_id}");
                println!("  Files ({}) :", files.len());

                let mut total_size = 0u64;
                for (id, file) in &files {
                    total_size += file.size;
                    println!(
                        "    - {} ({:.2} MB) [ID: {}]",
                        file.file_name,
                        file.size as f64 / (1024.0 * 1024.0),
                        id
                    );
                }
                println!("  Total Size: {:.2} MB", total_size as f64 / (1024.0 * 1024.0));

                if auto_accept {
                    println!("Auto-accepting incoming transfer...");
                    let all_ids = files.keys().cloned().collect();
                    let _ = decision_tx.send(PrepareUploadDecision::Accept(all_ids));
                } else {
                    let ans = prompt_user_input("Accept this transfer? [Y/n]: ")
                        .unwrap_or_else(|_| "n".to_string());
                    if ans.is_empty() || ans.eq_ignore_ascii_case("y") || ans.eq_ignore_ascii_case("yes") {
                        let all_ids = files.keys().cloned().collect();
                        let _ = decision_tx.send(PrepareUploadDecision::Accept(all_ids));
                        println!("Accepted transfer.");
                    } else {
                        let _ = decision_tx.send(PrepareUploadDecision::Decline);
                        println!("Declined transfer.");
                    }
                }
            }
            ServerTransferEvent::FileStarted { file_name, total_bytes, .. } => {
                println!(
                    "Receiving file: {} ({:.2} MB)...",
                    file_name,
                    total_bytes as f64 / (1024.0 * 1024.0)
                );
            }
            ServerTransferEvent::FileCompleted { result, .. } => {
                println!(
                    "File saved successfully: {} (SHA-256: {}) ✓",
                    result.target_path.display(),
                    result.sha256
                );
            }
            ServerTransferEvent::FileFailed { file_id, error, .. } => {
                eprintln!("File transfer failed [ID: {file_id}]: {error}");
            }
            ServerTransferEvent::SessionEnded { session_id, reason } => {
                println!("Transfer session ended [ID: {session_id}]: {reason:?}\n");
            }
        }
    }

    server_handle.wait().await;
    Ok(())
}
