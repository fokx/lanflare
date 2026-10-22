//! Subnet scanner for fallback discovery.
//!
//! Scans a `/24` IPv4 subnet by probing candidate IP addresses concurrently
//! using HTTP/HTTPS `/api/localsend/v2/info` or `/register` to discover peers when
//! multicast / mDNS traffic is blocked by network routers or AP isolation.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use futures_util::stream::{self, StreamExt};
use lanflare_proto::discovery::ProtocolType;

use crate::client::{LsClient, LsClientConfig};
use crate::crypto::cert::TlsIdentity;
use crate::discovery::registry::{DiscoveredPeer, PeerRegistry};

/// Concurrency limit for parallel subnet probing.
pub const SCAN_CONCURRENCY: usize = 64;

/// Timeout for probing individual IPs during a subnet scan.
pub const SCAN_PROBE_TIMEOUT: Duration = Duration::from_millis(2500);

/// Connect timeout for probing individual IPs during a subnet scan.
pub const SCAN_CONNECT_TIMEOUT: Duration = Duration::from_millis(1200);

#[cfg(target_os = "linux")]
fn get_arp_neighbors(base_prefix: [u8; 3]) -> std::collections::HashSet<Ipv4Addr> {
    let mut arp_ips = std::collections::HashSet::new();
    if let Ok(content) = std::fs::read_to_string("/proc/net/arp") {
        for line in content.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                // flags: 0x2 means ATF_COM (completed ARP resolution)
                let flags = parts[2];
                if flags == "0x2" || flags == "2" {
                    if let Ok(ip) = parts[0].parse::<Ipv4Addr>() {
                        let oct = ip.octets();
                        if oct[0] == base_prefix[0] && oct[1] == base_prefix[1] && oct[2] == base_prefix[2] {
                            arp_ips.insert(ip);
                        }
                    }
                }
            }
        }
    }
    arp_ips
}

#[cfg(not(target_os = "linux"))]
fn get_arp_neighbors(_base_prefix: [u8; 3]) -> std::collections::HashSet<Ipv4Addr> {
    std::collections::HashSet::new()
}

/// Scans the `/24` subnet containing `interface_ip` and populates the given `PeerRegistry`.
///
/// Probes every host in `[1..=254]` on the given `port` (defaults to 53317).
/// Skips `interface_ip` itself and suppresses self-discovered certificates using `identity`.
/// Invokes `on_peer` immediately whenever a peer is discovered.
pub async fn scan_subnet_with_callback<F>(
    interface_ip: Ipv4Addr,
    port: u16,
    identity: &TlsIdentity,
    registry: &Arc<PeerRegistry>,
    on_peer: F,
) -> Vec<DiscoveredPeer>
where
    F: Fn(&DiscoveredPeer) + Send + Sync,
{
    let base = interface_ip.octets();
    let client_config = LsClientConfig {
        timeout: SCAN_PROBE_TIMEOUT,
        connect_timeout: SCAN_CONNECT_TIMEOUT,
        tls_identity: Some(identity.clone()),
        target_fingerprint: None, // TOFU unpinned probe
    };

    let client = match LsClient::new(client_config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to create client for subnet scanner: {e}");
            return Vec::new();
        }
    };

    let mut candidates: Vec<Ipv4Addr> = (1..=254u8)
        .map(|host| Ipv4Addr::new(base[0], base[1], base[2], host))
        .filter(|&ip| ip != interface_ip)
        .collect();

    // Prioritize active ARP neighbors so devices currently talking on the LAN are probed first
    let arp_neighbors = get_arp_neighbors([base[0], base[1], base[2]]);
    if !arp_neighbors.is_empty() {
        candidates.sort_by_key(|ip| !arp_neighbors.contains(ip));
    }

    let client_ref = &client;
    let own_fp = identity.fingerprint.clone();
    let reg_ref = registry;
    let on_peer_ref = &on_peer;

    let discovered_peers: Vec<DiscoveredPeer> = stream::iter(candidates)
        .map(|target_ip| {
            let own_fp = own_fp.clone();
            async move {
                let peer = probe_host(client_ref, target_ip, port, &own_fp, reg_ref).await;
                if let Some(ref p) = peer {
                    on_peer_ref(p);
                }
                peer
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await;

    discovered_peers
}

/// Scans the `/24` subnet containing `interface_ip` and populates the given `PeerRegistry`.
pub async fn scan_subnet(
    interface_ip: Ipv4Addr,
    port: u16,
    identity: &TlsIdentity,
    registry: &Arc<PeerRegistry>,
) -> Vec<DiscoveredPeer> {
    scan_subnet_with_callback(interface_ip, port, identity, registry, |_| {}).await
}

/// Probes a single host for a LocalSend v2.2 info endpoint.
///
/// Tries HTTPS first (the modern LocalSend default), then falls back to plain HTTP
/// only if the failure was TLS-related (e.g. peer does not use TLS). If the host
/// port is completely closed or timed out, HTTP is skipped immediately to avoid
/// doubling scan latency on dead hosts.
async fn probe_host(
    client: &LsClient,
    target_ip: Ipv4Addr,
    port: u16,
    own_fingerprint: &str,
    registry: &Arc<PeerRegistry>,
) -> Option<DiscoveredPeer> {
    let protocols = [ProtocolType::Https, ProtocolType::Http];

    for proto in protocols {
        let proto_str = match proto {
            ProtocolType::Https => "https",
            ProtocolType::Http => "http",
        };
        let base_url = format!("{proto_str}://{target_ip}:{port}");

        match client.get_info(&base_url).await {
            Ok(info) => {
                // Suppress self-discovery
                if !info.fingerprint.is_empty()
                    && info.fingerprint.eq_ignore_ascii_case(own_fingerprint)
                {
                    return None;
                }

                // Ingest into registry
                let peer = registry
                    .handle_info(IpAddr::V4(target_ip), port, proto, &info)
                    .await;
                return peer;
            }
            Err(err) => {
                // If TCP connection could not be established (connection refused or timeout),
                // trying plain HTTP on the exact same port is guaranteed to fail similarly.
                // Only retry HTTP if the failure occurred after TCP connected (e.g. TLS handshake).
                let should_retry_http = match &err {
                    crate::client::ClientError::Reqwest(re) => {
                        !re.is_connect() && !re.is_timeout()
                    }
                    crate::client::ClientError::Tls(_) => true,
                    _ => false,
                };

                if !should_retry_http {
                    return None;
                }
                continue;
            }
        }
    }

    None
}
