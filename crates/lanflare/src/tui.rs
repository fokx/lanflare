//! CLI terminal output & pretty-printing utilities.

use lanflare_core::discovery::registry::DiscoveredPeer;
use lanflare_proto::discovery::{DeviceType, ProtocolType};

/// Returns an icon representing the device type or OS model.
pub fn get_device_icon(device_type: Option<DeviceType>, model: Option<&str>) -> &'static str {
    // Check model string for OS hints first
    if let Some(m) = model {
        let m_lower = m.to_ascii_lowercase();
        if m_lower.contains("windows") {
            return "🪟 ";
        } else if m_lower.contains("mac") || m_lower.contains("darwin") || m_lower.contains("apple") {
            return "🍎";
        } else if m_lower.contains("linux") || m_lower.contains("ubuntu") || m_lower.contains("debian") || m_lower.contains("fedora") || m_lower.contains("arch") {
            return "🐧";
        } else if m_lower.contains("android") {
            return "🤖";
        } else if m_lower.contains("iphone") || m_lower.contains("ipad") || m_lower.contains("ios") {
            return "📱";
        }
    }

    match device_type {
        Some(DeviceType::Mobile) => "📱",
        Some(DeviceType::Desktop) => "💻",
        Some(DeviceType::Web) => "🌐",
        Some(DeviceType::Headless) => "⚡",
        Some(DeviceType::Server) => "🖥️ ",
        None => "💻",
    }
}

/// Formats protocol type for display with HTTPS/HTTP indicators.
pub fn format_protocol(protocol: ProtocolType) -> &'static str {
    match protocol {
        ProtocolType::Http => "HTTP (plain)",
        ProtocolType::Https => "HTTPS (mTLS)",
    }
}

/// Shortens a 64-char hex fingerprint for compact display, e.g. "A1B2C3D4...E5F6"
pub fn shorten_fingerprint(fp: &str) -> String {
    if fp.len() <= 16 {
        fp.to_string()
    } else {
        format!("{}...{}", &fp[..8], &fp[fp.len() - 6..])
    }
}

/// Renders a formatted text table of discovered peers.
pub fn render_peer_table(peers: &[DiscoveredPeer]) -> String {
    if peers.is_empty() {
        return "No peers discovered on local network.\n".to_string();
    }

    let mut out = String::new();
    out.push_str("\nDiscovered Peers:\n");
    out.push_str(&format!(
        "{:<3} {:<22} {:<18} {:<14} {:<16} {:<16}\n",
        "OS", "ALIAS", "PRIMARY IP", "PORT", "PROTOCOL", "FINGERPRINT"
    ));
    out.push_str(&format!(
        "{:-<3} {:-<22} {:-<18} {:-<14} {:-<16} {:-<16}\n",
        "", "", "", "", "", ""
    ));

    for peer in peers {
        let icon = get_device_icon(peer.device_type, peer.device_model.as_deref());
        let alias = if peer.alias.len() > 20 {
            format!("{}…", &peer.alias[..19])
        } else {
            peer.alias.clone()
        };
        let ip_str = peer
            .primary_ip()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "-".to_string());
        let port_str = peer.port.to_string();
        let proto_str = match peer.protocol {
            ProtocolType::Http => "HTTP",
            ProtocolType::Https => "HTTPS (TLS)",
        };
        let fp_str = shorten_fingerprint(&peer.fingerprint);

        out.push_str(&format!(
            "{:<3} {:<22} {:<18} {:<14} {:<16} {:<16}\n",
            icon, alias, ip_str, port_str, proto_str, fp_str
        ));
    }

    out
}

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use lanflare_core::client::transfer::TransferProgress;

/// Manages multiple progress bars for a file transfer session.
pub struct TransferProgressTracker {
    mp: MultiProgress,
    overall_pb: ProgressBar,
    file_pbs: HashMap<String, ProgressBar>,
    spinner_style: ProgressStyle,
    bar_style: ProgressStyle,
    is_interactive: bool,
}

impl Default for TransferProgressTracker {
    fn default() -> Self {
        Self::new(true)
    }
}

