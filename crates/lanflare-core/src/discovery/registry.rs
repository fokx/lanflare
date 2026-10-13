//! Peer registry and deduplication store.
//!
//! Provides a unified, thread-safe repository of all active peers discovered across
//! the network through UDP Multicast, mDNS (DNS-SD), HTTP registration, or manual IP scanning.
//!
//! Key responsibilities:
//! - **Deduplication**: Peers are keyed and unified by their device identifier / TLS certificate SHA-256 fingerprint.
//! - **Address Aggregation**: Consolidates multiple IP addresses (IPv4 / IPv6) discovered across different network adapters or protocols.
//! - **Discovery Channel Tracking**: Tracks which discovery channels (Multicast, mDNS, HTTP) observed each peer.
//! - **Liveness & Heartbeat Tracking**: Keeps track of `first_seen` and `last_seen` timestamps per peer and per address.
//! - **Stale Peer Eviction**: Background task or manual sweeping to detect inactive peers based on configurable timeouts.
//! - **Reactivity**: Emits fine-grained `RegistryEvent`s over Tokio broadcast channels for UI/TUI listeners and transfer dispatchers.

use lanflare_proto::discovery::{DeviceType, MulticastMessageV2, ProtocolType, RegisterDtoV2};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tokio_util::sync::CancellationToken;
use crate::discovery::mdns::DiscoveredMdnsPeer;

/// Discovery channel or source through which a peer or address was observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoverySource {
    /// LocalSend UDP Multicast (224.0.0.167 / ff12::fd3a:e420).
    Multicast,
    /// Modern zero-config mDNS / DNS-SD (_lanflare._tcp.local.).
    Mdns,
    /// Direct HTTP registration endpoint (/api/localsend/v2/register).
    HttpRegister,
    /// Manual IP subnet scanner or user-configured static address.
    ManualScan,
}

impl std::fmt::Display for DiscoverySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Multicast => write!(f, "multicast"),
            Self::Mdns => write!(f, "mdns"),
            Self::HttpRegister => write!(f, "http_register"),
            Self::ManualScan => write!(f, "manual_scan"),
        }
    }
}

/// Information about a specific network endpoint (IP address) associated with a peer.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PeerEndpoint {
    /// Network IP address.
    pub ip: IpAddr,

    /// Optional IPv6 scope identifier if link-local.
    pub scope_id: Option<u32>,

    /// Discovery channels that reported this IP address.
    pub sources: HashSet<DiscoverySource>,

    /// Timestamp in milliseconds since UNIX epoch when this address was first observed.
    pub first_seen_ms: u64,

    /// Timestamp in milliseconds since UNIX epoch when this address was last observed.
    pub last_seen_ms: u64,
}

impl PeerEndpoint {
    pub fn new(ip: IpAddr, scope_id: Option<u32>, source: DiscoverySource, now_ms: u64) -> Self {
        let mut sources = HashSet::new();
        sources.insert(source);
        Self {
            ip,
            scope_id,
            sources,
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
        }
    }

    pub fn touch(&mut self, source: DiscoverySource, now_ms: u64) {
        self.sources.insert(source);
        self.last_seen_ms = now_ms;
    }
}

/// Unified representation of a discovered remote peer.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DiscoveredPeer {
    /// SHA-256 fingerprint of the peer's TLS certificate (or unique session identifier).
    /// Used as the canonical identity key. Formatted as uppercase 64-char hex string.
    pub fingerprint: String,

    /// Human-readable device alias (e.g. "Bob's MacBook").
    pub alias: String,

    /// Protocol version string (e.g. "2.0").
    pub version: String,

    /// Hardware or OS model name (e.g. "MacBookPro18,1", "Linux").
    pub device_model: Option<String>,

    /// Categorical device type (e.g. Desktop, Mobile).
    pub device_type: Option<DeviceType>,

    /// HTTP/HTTPS server port.
    pub port: u16,

    /// Transport protocol (`http` or `https`).
    pub protocol: ProtocolType,

    /// Whether download API is supported.
    pub download: bool,

    /// List of known IP endpoints for this peer, sorted deterministically (IPv4 first, then IPv6).
    pub endpoints: Vec<PeerEndpoint>,

    /// Consolidated set of all discovery channels that observed this peer.
    pub discovery_sources: HashSet<DiscoverySource>,

    /// Timestamp in milliseconds since UNIX epoch when this peer was first discovered.
    pub first_seen_ms: u64,

    /// Timestamp in milliseconds since UNIX epoch of the latest packet/heartbeat received from this peer.
    pub last_seen_ms: u64,

    /// Arbitrary metadata attributes (e.g. mDNS TXT properties).
    pub metadata: HashMap<String, String>,
}

