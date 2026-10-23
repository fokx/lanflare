//! CLI command implementations.

use anyhow::Context;
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::TlsIdentity;
use lanflare_core::discovery::interface::{is_sweepable_interface, local_interfaces, InterfaceFilter};
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

    // 4. Escalation worker: if subnet scan requested or fallback enabled,
    // escalate after a brief multicast grace period (500ms, matching official LocalSend)
    let registry_subnet = registry.clone();
    let identity_subnet = identity.clone();
    let port_subnet = config.port;
    let should_run_subnet = options.subnet || options.fallback_subnet;
    let immediate_subnet = options.subnet;

    let subnet_task = if should_run_subnet {
        Some(tokio::spawn(async move {
            if !immediate_subnet {
                // Multicast grace delay before escalating to subnet scan
                tokio::time::sleep(Duration::from_millis(500)).await;
                // If peers were already found via multicast/mDNS, skip subnet sweep
                if !registry_subnet.get_all().await.is_empty() {
                    return;
                }
            }

            let filter = InterfaceFilter::default();
            if let Ok(interfaces) = local_interfaces(&filter) {
                let valid_ifaces: Vec<_> = interfaces
                    .v4
                    .into_iter()
                    .filter(|iface| is_sweepable_interface(&iface.name, &iface.address))
                    .collect();

                if !valid_ifaces.is_empty() {
                    let descs: Vec<String> = valid_ifaces
                        .iter()
                        .map(|i| format!("{}/24 via {}", i.address, i.name))
                        .collect();
                    println!("Probing subnets in parallel: {}...", descs.join(", "));

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
                }
            }
        }))
    } else {
        None
    };

    // Wait for discovery to complete:
    // - Exits early as soon as peers are discovered (plus brief 250ms grace window for remaining packets)
    // - If immediate subnet scan was requested (-s), exits as soon as the subnet scan completes
    // - Otherwise waits for the configured scan duration
    let start = std::time::Instant::now();
    let scan_timeout = Duration::from_secs(options.duration_secs);

    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;

        if !registry.get_all().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(250)).await;
            break;
        }

        if let Some(ref st) = subnet_task {
            if st.is_finished() && (immediate_subnet || start.elapsed() >= scan_timeout) {
                break;
            }
        } else if start.elapsed() >= scan_timeout {
            break;
        }

        if start.elapsed() >= scan_timeout.max(Duration::from_secs(5)) {
            break;
        }
    }

    if let Some(st) = subnet_task {
        let _ = st.await;
    }

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

    // 5. Retrieve active peers
    let peers = registry.get_all().await;

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
                            hop_count: None,
                            route_path: None,
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
                hop_count: None,
                route_path: None,
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
        ..Default::default()
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
            ServerTransferEvent::FileProgress { .. } => {}
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
            ServerTransferEvent::BrowserDownload {
                remote_ip,
                file_name,
                file_size,
                decision_tx,
                ..
            } => {
                println!(
                    "Browser download requested by {} for '{}' ({:.2} MB). Auto-accepting.",
                    remote_ip,
                    file_name,
                    file_size as f64 / (1024.0 * 1024.0)
                );
                let _ = decision_tx.send(true);
            }
            ServerTransferEvent::SessionEnded { session_id, reason } => {
                println!("Transfer session ended [ID: {session_id}]: {reason:?}\n");
            }
        }
    }

    server_handle.wait().await;
    Ok(())
}

/// Options for the `lanflare serve` command.
#[derive(Clone, Debug)]
pub struct ServeOptions {
    pub path: PathBuf,
    pub port: Option<u16>,
    pub allow_upload: bool,
    pub allow_delete: bool,
    pub pin: Option<String>,
}