impl TransferProgressTracker {
    /// Creates a new progress tracker.
    ///
    /// If `is_interactive` is false, progress bars will be hidden/silent
    /// (useful for headless/scripting environments or unit tests).
    pub fn new(is_interactive: bool) -> Self {
        let mp = MultiProgress::new();

        let bar_style = ProgressStyle::default_bar()
            .template("{prefix:>18} [{elapsed_precise}] [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("━╾─");

        let spinner_style = ProgressStyle::default_spinner()
            .template("{prefix:>18} {spinner:.green} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());

        let overall_pb = if is_interactive {
            mp.add(ProgressBar::new(0))
        } else {
            ProgressBar::hidden()
        };
        overall_pb.set_style(bar_style.clone());
        overall_pb.set_prefix("Overall Progress");

        Self {
            mp,
            overall_pb,
            file_pbs: HashMap::new(),
            spinner_style,
            bar_style,
            is_interactive,
        }
    }

    /// Handles an incoming [`TransferProgress`] event and updates progress bars accordingly.
    pub fn handle_event(&mut self, event: &TransferProgress) {
        match event {
            TransferProgress::Started { total_bytes, accepted_files, .. } => {
                self.overall_pb.set_length(*total_bytes);
                self.overall_pb.set_position(0);
                self.overall_pb.set_message(format!("Uploading {} file(s)", accepted_files.len()));
            }
            TransferProgress::FileProgress {
                file_id,
                file_name,
                file_bytes_sent,
                file_total_bytes,
                session_bytes_sent,
                ..
            } => {
                self.overall_pb.set_position(*session_bytes_sent);

                let pb = self.file_pbs.entry(file_id.clone()).or_insert_with(|| {
                    let pb = if self.is_interactive {
                        self.mp.add(ProgressBar::new(*file_total_bytes))
                    } else {
                        ProgressBar::hidden()
                    };
                    pb.set_style(self.bar_style.clone());
                    let prefix = if file_name.len() > 18 {
                        format!("…{}", &file_name[file_name.len() - 17..])
                    } else {
                        file_name.clone()
                    };
                    pb.set_prefix(prefix);
                    pb
                });

                pb.set_position(*file_bytes_sent);
            }
            TransferProgress::FileFinished { file_id, file_name, .. } => {
                if let Some(pb) = self.file_pbs.get(file_id) {
                    pb.finish_with_message("Done ✓");
                } else if self.is_interactive {
                    let pb = self.mp.add(ProgressBar::new(0));
                    pb.set_style(self.spinner_style.clone());
                    pb.set_prefix(file_name.clone());
                    pb.finish_with_message("Done ✓");
                }
            }
            TransferProgress::FileFailed { file_id, error, .. } => {
                if let Some(pb) = self.file_pbs.get(file_id) {
                    pb.abandon_with_message(format!("Failed: {error}"));
                }
            }
            TransferProgress::SessionCompleted { total_files, total_bytes, duration, .. } => {
                self.overall_pb.set_position(*total_bytes);
                self.overall_pb.finish_with_message(format!(
                    "Completed {} file(s) in {:.1}s ✓",
                    total_files,
                    duration.as_secs_f64()
                ));
            }
            TransferProgress::SessionCancelled { reason, .. } => {
                self.overall_pb.abandon_with_message(format!("Cancelled: {reason}"));
            }
        }
    }

    /// Access the underlying MultiProgress if needed.
    pub fn multi_progress(&self) -> &MultiProgress {
        &self.mp
    }
}

/// Renders the mesh overlay status and known topology table.
pub fn render_mesh_status(
    alias: &str,
    local_fp: &str,
    relay: bool,
    graph: &lanflare_core::mesh::graph::MeshTopologyGraph,
) -> String {
    let mut out = String::new();
    out.push_str("\nLanFlare Mesh Overlay Status:\n");
    out.push_str(&format!("  Local Alias:   {}\n", alias));
    out.push_str(&format!("  Local FP:      {}\n", shorten_fingerprint(local_fp)));
    out.push_str(&format!("  Relay Mode:    {}\n", if relay { "Enabled" } else { "Disabled" }));
    out.push_str(&format!("  Known Nodes:   {}\n", graph.node_count()));
    out.push_str(&format!("  Active Links:  {}\n", graph.link_count()));
    out.push_str("\nOverlay Topology:\n");
    out.push_str(&format!(
        "{:<20} {:<18} {:<8} {:<6} {:<18} {:<10}\n",
        "ALIAS", "FINGERPRINT", "RELAY", "HOPS", "NEXT HOP", "RTT (EST)"
    ));
    out.push_str(&format!(
        "{:-<20} {:-<18} {:-<8} {:-<6} {:-<18} {:-<10}\n",
        "", "", "", "", "", ""
    ));

    for node in graph.nodes() {
        if node.fingerprint == local_fp {
            continue;
        }
        let node_alias = if node.alias.len() > 19 {
            format!("{}…", &node.alias[..18])
        } else {
            node.alias.clone()
        };
        let fp_short = shorten_fingerprint(&node.fingerprint);
        let relay_str = if node.relay_enabled { "Yes" } else { "No" };
        let (hops_str, next_hop_str, rtt_str) = if let Some(route) = graph.resolve_route(&node.fingerprint) {
            let hops = route.full_route_path.len().saturating_sub(1);
            let next_hop = if route.next_hop_fp == node.fingerprint {
                "Direct".to_string()
            } else {
                shorten_fingerprint(&route.next_hop_fp)
            };
            (hops.to_string(), next_hop, format!("{} ms", route.total_cost_ms))
        } else {
            ("-".to_string(), "-".to_string(), "-".to_string())
        };

        out.push_str(&format!(
            "{:<20} {:<18} {:<8} {:<6} {:<18} {:<10}\n",
            node_alias, fp_short, relay_str, hops_str, next_hop_str, rtt_str
        ));
    }

    out
}

/// Renders a summary of the simulated virtual mesh cluster.
pub fn render_sim_cluster_status(cluster: &lanflare_core::mesh::sim::SimulatedMeshCluster) -> String {
    let mut out = String::new();
    out.push_str("\nSimulated Virtual Mesh Cluster:\n");
    out.push_str(&format!(
        "{:<16} {:<18} {:<8} {:<24} {:<14}\n",
        "NODE ALIAS", "FINGERPRINT", "PORT", "SUBNETS", "RELAY ENABLED"
    ));
    out.push_str(&format!(
        "{:-<16} {:-<18} {:-<8} {:-<24} {:-<14}\n",
        "", "", "", "", ""
    ));

    for node in cluster.nodes() {
        let node_alias = node.alias();
        let fp_short = shorten_fingerprint(node.fingerprint());
        let port = node.bound_port().to_string();
        let subnets = node.subnets().join(", ");
        let relay = if node.mesh_router().is_relay_enabled() {
            "Yes"
        } else {
            "No"
        };

        out.push_str(&format!(
            "{:<16} {:<18} {:<8} {:<24} {:<14}\n",
            node_alias, fp_short, port, subnets, relay
        ));
    }
    out
}

/// Renders a traceroute hop table and summary.
pub fn render_traceroute_table(
    target_alias: &str,
    target_fp: &str,
    trace: &lanflare_proto::mesh::MeshTracerouteResponse,
) -> String {
    let mut out = String::new();
    let trace_id_short = if trace.trace_id.len() > 8 {
        &trace.trace_id[..8]
    } else {
        &trace.trace_id
    };
    out.push_str(&format!(
        "\nTraceroute to {} ({}) [Trace ID: {}]:\n",
        target_alias,
        shorten_fingerprint(target_fp),
        trace_id_short
    ));
    out.push_str(&format!(
        "{:<4} {:<20} {:<18} {:<10} {:<10}\n",
        "HOP", "NODE ALIAS", "FINGERPRINT", "RTT", "DELTA"
    ));
    out.push_str(&format!(
        "{:-<4} {:-<20} {:-<18} {:-<10} {:-<10}\n",
        "", "", "", "", ""
    ));

    let mut prev_rtt = 0u32;
    for (i, hop) in trace.hops.iter().enumerate() {
        let hop_num = (i + 1).to_string();
        let alias = if hop.alias.len() > 19 {
            format!("{}…", &hop.alias[..18])
        } else {
            hop.alias.clone()
        };
        let fp_short = shorten_fingerprint(&hop.node_fp);
        let rtt_str = format!("{} ms", hop.rtt_ms);
        let delta_str = if i == 0 {
            "-".to_string()
        } else {
            let delta = hop.rtt_ms.saturating_sub(prev_rtt);
            format!("+{} ms", delta)
        };
        prev_rtt = hop.rtt_ms;

        out.push_str(&format!(
            "{:<4} {:<20} {:<18} {:<10} {:<10}\n",
            hop_num, alias, fp_short, rtt_str, delta_str
        ));
    }

    out.push_str(&format!(
        "\nStatus: {}\nTotal Elapsed RTT: {} ms across {} hop(s)\n",
        if trace.success {
            "Destination Reached"
        } else {
            "TTL Expired / Incomplete"
        },
        trace.total_rtt_ms,
        trace.hops.len()
    ));

    out
}

/// Renders throughput benchmark results.
pub fn render_speedtest_result(
    target_alias: &str,
    target_fp: &str,
    bench: &lanflare_proto::mesh::SpeedtestResultDto,
) -> String {
    let mut out = String::new();
    out.push_str("\nMesh Throughput Benchmark Result:\n");
    out.push_str(&format!("  Target Peer:   {} ({})\n", target_alias, shorten_fingerprint(target_fp)));
    out.push_str(&format!("  Test ID:       {}\n", bench.test_id));
    out.push_str(&format!(
        "  Payload Size:  {:.2} MB ({} chunks, {} bytes)\n",
        (bench.total_bytes as f64) / (1024.0 * 1024.0),
        bench.chunks_received,
        bench.total_bytes
    ));
    out.push_str(&format!("  Elapsed Time:  {} ms\n", bench.duration_ms));
    let mb_per_sec = bench.throughput_mbps / 8.0;
    out.push_str(&format!(
        "  Throughput:    {:.2} Mbps ({:.2} MB/s)\n",
        bench.throughput_mbps, mb_per_sec
    ));
    out.push_str(&format!(
        "  Status:        {}\n",
        if bench.chunks_received > 0 { "SUCCESS (Delivered)" } else { "FAILED / NO CHUNKS" }
    ));
    out
}

/// Renders a contact's anti-harassment trust status.
pub fn render_contact_trust_status(
    alias: &str,
    fp: &str,
    state: lanflare_core::chat::trust::ContactTrustState,
) -> String {
    let mut out = String::new();
    out.push_str("\nContact Trust Inspection:\n");
    out.push_str(&format!("  Alias:        {}\n", alias));
    out.push_str(&format!("  Fingerprint:  {}\n", fp));
    let state_str = match state {
        lanflare_core::chat::trust::ContactTrustState::Stranger => "Stranger (Zero disk allocation, PoW required)",
        lanflare_core::chat::trust::ContactTrustState::PendingApproval => "Pending Approval (Introduction received, awaiting user approval)",
        lanflare_core::chat::trust::ContactTrustState::TrustedContact => "Trusted Contact (Full text, media, and group privileges)",
        lanflare_core::chat::trust::ContactTrustState::Blocked => "Blocked (All packets and handshakes rejected in O(1))",
    };
    out.push_str(&format!("  Trust State:  {}\n", state_str));
    out
}

/// Renders a table of causal Merkle-DAG group conversations.
pub fn render_groups_table(
    groups: &[(&str, &str, u64, usize, usize)],
) -> String {
    if groups.is_empty() {
        return "\nNo active group conversations found.\n".to_string();
    }
    let mut out = String::new();
    out.push_str("\nCausal DAG Group Conversations:\n");
    out.push_str(&format!(
        "{:<20} {:<24} {:<8} {:<10} {:<10}\n",
        "GROUP NAME", "GROUP ID", "EPOCH", "MEMBERS", "MESSAGES"
    ));
    out.push_str(&format!(
        "{:-<20} {:-<24} {:-<8} {:-<10} {:-<10}\n",
        "", "", "", "", ""
    ));
    for (id, name, epoch, members, msgs) in groups {
        let short_id = if id.len() > 22 {
            format!("{}…", &id[..21])
        } else {
            id.to_string()
        };
        out.push_str(&format!(
            "{:<20} {:<24} {:<8} {:<10} {:<10}\n",
            name, short_id, epoch, members, msgs
        ));
    }
    out
}

/// Renders detailed causal DAG state for a group.
pub fn render_group_detail(state: &lanflare_core::chat::group::GroupDagState) -> String {
    let mut out = String::new();
    let roster = state.roster();
    out.push_str(&format!("\nGroup Details: {}\n", roster.group_name));
    out.push_str(&format!("  Group ID:       {}\n", roster.group_id));
    out.push_str(&format!("  Current Epoch:  {}\n", state.current_epoch()));
    out.push_str(&format!("  Total Nodes:    {}\n", state.len()));
    out.push_str(&format!("  Frontier Heads: {}\n", state.heads().len()));
    out.push_str("\nMember Roster:\n");
    for m in &roster.members {
        out.push_str(&format!(
            "  - {} ({}) [Role: {:?}]\n",
            m.alias,
            shorten_fingerprint(&m.fingerprint),
            m.role
        ));
    }
    out.push_str("\nRecent Causal Messages (Topologically Sorted):\n");
    let sorted_node_ids = state.topological_sort();
    if sorted_node_ids.is_empty() {
        out.push_str("  (No messages authored yet)\n");
    } else {
        for (i, msg_id) in sorted_node_ids.iter().rev().take(10).enumerate() {
            if let Some(node) = state.get_node(msg_id) {
                let author_alias = roster
                    .get_member(&node.author_fp)
                    .map(|m| m.alias.as_str())
                    .unwrap_or("Unknown");
                let msg_short = if node.msg_id.len() > 8 {
                    &node.msg_id[..8]
                } else {
                    &node.msg_id
                };
                out.push_str(&format!(
                    "  [{}] {} ({}) @ HLC(wall: {}ms, counter: {}): {} parents, msg_id: {}\n",
                    i + 1,
                    author_alias,
                    shorten_fingerprint(&node.author_fp),
                    node.hlc.wall_time_ms,
                    node.hlc.counter,
                    node.parent_ids.len(),
                    msg_short
                ));
            }
        }
    }
    out
}

/// Formats file size in human readable format (KB, MB, GB).
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Renders federated search results table ranked by relevance.
pub fn render_search_results_table(
    query: &str,
    results: &[lanflare_proto::toolbox::FileSearchResultDto],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("\nFederated File Search Results for \"{}\":\n", query));

    struct RankedHit<'a> {
        node_alias: &'a str,
        hit: &'a lanflare_proto::toolbox::SearchHitDto,
    }

