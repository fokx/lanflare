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
}