/// Serves a directory to peers on local network using dufs-compatible file server.
pub async fn serve(
    config: &AppConfig,
    _identity: &TlsIdentity,
    options: &ServeOptions,
) -> anyhow::Result<()> {
    let folder = &options.path;
    if !folder.is_dir() {
        anyhow::bail!("Directory does not exist or is not accessible: {}", folder.display());
    }

    let folder_canon = folder.canonicalize().unwrap_or_else(|_| folder.clone());
    let folder_name = folder_canon
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Shared Folder")
        .to_string();

    let port = options.port.unwrap_or(config.port);
    let title = format!("{} ({})", folder_name, config.alias);

    let dufs_config = lanflare_core::server::DufsConfig {
        root_path: folder_canon.clone(),
        path_prefix: String::new(),
        allow_upload: options.allow_upload,
        allow_delete: options.allow_delete,
        auth_pin: options.pin.clone(),
        title,
    };

    let handle = lanflare_core::server::dufs::start_standalone_dufs_server(dufs_config, port).await?;
    let bound_port = handle.port;

    println!("============================================================");
    println!(" LanFlare Dufs File Server Active");
    println!("============================================================");
    println!(" Serving Directory:  {}", folder_canon.display());
    println!(" Bound Port:         {}", bound_port);
    println!(" Uploads Allowed:    {}", if options.allow_upload { "Yes" } else { "No" });
    println!(" Deletions Allowed:  {}", if options.allow_delete { "Yes" } else { "No" });
    if let Some(ref pin) = options.pin {
        println!(" PIN Protection:     Required (PIN: {})", pin);
        println!(" Login Hint:         Enter Password only (no username required)");
    } else {
        println!(" PIN Protection:     Open Access (No PIN)");
    }
    println!();
    println!(" Local Network Access URLs:");

    let auth_prefix = match &options.pin {
        Some(p) if !p.is_empty() => format!(":{p}@"),
        _ => String::new(),
    };

    let filter = InterfaceFilter::default();
    if let Ok(interfaces) = local_interfaces(&filter) {
        for v4 in interfaces.v4 {
            if v4.address.to_string() != "127.0.0.1" {
                println!("   http://{}{}:{}/", auth_prefix, v4.address, bound_port);
            }
        }
    }
    println!("   http://{}127.0.0.1:{}/", auth_prefix, bound_port);
    println!();
    println!("Peers on your Wi-Fi/LAN can open any of the above URLs in a browser.");
    println!("Press Ctrl+C to stop the server.\n");

    tokio::signal::ctrl_c().await?;
    println!("\nShutting down Dufs server...");
    handle.stop();
    Ok(())
}

fn current_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ============================================================================
// Mesh & Network Simulator Commands
// ============================================================================

/// Options for the `lanflare mesh status` command.
#[derive(Clone, Debug, Default)]
pub struct MeshStatusOptions {
    /// Whether to advertise relay capability
    pub relay: bool,
}

/// Handles `lanflare mesh status`.
pub async fn mesh_status(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &MeshStatusOptions,
) -> anyhow::Result<()> {
    let local_fp = &identity.fingerprint;
    println!("Inspecting local mesh overlay and discovering peers...");

    let scan_opts = ScanOptions {
        duration_secs: 2,
        ..Default::default()
    };
    let peers = scan(config, identity, &scan_opts).await.unwrap_or_default();

    let mut graph = lanflare_core::mesh::graph::MeshTopologyGraph::new(local_fp.clone());
    let mut local_node = lanflare_core::mesh::graph::MeshNode::new(
        local_fp.clone(),
        &config.alias,
        options.relay,
    );
    local_node.cert_pem = Some(identity.cert_pem.clone());
    graph.insert_node(local_node);

    let now_ms = current_time_millis();
    for peer in &peers {
        let is_relay = peer.device_type == Some(lanflare_proto::discovery::DeviceType::Server)
            || peer.metadata.get("relay").map(|v| v == "true").unwrap_or(false);
        let node = lanflare_core::mesh::graph::MeshNode::new(
            peer.fingerprint.clone(),
            &peer.alias,
            is_relay,
        );
        graph.insert_node(node);

        let rtt_est = 10;
        graph.add_bidirectional_link(local_fp, &peer.fingerprint, rtt_est, now_ms);
    }

    let rendered = crate::tui::render_mesh_status(
        &config.alias,
        local_fp,
        options.relay,
        &graph,
    );
    println!("{rendered}");
    Ok(())
}