    let mut all_hits = Vec::new();
    for res in results {
        for hit in &res.hits {
            all_hits.push(RankedHit {
                node_alias: &res.responder_alias,
                hit,
            });
        }
    }

    all_hits.sort_by(|a, b| b.hit.score.partial_cmp(&a.hit.score).unwrap_or(std::cmp::Ordering::Equal));

    if all_hits.is_empty() {
        out.push_str("  No matching files found across local catalog or discovered peers.\n");
        return out;
    }

    out.push_str(&format!(
        "{:<4} {:<24} {:<10} {:<16} {:<8} {:<32}\n",
        "#", "FILE NAME", "SIZE", "OWNER / NODE", "SCORE", "DOWNLOAD / STREAM URL"
    ));
    out.push_str(&format!(
        "{:-<4} {:-<24} {:-<10} {:-<16} {:-<8} {:-<32}\n",
        "", "", "", "", "", ""
    ));

    for (i, item) in all_hits.iter().enumerate() {
        let name = if item.hit.file_name.len() > 23 {
            format!("{}…", &item.hit.file_name[..22])
        } else {
            item.hit.file_name.clone()
        };
        let size_str = format_size(item.hit.size_bytes);
        let owner = if item.node_alias.len() > 15 {
            format!("{}…", &item.node_alias[..14])
        } else {
            item.node_alias.to_string()
        };
        let score_str = format!("{:.2}", item.hit.score);
        let url = if item.hit.download_url.len() > 32 {
            format!("{}…", &item.hit.download_url[..31])
        } else {
            item.hit.download_url.clone()
        };

        out.push_str(&format!(
            "{:<4} {:<24} {:<10} {:<16} {:<8} {:<32}\n",
            i + 1, name, size_str, owner, score_str, url
        ));
    }

