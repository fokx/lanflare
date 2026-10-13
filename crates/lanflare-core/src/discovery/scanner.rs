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
pub const SCAN_PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// Scans the `/24` subnet containing `interface_ip` and populates the given `PeerRegistry`.
///
/// Probes every host in `[1..=254]` on the given `port` (defaults to 53317).
/// Skips `interface_ip` itself and suppresses self-discovered certificates using `identity`.
pub async fn scan_subnet(
    interface_ip: Ipv4Addr,
    port: u16,
    identity: &TlsIdentity,
    registry: &Arc<PeerRegistry>,
) -> Vec<DiscoveredPeer> {
    let base = interface_ip.octets();
    let client_config = LsClientConfig {
        timeout: SCAN_PROBE_TIMEOUT,
        connect_timeout: Duration::from_millis(400),
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

    let candidates: Vec<Ipv4Addr> = (1..=254u8)
        .map(|host| Ipv4Addr::new(base[0], base[1], base[2], host))
        .filter(|&ip| ip != interface_ip)
        .collect();

    let client_ref = &client;
    let own_fp = identity.fingerprint.clone();
    let reg_ref = registry;

    let discovered_peers: Vec<DiscoveredPeer> = stream::iter(candidates)
        .map(|target_ip| {
            let own_fp = own_fp.clone();
            async move {
                probe_host(client_ref, target_ip, port, &own_fp, reg_ref).await
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .filter_map(std::future::ready)
        .collect()
        .await;

    discovered_peers
}

/// Probes a single host for a LocalSend v2.2 info endpoint.
///
/// Tries HTTPS first (the modern LocalSend default), then falls back to plain HTTP.
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
            Err(_) => {
                // Try next protocol
                continue;
            }
        }
    }

    None
}