/// Options for `lanflare mesh sim`.
#[derive(Clone, Debug)]
pub struct MeshSimOptions {
    pub nodes: usize,
    pub duration: u64,
    pub subnets: Vec<String>,
    pub latency_ms: u32,
}

/// Handles `lanflare mesh sim`.
pub async fn mesh_sim(
    _config: &AppConfig,
    _identity: &TlsIdentity,
    options: &MeshSimOptions,
) -> anyhow::Result<()> {
    println!("Initializing LanFlare Virtual Network Simulator...");
    let matrix = std::sync::Arc::new(lanflare_core::mesh::sim::VirtualNetworkMatrix::new());
    let mut cluster = lanflare_core::mesh::sim::SimulatedMeshCluster::new(matrix.clone());

    let node_count = options.nodes.max(2);
    let subnets = if options.subnets.is_empty() {
        vec!["SubnetA".to_string(), "SubnetB".to_string()]
    } else {
        options.subnets.clone()
    };

    println!("Spawning {node_count} simulated nodes across {} subnets...", subnets.len());

    let mut spawned_fps = Vec::new();
    for i in 0..node_count {
        let alias = match i {
            0 => "Alice".to_string(),
            1 if node_count == 2 => "Bob".to_string(),
            1 => "Relay-1".to_string(),
            2 if node_count == 3 => "Bob".to_string(),
            2 => "Relay-2".to_string(),
            3 => "Bob".to_string(),
            n => format!("Node-{}", n + 1),
        };

        let node_subnets = if node_count <= 2 || i == 0 {
            vec![subnets[0].clone()]
        } else if i == node_count - 1 {
            vec![subnets[subnets.len() - 1].clone()]
        } else {
            let s1 = (i - 1) % subnets.len();
            let s2 = i % subnets.len();
            if s1 != s2 {
                vec![subnets[s1].clone(), subnets[s2].clone()]
            } else {
                vec![subnets[s1].clone()]
            }
        };

        let is_relay = i > 0 && i < node_count - 1;
        let fp = cluster.spawn_node(&alias, node_subnets, is_relay).await?;
        spawned_fps.push(fp);
    }

    if options.latency_ms > 0 {
        for i in 0..spawned_fps.len() {
            for j in (i + 1)..spawned_fps.len() {
                let cfg = lanflare_core::mesh::sim::SimulatedLinkConfig::new()
                    .with_latency(options.latency_ms)
                    .with_jitter(options.latency_ms / 4);
                matrix.set_bidirectional_link_config(&spawned_fps[i], &spawned_fps[j], cfg);
            }
        }
    }

    let direct_links = cluster.establish_direct_links().await;
    println!("Established {direct_links} direct virtual links.");

    println!("Running 3 gossip exchange rounds across overlay...");
    cluster.exchange_gossip_rounds(3).await?;

    let table = crate::tui::render_sim_cluster_status(&cluster);
    println!("{table}");

    if options.duration > 0 {
        println!("Virtual mesh sandbox running for {} seconds (Ctrl+C to abort)...", options.duration);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(options.duration)) => {},
            _ = tokio::signal::ctrl_c() => {
                println!("\nReceived Ctrl+C, shutting down cluster...");
            }
        }
    } else {
        println!("Virtual mesh sandbox running interactively. Press Ctrl+C to terminate...");
        let _ = tokio::signal::ctrl_c().await;
        println!("\nShutting down cluster...");
    }

    cluster.shutdown();
    println!("Virtual mesh simulator shutdown cleanly.");
    Ok(())
}

/// Options for `lanflare mesh trace`.
#[derive(Clone, Debug)]
pub struct MeshTraceOptions {
    pub target: String,
    pub ttl: u8,
}