    out.push_str(&format!("\nTotal Results: {} hits\n", all_hits.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanflare_core::discovery::registry::PeerEndpoint;
    use lanflare_core::discovery::DiscoverySource;
    use std::collections::{HashMap, HashSet};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_device_icon_resolution() {
        assert_eq!(get_device_icon(Some(DeviceType::Desktop), Some("Linux 6.8")), "🐧");
        assert_eq!(get_device_icon(Some(DeviceType::Desktop), Some("Windows 11")), "🪟 ");
        assert_eq!(get_device_icon(Some(DeviceType::Desktop), Some("macOS Sonoma")), "🍎");
        assert_eq!(get_device_icon(Some(DeviceType::Mobile), Some("Pixel 8 Android")), "🤖");
        assert_eq!(get_device_icon(Some(DeviceType::Mobile), None), "📱");
        assert_eq!(get_device_icon(Some(DeviceType::Server), None), "🖥️ ");
        assert_eq!(get_device_icon(None, None), "💻");
    }

    #[test]
    fn test_shorten_fingerprint() {
        let fp = "ABCD1234EF567890ABCD1234EF567890ABCD1234EF567890ABCD1234EF567890";
        assert_eq!(shorten_fingerprint(fp), "ABCD1234...567890");
        assert_eq!(shorten_fingerprint("SHORT"), "SHORT");
    }

    #[test]
    fn test_render_peer_table_empty() {
        let out = render_peer_table(&[]);
        assert!(out.contains("No peers discovered"));
    }

    #[test]
    fn test_render_peer_table_with_peers() {
        let mut sources = HashSet::new();
        sources.insert(DiscoverySource::Multicast);

        let peer = DiscoveredPeer {
            fingerprint: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".to_string(),
            alias: "Alice's Laptop".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Linux 6.8".to_string()),
            device_type: Some(DeviceType::Desktop),
            port: 53317,
            protocol: ProtocolType::Https,
            download: false,
            endpoints: vec![PeerEndpoint {
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)),
                scope_id: None,
                sources: sources.clone(),
                first_seen_ms: 1000,
                last_seen_ms: 1000,
            }],
            discovery_sources: sources,
            first_seen_ms: 1000,
            last_seen_ms: 1000,
            metadata: HashMap::new(),
            hop_count: None,
            route_path: None,
        };

        let table = render_peer_table(&[peer]);
        assert!(table.contains("Alice's Laptop"));
        assert!(table.contains("192.168.1.100"));
        assert!(table.contains("53317"));
        assert!(table.contains("HTTPS (TLS)"));
        assert!(table.contains("11223344...DDEEFF"));
    }