impl DiscoveredPeer {
    /// Returns the primary IP address to connect to.
    /// Prefers the most recently active IPv4 address, followed by IPv6.
    pub fn primary_ip(&self) -> Option<IpAddr> {
        // First try IPv4 sorted by last_seen_ms descending
        if let Some(v4) = self
            .endpoints
            .iter()
            .filter(|ep| ep.ip.is_ipv4())
            .max_by_key(|ep| ep.last_seen_ms)
        {
            return Some(v4.ip);
        }

        // Fall back to IPv6 sorted by last_seen_ms descending
        self.endpoints
            .iter()
            .filter(|ep| ep.ip.is_ipv6())
            .max_by_key(|ep| ep.last_seen_ms)
            .map(|ep| ep.ip)
    }

    /// Formats the primary base URL (e.g. "https://192.168.1.50:53317").
    pub fn primary_base_url(&self) -> Option<String> {
        let ip = self.primary_ip()?;
        let proto = match self.protocol {
            ProtocolType::Http => "http",
            ProtocolType::Https => "https",
        };
        match ip {
            IpAddr::V4(v4) => Some(format!("{proto}://{v4}:{}", self.port)),
            IpAddr::V6(v6) => Some(format!("{proto}://[{v6}]:{}", self.port)),
        }
    }

    /// Checks if this peer has been observed on a specific discovery channel.
    pub fn has_source(&self, source: DiscoverySource) -> bool {
        self.discovery_sources.contains(&source)
    }

    /// Returns all IP addresses known for this peer.
    pub fn addresses(&self) -> Vec<IpAddr> {
        self.endpoints.iter().map(|ep| ep.ip).collect()
    }
}

/// Events emitted when peer state changes in the registry.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RegistryEvent {
    /// A new peer was discovered for the first time.
    PeerAdded(DiscoveredPeer),

    /// An existing peer's details (alias, endpoints, metadata) were updated.
    PeerUpdated {
        /// Updated peer state.
        peer: DiscoveredPeer,
        /// What changed (e.g. "addresses", "metadata", "heartbeat").
        reason: PeerUpdateReason,
    },

    /// A peer was evicted after exceeding the inactivity timeout.
    PeerExpired {
        /// Fingerprint of the expired peer.
        fingerprint: String,
        /// Last known alias.
        alias: String,
    },

    /// A peer was manually removed from the registry.
    PeerRemoved {
        /// Fingerprint of the removed peer.
        fingerprint: String,
    },
}

/// Specific reason why a `PeerUpdated` event was generated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerUpdateReason {
    /// New IP endpoint or interface discovered.
    NewEndpoint,
    /// Heartbeat refreshed / last seen timestamp updated.
    Heartbeat,
    /// Profile metadata changed (alias, model, protocol, port).
    MetadataChanged,
    /// Discovered via a new discovery channel.
    NewDiscoverySource,
}

/// Configuration options for the peer registry.
#[derive(Clone, Debug)]
pub struct PeerRegistryConfig {
    /// Duration of silence before a peer is considered stale and evicted.
    /// Default: 60 seconds.
    pub peer_timeout: Duration,

    /// Interval between automatic background eviction sweeps.
    /// Default: 5 seconds.
    pub eviction_interval: Duration,

    /// Maximum capacity of peers to track simultaneously.
    /// Default: 256.
    pub max_peers: usize,

    /// Capacity for the broadcast event channel buffer.
    /// Default: 128.
    pub event_buffer_capacity: usize,
}

impl Default for PeerRegistryConfig {
    fn default() -> Self {
        Self {
            peer_timeout: Duration::from_secs(60),
            eviction_interval: Duration::from_secs(5),
            max_peers: 256,
            event_buffer_capacity: 128,
        }
    }
}