/// Handles `lanflare mesh trace <target>`.
pub async fn mesh_trace(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &MeshTraceOptions,
) -> anyhow::Result<()> {
    let target_peer = discover_or_select_target(config, identity, Some(&options.target)).await?;
    let canonical_target = target_peer.fingerprint.to_ascii_uppercase();

    println!(
        "Running traceroute to {} ({}) with TTL={}...",
        target_peer.alias,
        crate::tui::shorten_fingerprint(&target_peer.fingerprint),
        options.ttl
    );

    let graph = std::sync::Arc::new(tokio::sync::RwLock::new(
        lanflare_core::mesh::graph::MeshTopologyGraph::new(identity.fingerprint.clone()),
    ));
    let router_cfg = lanflare_core::mesh::router::MeshRouterConfig {
        local_fingerprint: identity.fingerprint.clone(),
        local_alias: config.alias.clone(),
        relay_enabled: true,
        ..Default::default()
    };
    let router = std::sync::Arc::new(lanflare_core::mesh::router::MeshRouter::with_config(router_cfg, graph.clone()));

    let now_ms = current_time_millis();
    if let Some(base_url) = target_peer.primary_base_url() {
        router.register_peer_endpoint(&canonical_target, &base_url).await;
        let mut g = graph.write().await;
        let node = lanflare_core::mesh::graph::MeshNode::new(
            canonical_target.clone(),
            &target_peer.alias,
            false,
        );
        g.insert_node(node);
        g.add_bidirectional_link(&identity.fingerprint, &canonical_target, 15, now_ms);
    }

    let diagnostics = lanflare_core::mesh::diagnostics::MeshDiagnostics::new(&config.alias);
    match diagnostics.traceroute(&router, &canonical_target).await {
        Ok(trace_res) => {
            let rendered = crate::tui::render_traceroute_table(&target_peer.alias, &canonical_target, &trace_res);
            println!("{rendered}");
            Ok(())
        }
        Err(e) => {
            let hop = lanflare_proto::mesh::TracerouteHop {
                node_fp: identity.fingerprint.clone(),
                alias: config.alias.clone(),
                rtt_ms: 0,
                ingress_timestamp_ms: current_time_millis(),
            };
            let synth_res = lanflare_proto::mesh::MeshTracerouteResponse {
                trace_id: uuid::Uuid::new_v4().to_string(),
                origin_fp: identity.fingerprint.clone(),
                target_fp: canonical_target.clone(),
                success: false,
                total_hops: 1,
                total_rtt_ms: 0,
                hops: vec![hop],
                error: Some(e.to_string()),
            };
            let rendered = crate::tui::render_traceroute_table(&target_peer.alias, &canonical_target, &synth_res);
            println!("{rendered}");
            println!("Note: Diagnostics reported: {e}");
            Ok(())
        }
    }
}

/// Options for `lanflare mesh benchmark`.
#[derive(Clone, Debug)]
pub struct MeshBenchmarkOptions {
    pub target: String,
    pub chunks: u32,
    pub chunk_size: usize,
}

/// Handles `lanflare mesh benchmark <target>`.
pub async fn mesh_benchmark(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &MeshBenchmarkOptions,
) -> anyhow::Result<()> {
    let target_peer = discover_or_select_target(config, identity, Some(&options.target)).await?;
    let canonical_target = target_peer.fingerprint.to_ascii_uppercase();

    println!(
        "Starting in-memory throughput benchmark to {} ({}): {} chunks of {} bytes...",
        target_peer.alias,
        crate::tui::shorten_fingerprint(&target_peer.fingerprint),
        options.chunks,
        options.chunk_size
    );

    let graph = std::sync::Arc::new(tokio::sync::RwLock::new(
        lanflare_core::mesh::graph::MeshTopologyGraph::new(identity.fingerprint.clone()),
    ));
    let router_cfg = lanflare_core::mesh::router::MeshRouterConfig {
        local_fingerprint: identity.fingerprint.clone(),
        local_alias: config.alias.clone(),
        relay_enabled: true,
        ..Default::default()
    };
    let router = std::sync::Arc::new(lanflare_core::mesh::router::MeshRouter::with_config(router_cfg, graph.clone()));

    let now_ms = current_time_millis();
    if let Some(base_url) = target_peer.primary_base_url() {
        router.register_peer_endpoint(&canonical_target, &base_url).await;
        let mut g = graph.write().await;
        let node = lanflare_core::mesh::graph::MeshNode::new(
            canonical_target.clone(),
            &target_peer.alias,
            false,
        );
        g.insert_node(node);
        g.add_bidirectional_link(&identity.fingerprint, &canonical_target, 15, now_ms);
    }

    let diagnostics = lanflare_core::mesh::diagnostics::MeshDiagnostics::new(&config.alias);
    match diagnostics.run_speedtest(&router, &canonical_target, options.chunks, options.chunk_size).await {
        Ok(res) => {
            let rendered = crate::tui::render_speedtest_result(&target_peer.alias, &canonical_target, &res);
            println!("{rendered}");
            Ok(())
        }
        Err(e) => {
            println!("Benchmark error: {e}");
            Err(e.into())
        }
    }
}