    #[test]
    fn test_transfer_progress_tracker_lifecycle() {
        let mut tracker = TransferProgressTracker::new(false);
        tracker.handle_event(&TransferProgress::Started {
            session_id: "sess-1".to_string(),
            accepted_files: vec!["file-1".to_string()],
            total_bytes: 1000,
        });

        tracker.handle_event(&TransferProgress::FileProgress {
            session_id: "sess-1".to_string(),
            file_id: "file-1".to_string(),
            file_name: "test.txt".to_string(),
            file_bytes_sent: 500,
            file_total_bytes: 1000,
            session_bytes_sent: 500,
            session_total_bytes: 1000,
            bytes_per_second: 5000,
            eta: Some(std::time::Duration::from_secs(1)),
        });

        tracker.handle_event(&TransferProgress::FileFinished {
            session_id: "sess-1".to_string(),
            file_id: "file-1".to_string(),
            file_name: "test.txt".to_string(),
        });

        tracker.handle_event(&TransferProgress::SessionCompleted {
            session_id: "sess-1".to_string(),
            total_files: 1,
            total_bytes: 1000,
            duration: std::time::Duration::from_millis(200),
        });
    }

    #[test]
    fn test_transfer_progress_tracker_error_and_cancellation() {
        let mut tracker = TransferProgressTracker::new(false);
        tracker.handle_event(&TransferProgress::FileFailed {
            session_id: "sess-2".to_string(),
            file_id: "file-2".to_string(),
            file_name: "fail.bin".to_string(),
            error: "connection reset".to_string(),
        });

        tracker.handle_event(&TransferProgress::SessionCancelled {
            session_id: "sess-2".to_string(),
            reason: "user aborted".to_string(),
        });
    }