/// Helper to get current monotonic/unix epoch time in milliseconds.
fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Canonicalizes a certificate fingerprint to uppercase hex with whitespace stripped.
pub fn canonicalize_fingerprint(fp: &str) -> String {
    fp.trim().to_ascii_uppercase()
}

/// Sorts endpoints deterministically (IPv4 first, then IPv6, by IP).
fn sort_endpoints(endpoints: &mut [PeerEndpoint]) {
    endpoints.sort_by(|a, b| match (a.ip, b.ip) {
        (IpAddr::V4(a_v4), IpAddr::V4(b_v4)) => a_v4.cmp(&b_v4),
        (IpAddr::V4(_), IpAddr::V6(_)) => std::cmp::Ordering::Less,
        (IpAddr::V6(_), IpAddr::V4(_)) => std::cmp::Ordering::Greater,
        (IpAddr::V6(a_v6), IpAddr::V6(b_v6)) => a_v6.cmp(&b_v6),
    });
}

/// Central, thread-safe registry aggregating discovered network peers.
pub struct PeerRegistry {
    /// Internal peer map indexed by canonical uppercase fingerprint.
    peers: RwLock<HashMap<String, DiscoveredPeer>>,

    /// Fast peer count cache.
    count: AtomicUsize,

    /// Configuration parameters.
    config: PeerRegistryConfig,

    /// Broadcast sender for registry change events.
    event_tx: broadcast::Sender<RegistryEvent>,

    /// Cancellation token for background cleanup tasks.
    cancel: CancellationToken,
}

impl PeerRegistry {
    /// Creates a new `PeerRegistry` with default configuration.
    pub fn new() -> Arc<Self> {
        Self::with_config(PeerRegistryConfig::default())
    }