// ============================================================================
// Encrypted Chat & Stranger Trust Commands
// ============================================================================

/// Options for `lanflare chat send`.
#[derive(Clone, Debug)]
pub struct ChatSendOptions {
    pub target: String,
    pub message: String,
    pub intro: bool,
}

/// Handles `lanflare chat send <target> <message>`.
pub async fn chat_send(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &ChatSendOptions,
) -> anyhow::Result<()> {
    let target_peer = discover_or_select_target(config, identity, Some(&options.target)).await?;
    let canonical_target = target_peer.fingerprint.to_ascii_uppercase();

    let chat_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lanflare")
        .join("chat");
    tokio::fs::create_dir_all(&chat_dir).await?;

    let chat_cfg = lanflare_core::chat::ChatEngineConfig::new(&config.alias, &chat_dir);
    let registry = PeerRegistry::new();
    registry.insert_or_update(target_peer.clone()).await;

    let chat_engine = lanflare_core::chat::ChatEngine::new(identity.clone(), chat_cfg, registry).await?;

    let is_stranger = options.intro
        || chat_engine.trust_gate().get_trust_state(&canonical_target)
            == lanflare_core::chat::trust::ContactTrustState::Stranger;

    if is_stranger {
        println!("Peer is not yet trusted. Sending stranger introduction handshake with Hashcash PoW...");
        let req = chat_engine.send_trust_request(&canonical_target, &options.message).await?;
        println!("Trust handshake dispatched:");
        println!("  Request ID:  {}", req.request_id);
        println!("  Target FP:   {}", req.target_fp);
        println!("  PoW Nonce:   {}", req.pow_stamp.nonce);
        println!("  Status:      Pending recipient approval");
    } else {
        let msg = chat_engine.send_text_message(&canonical_target, &options.message).await?;
        println!("Chat message dispatched:");
        println!("  Message ID:  {}", msg.id);
        println!("  Recipient:   {} ({})", target_peer.alias, canonical_target);
        println!("  Content:     {}", options.message);
        println!("  Status:      Enqueued in DTN Outbox (Queued / Sending)");
    }

    Ok(())
}

/// Options for `lanflare chat trust`.
#[derive(Clone, Debug)]
pub struct ChatTrustOptions {
    pub target: String,
    pub approve: bool,
    pub block: bool,
}

/// Handles `lanflare chat trust <target>`.
pub async fn chat_trust(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &ChatTrustOptions,
) -> anyhow::Result<()> {
    let canonical_target = options.target.trim().to_ascii_uppercase();

    let chat_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lanflare")
        .join("chat");
    tokio::fs::create_dir_all(&chat_dir).await?;

    let chat_cfg = lanflare_core::chat::ChatEngineConfig::new(&config.alias, &chat_dir);
    let registry = PeerRegistry::new();
    let chat_engine = lanflare_core::chat::ChatEngine::new(identity.clone(), chat_cfg, registry).await?;

    if options.approve {
        chat_engine.approve_contact(&canonical_target).await?;
        println!(
            "Contact {} successfully approved and elevated to TrustedContact.",
            crate::tui::shorten_fingerprint(&canonical_target)
        );
    } else if options.block {
        chat_engine.block_contact(&canonical_target).await?;
        println!(
            "Contact {} successfully blocked.",
            crate::tui::shorten_fingerprint(&canonical_target)
        );
    } else {
        let state = chat_engine.trust_gate().get_trust_state(&canonical_target);
        let contact_alias = chat_engine
            .store()
            .get_contact(&canonical_target)
            .await
            .map(|c| c.alias)
            .unwrap_or_else(|| "Unknown".to_string());
        let rendered = crate::tui::render_contact_trust_status(&contact_alias, &canonical_target, state);
        println!("{rendered}");
    }

    Ok(())
}