    #[test]
    fn test_render_mesh_status() {
        let local_fp = "1111222233334444555566667777888811112222333344445555666677778888";
        let remote_fp = "AAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDD";
        let mut graph = lanflare_core::mesh::graph::MeshTopologyGraph::new(local_fp);
        let node_a = lanflare_core::mesh::graph::MeshNode::new(local_fp, "Node A", true);
        let node_b = lanflare_core::mesh::graph::MeshNode::new(remote_fp, "Node B", false);
        graph.insert_node(node_a);
        graph.insert_node(node_b);
        graph.add_bidirectional_link(local_fp, remote_fp, 20, 1000);

        let out = render_mesh_status("Node A", local_fp, true, &graph);
        assert!(out.contains("LanFlare Mesh Overlay Status"));
        assert!(out.contains("Node A"));
        assert!(out.contains("Node B"));
        assert!(out.contains("Direct"));
    }

    #[test]
    fn test_render_traceroute_and_speedtest() {
        let trace = lanflare_proto::mesh::MeshTracerouteResponse {
            trace_id: "trace-999".to_string(),
            origin_fp: "1111222233334444555566667777888811112222333344445555666677778888".to_string(),
            target_fp: "AAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDD".to_string(),
            success: true,
            total_hops: 2,
            total_rtt_ms: 35,
            hops: vec![
                lanflare_proto::mesh::TracerouteHop {
                    node_fp: "1111222233334444555566667777888811112222333344445555666677778888".to_string(),
                    alias: "Node A".to_string(),
                    rtt_ms: 0,
                    ingress_timestamp_ms: 1000,
                },
                lanflare_proto::mesh::TracerouteHop {
                    node_fp: "AAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDDAAAABBBBCCCCDDDD".to_string(),
                    alias: "Node B".to_string(),
                    rtt_ms: 35,
                    ingress_timestamp_ms: 1035,
                },
            ],
            error: None,
        };
        let out_trace = render_traceroute_table("Node B", &trace.target_fp, &trace);
        assert!(out_trace.contains("Traceroute to Node B"));
        assert!(out_trace.contains("Destination Reached"));
        assert!(out_trace.contains("35 ms"));

        let bench = lanflare_proto::mesh::SpeedtestResultDto {
            test_id: "test-123".to_string(),
            origin_fp: trace.origin_fp.clone(),
            target_fp: trace.target_fp.clone(),
            duration_ms: 50,
            total_bytes: 1024 * 1024,
            throughput_mbps: 163.84,
            chunks_received: 8,
        };
        let out_bench = render_speedtest_result("Node B", &bench.target_fp, &bench);
        assert!(out_bench.contains("Mesh Throughput Benchmark Result"));
        assert!(out_bench.contains("163.84 Mbps"));
    }