    /// Creates a new `PeerRegistry` with custom configuration and starts background eviction.
    pub fn with_config(config: PeerRegistryConfig) -> Arc<Self> {
        let (event_tx, _) = broadcast::channel(config.event_buffer_capacity);
        let registry = Arc::new(Self {
            peers: RwLock::new(HashMap::new()),
            count: AtomicUsize::new(0),
            config: config.clone(),
            event_tx,
            cancel: CancellationToken::new(),
        });

        // Spawn background eviction loop if called within an active Tokio runtime context
        if tokio::runtime::Handle::try_current().is_ok() {
            let reg_weak = Arc::downgrade(&registry);
            let cancel = registry.cancel.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(config.eviction_interval);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = interval.tick() => {
                            if let Some(reg) = reg_weak.upgrade() {
                                reg.sweep_stale_peers().await;
                            } else {
                                break;
                            }
                        }
                    }
                }
            });
        }

        registry
    }

    /// Explicitly starts the background eviction task if not already running.
    pub fn start_eviction_task(self: &Arc<Self>) {
        let reg_weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        let interval_duration = self.config.eviction_interval;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => {
                        if let Some(reg) = reg_weak.upgrade() {
                            reg.sweep_stale_peers().await;
                        } else {
                            break;
                        }
                    }
                }
            }
        });
    }

    /// Subscribes to peer discovery, update, expiration, and removal events.
    pub fn subscribe(&self) -> broadcast::Receiver<RegistryEvent> {
        self.event_tx.subscribe()
    }

    /// Returns the current number of active peers.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Checks if a peer with the given fingerprint is present in the registry.
    pub async fn contains(&self, fingerprint: &str) -> bool {
        let key = canonicalize_fingerprint(fingerprint);
        let peers = self.peers.read().await;
        peers.contains_key(&key)
    }

    /// Retrieves a cloned copy of a peer by its fingerprint.
    pub async fn get(&self, fingerprint: &str) -> Option<DiscoveredPeer> {
        let key = canonicalize_fingerprint(fingerprint);
        let peers = self.peers.read().await;
        peers.get(&key).cloned()
    }

    /// Returns a snapshot of all active peers currently registered.
    pub async fn get_all(&self) -> Vec<DiscoveredPeer> {
        let peers = self.peers.read().await;
        peers.values().cloned().collect()
    }

    /// Manually inserts or merges an arbitrary `DiscoveredPeer`.
    pub async fn insert_or_update(&self, peer: DiscoveredPeer) -> Option<DiscoveredPeer> {
        let now_ms = current_time_ms();
        self.merge_peer_internal(peer, now_ms).await
    }

    /// Integrates a UDP multicast announcement or reply packet.
    pub async fn handle_multicast(
        &self,
        remote_ip: IpAddr,
        scope_id: Option<u32>,
        msg: &MulticastMessageV2,
    ) -> Option<DiscoveredPeer> {
        let now_ms = current_time_ms();
        let fingerprint = canonicalize_fingerprint(&msg.fingerprint);
        if fingerprint.is_empty() {
            return None;
        }

        let mut sources = HashSet::new();
        sources.insert(DiscoverySource::Multicast);

        let endpoint = PeerEndpoint {
            ip: remote_ip,
            scope_id,
            sources: sources.clone(),
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
        };

        let candidate = DiscoveredPeer {
            fingerprint,
            alias: msg.alias.clone(),
            version: msg.version.clone(),
            device_model: msg.device_model.clone(),
            device_type: msg.device_type,
            port: msg.port,
            protocol: msg.protocol,
            download: msg.download,
            endpoints: vec![endpoint],
            discovery_sources: sources,
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            metadata: HashMap::new(),
        };

        self.merge_peer_internal(candidate, now_ms).await
    }

    /// Integrates an mDNS peer resolution event.
    pub async fn handle_mdns(&self, mdns_peer: &DiscoveredMdnsPeer) -> Option<DiscoveredPeer> {
        let now_ms = current_time_ms();
        let fingerprint = canonicalize_fingerprint(&mdns_peer.fingerprint);
        if fingerprint.is_empty() {
            return None;
        }

        let mut sources = HashSet::new();
        sources.insert(DiscoverySource::Mdns);

        let endpoints: Vec<PeerEndpoint> = mdns_peer
            .addresses
            .iter()
            .map(|&ip| PeerEndpoint {
                ip,
                scope_id: None,
                sources: sources.clone(),
                first_seen_ms: now_ms,
                last_seen_ms: now_ms,
            })
            .collect();

        let candidate = DiscoveredPeer {
            fingerprint,
            alias: mdns_peer.alias.clone(),
            version: mdns_peer.version.clone(),
            device_model: mdns_peer.device_model.clone(),
            device_type: mdns_peer.device_type,
            port: mdns_peer.port,
            protocol: mdns_peer.protocol,
            download: mdns_peer.download,
            endpoints,
            discovery_sources: sources,
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            metadata: mdns_peer.txt_records.clone(),
        };

        self.merge_peer_internal(candidate, now_ms).await
    }

    /// Integrates a peer discovered via direct HTTP `/api/localsend/v2/register`.
    pub async fn handle_http_register(
        &self,
        remote_ip: IpAddr,
        dto: &RegisterDtoV2,
    ) -> Option<DiscoveredPeer> {
        let now_ms = current_time_ms();
        let fingerprint = canonicalize_fingerprint(&dto.fingerprint);
        if fingerprint.is_empty() {
            return None;
        }

        let mut sources = HashSet::new();
        sources.insert(DiscoverySource::HttpRegister);

        let endpoint = PeerEndpoint {
            ip: remote_ip,
            scope_id: None,
            sources: sources.clone(),
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
        };

        let candidate = DiscoveredPeer {
            fingerprint,
            alias: dto.alias.clone(),
            version: dto.version.clone(),
            device_model: dto.device_model.clone(),
            device_type: dto.device_type,
            port: dto.port,
            protocol: dto.protocol,
            download: dto.download,
            endpoints: vec![endpoint],
            discovery_sources: sources,
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            metadata: HashMap::new(),
        };

        self.merge_peer_internal(candidate, now_ms).await
    }

    /// Internal merge routine consolidating peer endpoints and attributes.
    async fn merge_peer_internal(
        &self,
        incoming: DiscoveredPeer,
        now_ms: u64,
    ) -> Option<DiscoveredPeer> {
        let key = incoming.fingerprint.clone();
        let mut peers = self.peers.write().await;

        if let Some(existing) = peers.get_mut(&key) {
            let mut reason = PeerUpdateReason::Heartbeat;

            // Update heartbeat timestamp
            existing.last_seen_ms = now_ms;

            // Merge discovery channels
            for src in &incoming.discovery_sources {
                if existing.discovery_sources.insert(*src) {
                    reason = PeerUpdateReason::NewDiscoverySource;
                }
            }

            // Update mutable metadata if changed
            if existing.alias != incoming.alias {
                existing.alias = incoming.alias;
                reason = PeerUpdateReason::MetadataChanged;
            }
            if incoming.device_model.is_some() && existing.device_model != incoming.device_model {
                existing.device_model = incoming.device_model;
                reason = PeerUpdateReason::MetadataChanged;
            }
            if incoming.device_type.is_some() && existing.device_type != incoming.device_type {
                existing.device_type = incoming.device_type;
                reason = PeerUpdateReason::MetadataChanged;
            }
            if existing.port != incoming.port {
                existing.port = incoming.port;
                reason = PeerUpdateReason::MetadataChanged;
            }
            if existing.protocol != incoming.protocol {
                existing.protocol = incoming.protocol;
                reason = PeerUpdateReason::MetadataChanged;
            }
            if existing.download != incoming.download {
                existing.download = incoming.download;
                reason = PeerUpdateReason::MetadataChanged;
            }

            // Merge metadata map
            for (k, v) in incoming.metadata {
                existing.metadata.insert(k, v);
            }

            // Merge IP endpoints
            for inc_ep in incoming.endpoints {
                if let Some(existing_ep) = existing.endpoints.iter_mut().find(|ep| ep.ip == inc_ep.ip) {
                    existing_ep.last_seen_ms = now_ms;
                    for s in inc_ep.sources {
                        existing_ep.sources.insert(s);
                    }
                    if inc_ep.scope_id.is_some() && existing_ep.scope_id != inc_ep.scope_id {
                        existing_ep.scope_id = inc_ep.scope_id;
                    }
                } else {
                    existing.endpoints.push(inc_ep);
                    sort_endpoints(&mut existing.endpoints);
                    reason = PeerUpdateReason::NewEndpoint;
                }
            }

            let updated_clone = existing.clone();
            // Emit update event
            let _ = self.event_tx.send(RegistryEvent::PeerUpdated {
                peer: updated_clone.clone(),
                reason,
            });

            Some(updated_clone)
        } else {
            // New peer insertion
            if peers.len() >= self.config.max_peers {
                // Evict oldest peer to respect max_peers limit
                let oldest_key = peers
                    .iter()
                    .min_by_key(|(_, p)| p.last_seen_ms)
                    .map(|(k, _)| k.clone());
                if let Some(evicted) = oldest_key.and_then(|k| peers.remove(&k)) {
                    let _ = self.event_tx.send(RegistryEvent::PeerExpired {
                        fingerprint: evicted.fingerprint,
                        alias: evicted.alias,
                    });
                }
            }

            let mut new_peer = incoming;
            sort_endpoints(&mut new_peer.endpoints);
            peers.insert(key, new_peer.clone());
            self.count.store(peers.len(), Ordering::Relaxed);

            let _ = self.event_tx.send(RegistryEvent::PeerAdded(new_peer.clone()));
            Some(new_peer)
        }
    }

    /// Manually removes a peer from the registry.
    pub async fn remove(&self, fingerprint: &str) -> Option<DiscoveredPeer> {
        let key = canonicalize_fingerprint(fingerprint);
        let mut peers = self.peers.write().await;
        let removed = peers.remove(&key);
        if let Some(ref peer) = removed {
            self.count.store(peers.len(), Ordering::Relaxed);
            let _ = self.event_tx.send(RegistryEvent::PeerRemoved {
                fingerprint: peer.fingerprint.clone(),
            });
        }
        removed
    }

    /// Removes peers whose `last_seen_ms` exceeds the configured `peer_timeout`.
    /// Returns the number of evicted peers.
    pub async fn sweep_stale_peers(&self) -> usize {
        let now_ms = current_time_ms();
        let timeout_ms = self.config.peer_timeout.as_millis() as u64;

        let mut to_evict = Vec::new();
        {
            let peers = self.peers.read().await;
            for (key, peer) in peers.iter() {
                if now_ms.saturating_sub(peer.last_seen_ms) > timeout_ms {
                    to_evict.push((key.clone(), peer.alias.clone()));
                }
            }
        }

        if to_evict.is_empty() {
            return 0;
        }

        let mut evicted_count = 0;
        let mut peers = self.peers.write().await;
        for (key, alias) in to_evict {
            if peers.remove(&key).is_some() {
                evicted_count += 1;
                let _ = self.event_tx.send(RegistryEvent::PeerExpired {
                    fingerprint: key,
                    alias,
                });
            }
        }

        self.count.store(peers.len(), Ordering::Relaxed);
        evicted_count
    }

    /// Clears all peers from the registry.
    pub async fn clear(&self) {
        let mut peers = self.peers.write().await;
        let keys: Vec<String> = peers.keys().cloned().collect();
        peers.clear();
        self.count.store(0, Ordering::Relaxed);
        for key in keys {
            let _ = self.event_tx.send(RegistryEvent::PeerRemoved {
                fingerprint: key,
            });
        }
    }

    /// Stops background eviction and cleans up tasks.
    pub fn shutdown(&self) {
        self.cancel.cancel();
    }
}

