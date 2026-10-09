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
        };

        let table = render_peer_table(&[peer]);
        assert!(table.contains("Alice's Laptop"));
        assert!(table.contains("192.168.1.100"));
        assert!(table.contains("53317"));
        assert!(table.contains("HTTPS (TLS)"));
        assert!(table.contains("11223344...DDEEFF"));
    }
}