/// Options for `lanflare chat groups`.
#[derive(Clone, Debug, Default)]
pub struct ChatGroupsOptions {
    pub id: Option<String>,
}

/// Handles `lanflare chat groups`.
pub async fn chat_groups(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &ChatGroupsOptions,
) -> anyhow::Result<()> {
    let chat_dir = dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("lanflare")
        .join("chat");
    tokio::fs::create_dir_all(&chat_dir).await?;

    let chat_cfg = lanflare_core::chat::ChatEngineConfig::new(&config.alias, &chat_dir);
    let registry = PeerRegistry::new();
    let chat_engine = lanflare_core::chat::ChatEngine::new(identity.clone(), chat_cfg, registry).await?;

    if let Some(group_id) = &options.id {
        if let Some(group_arc) = chat_engine.get_group(group_id).await {
            let state = group_arc.read().await;
            let rendered = crate::tui::render_group_detail(&state);
            println!("{rendered}");
        } else {
            println!("Group not found: {}", group_id);
        }
    } else {
        let group_ids = chat_engine.group_sync().list_groups().await;
        let mut group_info = Vec::new();
        for gid in &group_ids {
            if let Some(g_arc) = chat_engine.get_group(gid).await {
                let s = g_arc.read().await;
                group_info.push((
                    s.roster().group_id.clone(),
                    s.roster().group_name.clone(),
                    s.current_epoch(),
                    s.roster().members.len(),
                    s.len(),
                ));
            }
        }
        let borrowed_info: Vec<(&str, &str, u64, usize, usize)> = group_info
            .iter()
            .map(|(id, name, ep, mems, msgs)| (id.as_str(), name.as_str(), *ep, *mems, *msgs))
            .collect();
        let rendered = crate::tui::render_groups_table(&borrowed_info);
        println!("{rendered}");
    }

    Ok(())
}

// ============================================================================
// Federated File Search Commands
// ============================================================================

/// Options for `lanflare search`.
#[derive(Clone, Debug)]
pub struct SearchOptions {
    pub query: String,
    pub pin: Option<String>,
    pub timeout: u64,
}

/// Handles `lanflare search <query>`.
pub async fn search(
    config: &AppConfig,
    identity: &TlsIdentity,
    options: &SearchOptions,
) -> anyhow::Result<()> {
    println!("Executing federated search for \"{}\" across LAN and mesh peers...", options.query);

    let catalog = std::sync::Arc::new(lanflare_core::toolbox::search::MeshFileCatalog::new(
        &identity.fingerprint,
        &config.alias,
    ));
    if config.download_dir.exists() {
        let _ = catalog.add_shared_folder("Shared", &config.download_dir, "/", None);
    }

    let scan_opts = ScanOptions {
        duration_secs: options.timeout.min(2),
        ..Default::default()
    };
    let peers = scan(config, identity, &scan_opts).await.unwrap_or_default();

    let registry = PeerRegistry::new();
    for peer in peers {
        registry.insert_or_update(peer).await;
    }

    let search_engine = lanflare_core::toolbox::search::FederatedSearchEngine::new(
        catalog,
        &identity.fingerprint,
        &config.alias,
        config.port,
    )
    .with_registry(registry);

    let mut query_dto = lanflare_proto::toolbox::FileSearchQueryDto::new(
        &options.query,
        &identity.fingerprint,
    )
    .with_max_results(50);
    if let Some(pin) = &options.pin {
        query_dto = query_dto.with_pin(pin);
    }

    let results = search_engine
        .search_federated(&query_dto, Duration::from_secs(options.timeout))
        .await;

    let rendered = crate::tui::render_search_results_table(&options.query, &results);
    println!("{rendered}");
    Ok(())
}