impl Drop for PeerRegistry {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn make_test_multicast_msg(fingerprint: &str, alias: &str, port: u16) -> MulticastMessageV2 {
        MulticastMessageV2 {
            alias: alias.to_string(),
            version: "2.0".to_string(),
            device_model: Some("Linux Box".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: fingerprint.to_string(),
            port,
            protocol: ProtocolType::Https,
            download: true,
            announce: true,
        }
    }

    #[tokio::test]
    async fn test_new_peer_multicast_discovery() {
        let registry = PeerRegistry::with_config(PeerRegistryConfig {
            peer_timeout: Duration::from_secs(10),
            eviction_interval: Duration::from_secs(10),
            max_peers: 10,
            event_buffer_capacity: 10,
        });

        let mut events = registry.subscribe();

        let msg = make_test_multicast_msg("ABCD1234FFFF", "Alice Device", 53317);
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));

        let peer = registry.handle_multicast(ip, None, &msg).await.unwrap();
        assert_eq!(peer.fingerprint, "ABCD1234FFFF");
        assert_eq!(peer.alias, "Alice Device");
        assert_eq!(peer.port, 53317);
        assert_eq!(peer.protocol, ProtocolType::Https);
        assert!(peer.download);
        assert_eq!(peer.endpoints.len(), 1);
        assert_eq!(peer.endpoints[0].ip, ip);
        assert!(peer.has_source(DiscoverySource::Multicast));