    #[test]
    fn test_render_contact_trust_and_search_results() {
        let trust_out = render_contact_trust_status(
            "Eve",
            "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF",
            lanflare_core::chat::trust::ContactTrustState::Stranger,
        );
        assert!(trust_out.contains("Eve"));
        assert!(trust_out.contains("Stranger"));

        let hit = lanflare_proto::toolbox::SearchHitDto {
            file_name: "blueprint.pdf".to_string(),
            relative_path: "docs/blueprint.pdf".to_string(),
            size_bytes: 2048576,
            modified_at_ms: 1000,
            mime_type: "application/pdf".to_string(),
            sha256: None,
            requires_pin: false,
            download_url: "http://127.0.0.1:53317/dufs/docs/blueprint.pdf".to_string(),
            score: 0.95,
        };
        let res = lanflare_proto::toolbox::FileSearchResultDto {
            responder_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".to_string(),
            responder_alias: "Bob's Workstation".to_string(),
            hits: vec![hit],
            total_indexed: 1,
            search_duration_ms: 5,
        };
        let search_out = render_search_results_table("blueprint", &[res]);
        assert!(search_out.contains("blueprint.pdf"));
        assert!(search_out.contains("Bob's Workstat"));
        assert!(search_out.contains("1 hits"));
    }
}