        // Event verified
        let event = events.recv().await.unwrap();
        match event {
            RegistryEvent::PeerAdded(p) => {
                assert_eq!(p.fingerprint, "ABCD1234FFFF");
                assert_eq!(p.alias, "Alice Device");
            }
            _ => panic!("Expected PeerAdded event"),
        }
    }

    #[tokio::test]
    async fn test_peer_deduplication_and_address_aggregation() {
        let registry = PeerRegistry::new();
        let mut events = registry.subscribe();

        let fp = "DEADBEEF0001";
        let ip_v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100));
        let ip_v6 = IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1));

        // 1. First discovered via Multicast on IPv4
        let msg = make_test_multicast_msg(fp, "Bob Laptop", 53317);
        registry.handle_multicast(ip_v4, None, &msg).await.unwrap();

        let ev1 = events.recv().await.unwrap();
        assert!(matches!(ev1, RegistryEvent::PeerAdded(_)));

        // 2. Discovered via mDNS with IPv4 + IPv6
        let mdns_peer = DiscoveredMdnsPeer {
            fullname: "Bob._lanflare._tcp.local.".to_string(),
            addresses: vec![ip_v4, ip_v6],
            port: 53317,
            hostname: "bob.local.".to_string(),
            alias: "Bob Laptop (Updated)".to_string(),
            version: "2.0".to_string(),
            device_model: Some("MacBookPro".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: fp.to_string(),
            protocol: ProtocolType::Https,
            download: true,
            txt_records: HashMap::from([("custom_tag".to_string(), "lanflare_test".to_string())]),
        };

        let merged = registry.handle_mdns(&mdns_peer).await.unwrap();

        // Should deduplicate to 1 entry
        assert_eq!(registry.count(), 1);
        assert_eq!(merged.fingerprint, fp);
        assert_eq!(merged.alias, "Bob Laptop (Updated)");
        assert_eq!(merged.device_model, Some("MacBookPro".to_string()));

        // Endpoints merged (both IPv4 and IPv6)
        assert_eq!(merged.endpoints.len(), 2);
        assert_eq!(merged.endpoints[0].ip, ip_v4); // sorted IPv4 first
        assert_eq!(merged.endpoints[1].ip, ip_v6);

        // Discovery sources combined
        assert!(merged.has_source(DiscoverySource::Multicast));
        assert!(merged.has_source(DiscoverySource::Mdns));

        // Metadata merged
        assert_eq!(
            merged.metadata.get("custom_tag"),
            Some(&"lanflare_test".to_string())
        );

        // Event verified
        let ev2 = events.recv().await.unwrap();
        match ev2 {
            RegistryEvent::PeerUpdated { peer, reason } => {
                assert_eq!(peer.fingerprint, fp);
                assert_eq!(peer.endpoints.len(), 2);
                assert!(matches!(
                    reason,
                    PeerUpdateReason::NewEndpoint
                        | PeerUpdateReason::NewDiscoverySource
                        | PeerUpdateReason::MetadataChanged
                ));
            }
            _ => panic!("Expected PeerUpdated event"),
        }
    }

    #[tokio::test]
    async fn test_case_insensitive_fingerprint_matching() {
        let registry = PeerRegistry::new();

        let msg_lower = make_test_multicast_msg("abcdef123456", "Node Lower", 53317);
        let msg_upper = make_test_multicast_msg("ABCDEF123456", "Node Upper", 53317);

        let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        registry.handle_multicast(ip, None, &msg_lower).await;
        registry.handle_multicast(ip, None, &msg_upper).await;

        assert_eq!(registry.count(), 1);
        let peer = registry.get("abcdef123456").await.unwrap();
        assert_eq!(peer.alias, "Node Upper");
        assert_eq!(peer.fingerprint, "ABCDEF123456");
    }

    #[tokio::test]
    async fn test_primary_ip_and_base_url() {
        let registry = PeerRegistry::new();
        let fp = "CAFE1234";
        let ip_v4 = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 55));
        let ip_v6 = IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1));

        let msg = make_test_multicast_msg(fp, "Dual Stack Node", 53317);
        registry.handle_multicast(ip_v6, None, &msg).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        registry.handle_multicast(ip_v4, None, &msg).await;

        let peer = registry.get(fp).await.unwrap();
        // Prefers IPv4
        assert_eq!(peer.primary_ip(), Some(ip_v4));
        assert_eq!(
            peer.primary_base_url(),
            Some("https://192.168.1.55:53317".to_string())
        );
    }

    #[tokio::test]
    async fn test_stale_peer_timeout_eviction() {
        let registry = PeerRegistry::with_config(PeerRegistryConfig {
            peer_timeout: Duration::from_millis(50),
            eviction_interval: Duration::from_millis(500), // manual sweep in test
            max_peers: 10,
            event_buffer_capacity: 10,
        });

        let mut events = registry.subscribe();

        let msg = make_test_multicast_msg("OLDPEER9999", "Old Node", 53317);
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)), None, &msg)
            .await;

        assert_eq!(registry.count(), 1);
        let _ = events.recv().await.unwrap(); // PeerAdded

        // Wait past timeout
        tokio::time::sleep(Duration::from_millis(70)).await;

        let evicted = registry.sweep_stale_peers().await;
        assert_eq!(evicted, 1);
        assert_eq!(registry.count(), 0);

        let ev2 = events.recv().await.unwrap();
        match ev2 {
            RegistryEvent::PeerExpired { fingerprint, alias } => {
                assert_eq!(fingerprint, "OLDPEER9999");
                assert_eq!(alias, "Old Node");
            }
            _ => panic!("Expected PeerExpired event"),
        }
    }

    #[tokio::test]
    async fn test_max_peers_capacity_eviction() {
        let registry = PeerRegistry::with_config(PeerRegistryConfig {
            peer_timeout: Duration::from_secs(60),
            eviction_interval: Duration::from_secs(60),
            max_peers: 2,
            event_buffer_capacity: 10,
        });

        let msg1 = make_test_multicast_msg("PEER1", "Node 1", 53317);
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), None, &msg1)
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        let msg2 = make_test_multicast_msg("PEER2", "Node 2", 53317);
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), None, &msg2)
            .await;
        tokio::time::sleep(Duration::from_millis(10)).await;

        assert_eq!(registry.count(), 2);

        // Inserting 3rd peer should evict oldest (PEER1)
        let msg3 = make_test_multicast_msg("PEER3", "Node 3", 53317);
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 3)), None, &msg3)
            .await;

        assert_eq!(registry.count(), 2);
        assert!(!registry.contains("PEER1").await);
        assert!(registry.contains("PEER2").await);
        assert!(registry.contains("PEER3").await);
    }

    #[tokio::test]
    async fn test_http_register_handling() {
        let registry = PeerRegistry::new();
        let dto = RegisterDtoV2 {
            alias: "Carol Web".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Chrome".to_string()),
            device_type: Some(DeviceType::Web),
            fingerprint: "WEB123456789".to_string(),
            port: 53318,
            protocol: ProtocolType::Http,
            download: false,
        };

        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 200));
        let peer = registry.handle_http_register(ip, &dto).await.unwrap();

        assert_eq!(peer.fingerprint, "WEB123456789");
        assert_eq!(peer.alias, "Carol Web");
        assert_eq!(peer.device_type, Some(DeviceType::Web));
        assert_eq!(peer.protocol, ProtocolType::Http);
        assert!(peer.has_source(DiscoverySource::HttpRegister));
    }

    #[tokio::test]
    async fn test_peer_manual_remove_and_clear() {
        let registry = PeerRegistry::new();
        let mut events = registry.subscribe();

        let msg1 = make_test_multicast_msg("PEER_REM_1", "Node A", 53317);
        let msg2 = make_test_multicast_msg("PEER_REM_2", "Node B", 53317);

        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), None, &msg1)
            .await;
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11)), None, &msg2)
            .await;

        assert_eq!(registry.count(), 2);
        let _ = events.recv().await.unwrap(); // PeerAdded
        let _ = events.recv().await.unwrap(); // PeerAdded

        // Remove single peer
        let removed = registry.remove("PEER_REM_1").await;
        assert!(removed.is_some());
        assert_eq!(registry.count(), 1);
        assert!(!registry.contains("PEER_REM_1").await);
        assert!(registry.contains("PEER_REM_2").await);

        let ev_rem = events.recv().await.unwrap();
        match ev_rem {
            RegistryEvent::PeerRemoved { fingerprint } => {
                assert_eq!(fingerprint, "PEER_REM_1");
            }
            _ => panic!("Expected PeerRemoved event"),
        }

        // Clear registry
        registry.clear().await;
        assert_eq!(registry.count(), 0);
        let ev_clr = events.recv().await.unwrap();
        match ev_clr {
            RegistryEvent::PeerRemoved { fingerprint } => {
                assert_eq!(fingerprint, "PEER_REM_2");
            }
            _ => panic!("Expected PeerRemoved event"),
        }
    }

    #[tokio::test]
    async fn test_background_eviction_task() {
        let registry = PeerRegistry::with_config(PeerRegistryConfig {
            peer_timeout: Duration::from_millis(60),
            eviction_interval: Duration::from_millis(20),
            max_peers: 10,
            event_buffer_capacity: 10,
        });

        let mut events = registry.subscribe();

        let msg = make_test_multicast_msg("AUTO_EVICT_1", "Quick Node", 53317);
        registry
            .handle_multicast(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), None, &msg)
            .await;

        assert_eq!(registry.count(), 1);
        let _ = events.recv().await.unwrap(); // PeerAdded

        // Wait for background eviction loop to automatically sweep
        tokio::time::sleep(Duration::from_millis(150)).await;

        assert_eq!(registry.count(), 0);
        let ev_exp = events.recv().await.unwrap();
        match ev_exp {
            RegistryEvent::PeerExpired { fingerprint, .. } => {
                assert_eq!(fingerprint, "AUTO_EVICT_1");
            }
            _ => panic!("Expected PeerExpired event from background task"),
        }
    }
}
