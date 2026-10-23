//! Multi-hop gossip engine, E2EE packet forwarder, and cut-through stream relay.
//!
//! Sub-phase 7.3 implementation:
//! - Mounts Hyper endpoints:
//!   - `POST /api/lanflare/v1/mesh/gossip` (exchange signed node announcements, link metrics, and route advertisements)
//!   - `GET  /api/lanflare/v1/mesh/topology` (export active topology graph)
//!   - `POST /api/lanflare/v1/mesh/forward` (multi-hop routed envelope relay)
//!   - `POST /api/lanflare/v1/mesh/stream` (cut-through chunked file streaming relay)
//! - Packet deduplication ring buffer (`seen_packet_ids`) with bounded capacity.
//! - TTL decrementing and loop detection on forwarded envelopes.
//! - Cut-through streaming file relay in `/api/lanflare/v1/mesh/stream` piping 128 KB chunks
//!   from inbound HTTP body directly to next-hop outbound HTTP stream with bounded memory
//!   (< 2 MB per stream) and configurable `relay_enabled` policy.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use lanflare_proto::mesh::{
    MailboxDepositRequest, MailboxDepositResponse, MailboxFetchRequest, MailboxFetchResponse,
    MeshGossipPacket, MeshLinkMetric, MeshNodeAnnouncement, MeshRouteAdvertisement,
    MeshStreamHeader, RoutedEnvelope, DEFAULT_STREAM_CHUNK_SIZE, MAX_MESH_TTL,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::mesh::graph::{MeshLink, MeshNode, MeshTopologyGraph};
use crate::mesh::mailbox::RelayMailbox;
use crate::server::dufs::{empty_box_response, full_box_body, json_response, ServerResponseBody};
use crate::server::router::ClientConnectionInfo;

/// Default capacity for the packet deduplication ring buffer (10,000 entries).
pub const DEFAULT_DEDUP_CAPACITY: usize = 10_000;

/// Default channel buffer capacity for cut-through stream chunks (16 chunks of 128 KB = 2 MB).
pub const DEFAULT_STREAM_BUFFER_CHUNKS: usize = 16;

/// Default request timeout for outbound relay requests (30 seconds).
pub const DEFAULT_RELAY_TIMEOUT_SECS: u64 = 30;

/// Maximum allowed JSON control body size (10 MB).
pub const MAX_CONTROL_BODY_SIZE: usize = 10 * 1024 * 1024;

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Errors returned by the mesh router during packet or stream forwarding.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("Duplicate packet ID: {0}")]
    DuplicatePacket(String),

    #[error("TTL expired (ttl = {0})")]
    TtlExpired(u8),

    #[error("Relay forwarding is disabled on this node")]
    RelayDisabled,

    #[error("No route to destination: {0}")]
    NoRoute(String),

    #[error("Unresolved endpoint for next-hop node: {0}")]
    UnresolvedEndpoint(String),

    #[error("Network relay error: {0}")]
    Network(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Stream transfer error: {0}")]
    StreamError(String),
}

/// Bounded FIFO/ring buffer for packet ID deduplication.
#[derive(Debug)]
pub struct PacketDedupBuffer {
    seen_set: HashSet<String>,
    queue: VecDeque<String>,
    capacity: usize,
}

impl PacketDedupBuffer {
    /// Creates a new packet deduplication buffer with the given capacity.
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(1);
        Self {
            seen_set: HashSet::with_capacity(cap),
            queue: VecDeque::with_capacity(cap),
            capacity: cap,
        }
    }

    /// Checks if a packet has already been seen. If already seen, returns `true`.
    /// Otherwise records the packet ID and returns `false`.
    pub fn check_and_insert(&mut self, packet_id: &str) -> bool {
        if self.seen_set.contains(packet_id) {
            return true;
        }
        if self.queue.len() >= self.capacity
            && let Some(old) = self.queue.pop_front()
        {
            self.seen_set.remove(&old);
        }
        self.seen_set.insert(packet_id.to_string());
        self.queue.push_back(packet_id.to_string());
        false
    }

    /// Checks whether the buffer currently contains the given packet ID.
    pub fn contains(&self, packet_id: &str) -> bool {
        self.seen_set.contains(packet_id)
    }

    /// Returns the number of tracked packet IDs.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if no packet IDs are tracked.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Clears all tracked packet IDs.
    pub fn clear(&mut self) {
        self.seen_set.clear();
        self.queue.clear();
    }
}

/// Configuration options for initializing a `MeshRouter`.
#[derive(Clone, Debug)]
pub struct MeshRouterConfig {
    /// Canonical uppercase SHA-256 fingerprint of the local node.
    pub local_fingerprint: String,

    /// Whether this node acts as an intermediate packet and stream relay.
    pub relay_enabled: bool,

    /// Maximum hop limit allowed for routed envelopes.
    pub max_ttl: u8,

    /// Capacity for the packet deduplication ring buffer.
    pub dedup_capacity: usize,

    /// Max in-flight chunks buffered during cut-through streaming (bounded to < 2 MB memory).
    pub stream_buffer_chunks: usize,

    /// Chunk size for streaming transfers (default: 128 KB).
    pub stream_chunk_size: usize,

    /// Request timeout for forwarding requests to next hops.
    pub relay_timeout: Duration,

    /// Human-readable device alias of the local node.
    pub local_alias: String,
}

impl Default for MeshRouterConfig {
    fn default() -> Self {
        Self {
            local_fingerprint: String::new(),
            relay_enabled: true,
            max_ttl: MAX_MESH_TTL,
            dedup_capacity: DEFAULT_DEDUP_CAPACITY,
            stream_buffer_chunks: DEFAULT_STREAM_BUFFER_CHUNKS,
            stream_chunk_size: DEFAULT_STREAM_CHUNK_SIZE,
            relay_timeout: Duration::from_secs(DEFAULT_RELAY_TIMEOUT_SECS),
            local_alias: "LanFlare Node".to_string(),
        }
    }
}

/// JSON response returned upon successful envelope forwarding or local delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardResponse {
    /// "delivered" if reached final destination, or "forwarded" if relayed to next hop.
    pub status: String,

    /// The unique packet identifier.
    pub packet_id: String,

    /// Fingerprint of next hop node if forwarded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,

    /// Remaining Time-To-Live hop count after decrementing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_ttl: Option<u8>,
}

/// Statistics returned upon successful stream delivery or relay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamDeliveryStats {
    /// "delivered" if received at destination, or "relayed" if passed through.
    pub status: String,

    /// Stream session identifier.
    pub stream_id: String,

    /// Total bytes transferred / received.
    pub bytes_received: u64,

    /// Uppercase 64-char hex SHA-256 checksum of the transferred stream payload.
    pub sha256: String,

    /// Next hop node fingerprint if relayed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_hop: Option<String>,
}

/// Exported active topology graph view returned by `GET /api/lanflare/v1/mesh/topology`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshTopologyView {
    /// Fingerprint of the reporting node.
    pub local_fingerprint: String,

    /// Whether relay mode is currently active on this node.
    pub relay_enabled: bool,

    /// All known nodes in the overlay mesh.
    pub nodes: Vec<MeshNode>,

    /// All directed links between nodes.
    pub links: Vec<MeshLink>,

    /// Active path-vector route advertisements.
    pub route_advertisements: Vec<MeshRouteAdvertisement>,
}

/// Type alias for custom dynamic endpoint resolvers: `(fingerprint) -> Option<base_url>`.
pub type DynEndpointResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Type alias for callbacks invoked when an envelope arrives for local delivery.
pub type PacketSink = Arc<dyn Fn(RoutedEnvelope) + Send + Sync>;

/// Type alias for callbacks invoked when cut-through stream chunks arrive for local delivery.
pub type StreamChunkSink = Arc<dyn Fn(&str, Bytes) + Send + Sync>;

/// Trait for intercepting outbound mesh packet forwarding, streaming, and gossip.
pub trait MeshNetworkInterceptor: Send + Sync {
    /// Intercepts a transmission from `src_fp` to `next_hop_fp`.
    /// Returns `Ok(Duration)` to delay (for latency/jitter simulation),
    /// or `Err(RouterError)` to drop/reject (for severed links, partitions, or drop rates).
    fn intercept(&self, src_fp: &str, next_hop_fp: &str) -> Result<Duration, RouterError>;
}

/// Mesh router handling multi-hop gossip exchange, packet forwarding, and cut-through stream relays.
pub struct MeshRouter {
    config: MeshRouterConfig,
    local_fingerprint: String,
    relay_enabled: AtomicBool,
    graph: Arc<RwLock<MeshTopologyGraph>>,
    dedup: Mutex<PacketDedupBuffer>,
    client: reqwest::Client,
    peer_endpoints: RwLock<HashMap<String, String>>,
    custom_resolver: RwLock<Option<DynEndpointResolver>>,
    packet_sink: RwLock<Option<PacketSink>>,
    stream_sink: RwLock<Option<StreamChunkSink>>,
    registry: RwLock<Option<Arc<crate::discovery::registry::PeerRegistry>>>,
    interceptor: RwLock<Option<Arc<dyn MeshNetworkInterceptor>>>,
    mailbox: Arc<RelayMailbox>,
    diagnostics: Arc<crate::mesh::diagnostics::MeshDiagnostics>,
}

impl MeshRouter {
    /// Creates a new `MeshRouter` for the local node with standard defaults.
    pub fn new(local_fp: impl Into<String>) -> Self {
        let canonical_fp = local_fp.into().to_ascii_uppercase();
        let config = MeshRouterConfig {
            local_fingerprint: canonical_fp.clone(),
            ..Default::default()
        };
        let graph = Arc::new(RwLock::new(MeshTopologyGraph::new(canonical_fp)));
        Self::with_config(config, graph)
    }

    /// Creates a new `MeshRouter` with explicit configuration and shared topology graph.
    pub fn with_config(config: MeshRouterConfig, graph: Arc<RwLock<MeshTopologyGraph>>) -> Self {
        let canonical_fp = config.local_fingerprint.to_ascii_uppercase();
        let dedup_cap = config.dedup_capacity;
        let relay_on = config.relay_enabled;

        let client = reqwest::Client::builder()
            .timeout(config.relay_timeout)
            .tcp_nodelay(true)
            .build()
            .unwrap_or_default();

        let diagnostics = Arc::new(crate::mesh::diagnostics::MeshDiagnostics::new(
            &config.local_alias,
        ));

        Self {
            config,
            local_fingerprint: canonical_fp,
            relay_enabled: AtomicBool::new(relay_on),
            graph,
            dedup: Mutex::new(PacketDedupBuffer::new(dedup_cap)),
            client,
            peer_endpoints: RwLock::new(HashMap::new()),
            custom_resolver: RwLock::new(None),
            packet_sink: RwLock::new(None),
            stream_sink: RwLock::new(None),
            registry: RwLock::new(None),
            interceptor: RwLock::new(None),
            mailbox: Arc::new(RelayMailbox::default()),
            diagnostics,
        }
    }

    /// Returns a reference to the diagnostics engine.
    pub fn diagnostics(&self) -> &Arc<crate::mesh::diagnostics::MeshDiagnostics> {
        &self.diagnostics
    }

    /// Returns a reference to the reqwest HTTP client.
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Returns a reference to the network interceptor lock.
    pub fn interceptor(&self) -> &RwLock<Option<Arc<dyn MeshNetworkInterceptor>>> {
        &self.interceptor
    }

    /// Initiates a multi-hop traceroute to `target_fp`.
    pub async fn traceroute(
        &self,
        target_fp: &str,
    ) -> Result<lanflare_proto::mesh::MeshTracerouteResponse, crate::mesh::diagnostics::DiagnosticsError> {
        self.diagnostics.traceroute(self, target_fp).await
    }

    /// Runs a memory-to-memory throughput speedtest streaming `num_chunks` chunks to `target_fp`.
    pub async fn run_speedtest(
        &self,
        target_fp: &str,
        num_chunks: u32,
        chunk_size: usize,
    ) -> Result<lanflare_proto::mesh::SpeedtestResultDto, crate::mesh::diagnostics::DiagnosticsError> {
        self.diagnostics
            .run_speedtest(self, target_fp, num_chunks, chunk_size)
            .await
    }

    /// Returns a reference to the custodial relay mailbox.
    pub fn mailbox(&self) -> &Arc<RelayMailbox> {
        &self.mailbox
    }

    /// Returns the canonical local node fingerprint.
    pub fn local_fingerprint(&self) -> &str {
        &self.local_fingerprint
    }

    /// Returns whether this node is currently configured to relay non-local packets and streams.
    pub fn is_relay_enabled(&self) -> bool {
        self.relay_enabled.load(Ordering::SeqCst)
    }

    /// Dynamically enables or disables intermediate relay forwarding.
    pub fn set_relay_enabled(&self, enabled: bool) {
        self.relay_enabled.store(enabled, Ordering::SeqCst);
    }

    /// Returns a reference to the shared topology graph.
    pub fn graph(&self) -> &Arc<RwLock<MeshTopologyGraph>> {
        &self.graph
    }

    /// Registers a direct HTTP/HTTPS base URL endpoint for a peer fingerprint.
    pub async fn register_peer_endpoint(&self, fingerprint: impl Into<String>, base_url: impl Into<String>) {
        let canonical_fp = fingerprint.into().to_ascii_uppercase();
        let url = base_url.into();
        let mut endpoints = self.peer_endpoints.write().await;
        endpoints.insert(canonical_fp, url);
    }

    /// Removes a registered endpoint mapping.
    pub async fn remove_peer_endpoint(&self, fingerprint: &str) {
        let canonical_fp = fingerprint.to_ascii_uppercase();
        let mut endpoints = self.peer_endpoints.write().await;
        endpoints.remove(&canonical_fp);
    }

    /// Registers a custom dynamic endpoint resolver function.
    pub async fn set_endpoint_resolver<F>(&self, resolver: F)
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        let mut res = self.custom_resolver.write().await;
        *res = Some(Arc::new(resolver));
    }

    /// Registers a callback for locally delivered routed envelopes.
    pub async fn set_packet_sink<F>(&self, sink: F)
    where
        F: Fn(RoutedEnvelope) + Send + Sync + 'static,
    {
        let mut ps = self.packet_sink.write().await;
        *ps = Some(Arc::new(sink));
    }

    /// Registers a callback for locally delivered stream chunks.
    pub async fn set_stream_sink<F>(&self, sink: F)
    where
        F: Fn(&str, Bytes) + Send + Sync + 'static,
    {
        let mut ss = self.stream_sink.write().await;
        *ss = Some(Arc::new(sink));
    }

    /// Attaches a `PeerRegistry` to automatically sync multi-hop peers learned via mesh gossip.
    pub async fn set_registry(&self, registry: Arc<crate::discovery::registry::PeerRegistry>) {
        let mut r = self.registry.write().await;
        *r = Some(registry);
    }

    /// Attaches a network interceptor (such as `VirtualNetworkMatrix`).
    pub async fn set_interceptor(&self, interceptor: Arc<dyn MeshNetworkInterceptor>) {
        let mut int = self.interceptor.write().await;
        *int = Some(interceptor);
    }

    /// Clears any network interceptor.
    pub async fn clear_interceptor(&self) {
        let mut int = self.interceptor.write().await;
        *int = None;
    }

    /// Synchronizes the attached peer registry with current topology graph.
    pub async fn sync_registry(&self) {
        if let Some(registry) = self.registry.read().await.as_ref() {
            let graph = self.graph.read().await;
            registry.sync_from_mesh_graph(&graph).await;
        }
    }

    /// Resolves the base URL for the given peer fingerprint.
    pub async fn resolve_endpoint(&self, fingerprint: &str) -> Option<String> {
        let canonical_fp = fingerprint.to_ascii_uppercase();
        {
            let endpoints = self.peer_endpoints.read().await;
            if let Some(url) = endpoints.get(&canonical_fp) {
                return Some(url.clone());
            }
        }
        {
            let resolver = self.custom_resolver.read().await;
            if let Some(res) = resolver.as_ref()
                && let Some(url) = res(&canonical_fp)
            {
                return Some(url);
            }
        }
        None
    }

    /// Returns the number of packet IDs currently tracked in the deduplication buffer.
    pub fn dedup_buffer_len(&self) -> usize {
        self.dedup.lock().unwrap().len()
    }

    /// Clears the packet deduplication cache.
    pub fn clear_dedup_buffer(&self) {
        self.dedup.lock().unwrap().clear();
    }

    /// Exports a snapshot view of the active topology graph.
    pub async fn topology_view(&self) -> MeshTopologyView {
        let graph = self.graph.read().await;
        MeshTopologyView {
            local_fingerprint: self.local_fingerprint.clone(),
            relay_enabled: self.is_relay_enabled(),
            nodes: graph.nodes().cloned().collect(),
            links: graph.links().cloned().collect(),
            route_advertisements: graph.route_advertisements().cloned().collect(),
        }
    }

    /// Processes an incoming gossip batch and returns the local node's gossip reply.
    pub async fn process_gossip(&self, packet: MeshGossipPacket) -> Result<MeshGossipPacket, RouterError> {
        {
            let mut graph = self.graph.write().await;

            let now_ms = current_time_ms();
            for ann in &packet.announcements {
                let _ = graph.process_node_announcement(ann);
            }
            for metric in &packet.link_metrics {
                graph.process_link_metric(metric);
            }
            for route_ad in &packet.route_advertisements {
                let _ = graph.process_route_advertisement(route_ad, now_ms);
            }
        }

        // Sync learned nodes to attached registry if present
        if let Some(registry) = self.registry.read().await.as_ref() {
            let graph = self.graph.read().await;
            registry.sync_from_mesh_graph(&graph).await;
        }

        let graph = self.graph.read().await;
        let local_announcements: Vec<MeshNodeAnnouncement> = graph
            .nodes()
            .map(|n| MeshNodeAnnouncement {
                fingerprint: n.fingerprint.clone(),
                alias: n.alias.clone(),
                cert_pem: n.cert_pem.clone().unwrap_or_default(),
                capabilities: n.capabilities.clone(),
                relay_enabled: if n.fingerprint.eq_ignore_ascii_case(&self.local_fingerprint) {
                    self.is_relay_enabled()
                } else {
                    n.relay_enabled
                },
                seq_no: n.seq_no,
                timestamp_ms: n.last_seen_ms,
                signature: String::new(),
            })
            .collect();

        let local_links: Vec<MeshLinkMetric> = graph
            .links()
            .filter(|l| l.src_fp.eq_ignore_ascii_case(&self.local_fingerprint))
            .map(|l| MeshLinkMetric {
                src_fp: l.src_fp.clone(),
                dst_fp: l.dst_fp.clone(),
                rtt_ms: l.rtt_ms,
                subnet_tag: l.subnet_tag.clone(),
                bandwidth_kbps: l.bandwidth_kbps,
                last_seen_ms: l.last_seen_ms,
            })
            .collect();

        let mut local_routes: Vec<MeshRouteAdvertisement> =
            graph.route_advertisements().cloned().collect();

        for adv in graph.generate_all_route_advertisements() {
            if !local_routes.iter().any(|r| r.origin_fp == adv.origin_fp) {
                local_routes.push(adv);
            }
        }

        Ok(MeshGossipPacket {
            sender_fp: self.local_fingerprint.clone(),
            announcements: local_announcements,
            link_metrics: local_links,
            route_advertisements: local_routes,
        })
    }

    /// Sends a gossip batch to a neighbor peer over HTTP and ingests the gossip reply.
    pub async fn send_gossip(&self, peer_fp: &str) -> Result<MeshGossipPacket, RouterError> {
        let canonical_fp = peer_fp.to_ascii_uppercase();

        // Interceptor check (latency/drop/partition)
        if let Some(interceptor) = self.interceptor.read().await.as_ref() {
            let delay = interceptor.intercept(&self.local_fingerprint, &canonical_fp)?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        let target_url = self
            .resolve_endpoint(&canonical_fp)
            .await
            .ok_or_else(|| RouterError::UnresolvedEndpoint(canonical_fp.clone()))?;

        // Prepare local gossip packet
        let gossip_out = {
            let graph = self.graph.read().await;
            let announcements: Vec<MeshNodeAnnouncement> = graph
                .nodes()
                .map(|n| MeshNodeAnnouncement {
                    fingerprint: n.fingerprint.clone(),
                    alias: n.alias.clone(),
                    cert_pem: n.cert_pem.clone().unwrap_or_default(),
                    capabilities: n.capabilities.clone(),
                    relay_enabled: n.relay_enabled,
                    seq_no: n.seq_no,
                    timestamp_ms: n.last_seen_ms,
                    signature: String::new(),
                })
                .collect();

            let link_metrics: Vec<MeshLinkMetric> = graph
                .links()
                .map(|l| MeshLinkMetric {
                    src_fp: l.src_fp.clone(),
                    dst_fp: l.dst_fp.clone(),
                    rtt_ms: l.rtt_ms,
                    subnet_tag: l.subnet_tag.clone(),
                    bandwidth_kbps: l.bandwidth_kbps,
                    last_seen_ms: l.last_seen_ms,
                })
                .collect();

            let mut route_ads: Vec<MeshRouteAdvertisement> = graph
                .route_advertisements()
                .cloned()
                .collect();

            for adv in graph.generate_all_route_advertisements() {
                if !route_ads.iter().any(|r| r.origin_fp == adv.origin_fp) {
                    route_ads.push(adv);
                }
            }

            MeshGossipPacket {
                sender_fp: self.local_fingerprint.clone(),
                announcements,
                link_metrics,
                route_advertisements: route_ads,
            }
        };

        let post_url = format!("{}/api/lanflare/v1/mesh/gossip", target_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&post_url)
            .json(&gossip_out)
            .send()
            .await
            .map_err(|e| RouterError::Network(format!("Failed to gossip with {}: {}", canonical_fp, e)))?;

        if !resp.status().is_success() {
            return Err(RouterError::Network(format!(
                "Gossip with {} returned status {}",
                canonical_fp,
                resp.status()
            )));
        }

        let reply: MeshGossipPacket = resp
            .json()
            .await
            .map_err(|e| RouterError::Network(format!("Failed to parse gossip reply: {}", e)))?;

        // Ingest the reply into our graph
        {
            let mut graph = self.graph.write().await;
            let now_ms = current_time_ms();
            for ann in &reply.announcements {
                let _ = graph.process_node_announcement(ann);
            }
            for metric in &reply.link_metrics {
                graph.process_link_metric(metric);
            }
            for route_ad in &reply.route_advertisements {
                let _ = graph.process_route_advertisement(route_ad, now_ms);
            }
        }

        // Sync registry if attached
        if let Some(registry) = self.registry.read().await.as_ref() {
            let graph = self.graph.read().await;
            registry.sync_from_mesh_graph(&graph).await;
        }

        Ok(reply)
    }

    /// Sends a cut-through stream directly to destination `dst_fp` along resolved multi-hop mesh route.
    pub async fn send_stream(
        &self,
        dst_fp: &str,
        file_id: &str,
        stream_id: &str,
        chunks: Vec<Bytes>,
    ) -> Result<StreamDeliveryStats, RouterError> {
        let canonical_dst = dst_fp.to_ascii_uppercase();

        // 1. If self is destination:
        if canonical_dst == self.local_fingerprint {
            let mut hasher = Sha256::new();
            let mut total_bytes = 0u64;
            for chunk in chunks {
                total_bytes += chunk.len() as u64;
                hasher.update(&chunk);
                if let Some(sink) = self.stream_sink.read().await.as_ref() {
                    sink(stream_id, chunk);
                }
            }
            let sha256_hex = format!("{:064X}", hasher.finalize());
            return Ok(StreamDeliveryStats {
                status: "delivered".to_string(),
                stream_id: stream_id.to_string(),
                bytes_received: total_bytes,
                sha256: sha256_hex,
                next_hop: None,
            });
        }

        // 2. Resolve route to destination
        let next_hop_fp = {
            let graph = self.graph.read().await;
            let route = graph
                .resolve_route(&canonical_dst)
                .ok_or_else(|| RouterError::NoRoute(canonical_dst.clone()))?;
            route.next_hop_fp
        };

        // 3. Resolve endpoint URL for next hop
        let next_hop_url = self
            .resolve_endpoint(&next_hop_fp)
            .await
            .ok_or_else(|| RouterError::UnresolvedEndpoint(next_hop_fp.clone()))?;

        // 4. Interceptor check
        if let Some(interceptor) = self.interceptor.read().await.as_ref() {
            let delay = interceptor.intercept(&self.local_fingerprint, &next_hop_fp)?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        let total_size: u64 = chunks.iter().map(|c| c.len() as u64).sum();
        let header = MeshStreamHeader {
            stream_id: stream_id.to_string(),
            src_fp: self.local_fingerprint.clone(),
            dst_fp: canonical_dst,
            file_id: file_id.to_string(),
            total_size,
            route_path: vec![self.local_fingerprint.clone()],
            chunk_index: 0,
            is_last_chunk: false,
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(self.config.stream_buffer_chunks);
        tokio::spawn(async move {
            for chunk in chunks {
                if tx.send(Ok(chunk)).await.is_err() {
                    break;
                }
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let target_url = format!("{}/api/lanflare/v1/mesh/stream", next_hop_url.trim_end_matches('/'));

        let forward_req = self
            .client
            .post(&target_url)
            .header(
                "x-lanflare-stream-header",
                serde_json::to_string(&header).unwrap_or_default(),
            )
            .header("x-lanflare-stream-id", &header.stream_id)
            .header("x-lanflare-src-fp", &header.src_fp)
            .header("x-lanflare-dst-fp", &header.dst_fp)
            .header("x-lanflare-file-id", &header.file_id)
            .header("x-lanflare-total-size", header.total_size.to_string())
            .header("x-lanflare-route-path", header.route_path.join(","))
            .header("x-lanflare-chunk-index", header.chunk_index.to_string())
            .header("x-lanflare-is-last-chunk", header.is_last_chunk.to_string())
            .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::Body::wrap_stream(stream));

        let resp = forward_req
            .send()
            .await
            .map_err(|e| RouterError::Network(format!("Failed to stream to next hop {}: {}", next_hop_fp, e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            return Err(RouterError::Network(format!(
                "Next hop {} returned error status {}: {}",
                next_hop_fp, status, err_text
            )));
        }

        let stats: StreamDeliveryStats = resp
            .json()
            .await
            .map_err(|e| RouterError::StreamError(format!("Failed to parse delivery stats: {}", e)))?;

        Ok(stats)
    }

    /// Forwards an envelope or delivers it locally.
    pub async fn forward_envelope(&self, envelope: RoutedEnvelope) -> Result<ForwardResponse, RouterError> {
        // 1. Check TTL bound
        if envelope.ttl == 0 {
            return Err(RouterError::TtlExpired(0));
        }

        // 2. Check deduplication ring buffer
        {
            let mut dedup = self.dedup.lock().unwrap();
            if dedup.check_and_insert(&envelope.packet_id) {
                return Err(RouterError::DuplicatePacket(envelope.packet_id.clone()));
            }
        }

        // 3. Check for local destination delivery
        if envelope.dst_fp.eq_ignore_ascii_case(&self.local_fingerprint) {
            if let Some(sink) = self.packet_sink.read().await.as_ref() {
                sink(envelope.clone());
            }
            return Ok(ForwardResponse {
                status: "delivered".to_string(),
                packet_id: envelope.packet_id,
                next_hop: None,
                remaining_ttl: Some(envelope.ttl),
            });
        }

        // 4. Relay policy check: intermediate forwarding requires relaying enabled
        if !self.is_relay_enabled() && !envelope.src_fp.eq_ignore_ascii_case(&self.local_fingerprint) {
            return Err(RouterError::RelayDisabled);
        }

        // 5. Decrement TTL for the next hop
        let new_ttl = envelope.ttl.saturating_sub(1);
        if new_ttl == 0 {
            return Err(RouterError::TtlExpired(0));
        }

        // 6. Route resolution
        let next_hop_fp = {
            let graph = self.graph.read().await;
            let route = graph
                .resolve_route(&envelope.dst_fp)
                .ok_or_else(|| RouterError::NoRoute(envelope.dst_fp.clone()))?;
            route.next_hop_fp
        };

        // 7. Resolve next hop endpoint URL
        let next_hop_url = self
            .resolve_endpoint(&next_hop_fp)
            .await
            .ok_or_else(|| RouterError::UnresolvedEndpoint(next_hop_fp.clone()))?;

        // 8. Prepare forwarded envelope
        let mut forwarded = envelope.clone();
        forwarded.ttl = new_ttl;
        forwarded.route_path.push(self.local_fingerprint.clone());

        // 8.5 Interceptor check (latency/drop/partition)
        if let Some(interceptor) = self.interceptor.read().await.as_ref() {
            let delay = interceptor.intercept(&self.local_fingerprint, &next_hop_fp)?;
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        // 9. Forward envelope to next hop over HTTP
        let target_url = format!("{}/api/lanflare/v1/mesh/forward", next_hop_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&target_url)
            .json(&forwarded)
            .send()
            .await
            .map_err(|e| RouterError::Network(format!("Failed to forward to next hop {}: {}", next_hop_fp, e)))?;

        if !response.status().is_success() {
            return Err(RouterError::Network(format!(
                "Next hop {} returned error status: {}",
                next_hop_fp,
                response.status()
            )));
        }

        Ok(ForwardResponse {
            status: "forwarded".to_string(),
            packet_id: envelope.packet_id,
            next_hop: Some(next_hop_fp),
            remaining_ttl: Some(new_ttl),
        })
    }

    /// Handles incoming Hyper requests targeting `/api/lanflare/v1/mesh/*`.
    pub async fn handle_hyper_request(
        &self,
        req: Request<Incoming>,
        _client: Option<ClientConnectionInfo>,
    ) -> Response<ServerResponseBody> {
        let method = req.method();
        let path = req.uri().path();

        match (method, path) {
            (&Method::POST, "/api/lanflare/v1/mesh/gossip") => self.handle_gossip_endpoint(req).await,
            (&Method::GET, "/api/lanflare/v1/mesh/topology") => self.handle_topology_endpoint().await,
            (&Method::POST, "/api/lanflare/v1/mesh/forward") => self.handle_forward_endpoint(req).await,
            (&Method::POST, "/api/lanflare/v1/mesh/stream") => self.handle_stream_endpoint(req).await,
            (&Method::POST, "/api/lanflare/v1/mesh/mailbox/deposit") => {
                self.mailbox.handle_deposit_endpoint(req).await
            }
            (&Method::POST, "/api/lanflare/v1/mesh/mailbox/fetch") => {
                self.mailbox.handle_fetch_endpoint(req).await
            }
            (&Method::POST, "/api/lanflare/v1/mesh/trace") => {
                self.diagnostics.handle_trace_endpoint(req, self).await
            }
            (&Method::POST, "/api/lanflare/v1/mesh/benchmark") => {
                self.diagnostics.handle_benchmark_endpoint(req, self).await
            }
            _ => empty_box_response(StatusCode::NOT_FOUND),
        }
    }

    /// Deposits an envelope into a remote relay node's custodial mailbox.
    pub async fn deposit_to_relay_mailbox(
        &self,
        relay_fp: &str,
        deposit_req: &MailboxDepositRequest,
    ) -> Result<MailboxDepositResponse, RouterError> {
        let base_url = self
            .resolve_endpoint(relay_fp)
            .await
            .ok_or_else(|| RouterError::UnresolvedEndpoint(relay_fp.to_string()))?;

        RelayMailbox::deposit_to_relay_http(&self.client, &base_url, deposit_req)
            .await
            .map_err(|e| RouterError::Network(e.to_string()))
    }

    /// Fetches pending envelopes from a remote relay node's custodial mailbox.
    pub async fn fetch_from_relay_mailbox(
        &self,
        relay_fp: &str,
        fetch_req: &MailboxFetchRequest,
    ) -> Result<MailboxFetchResponse, RouterError> {
        let base_url = self
            .resolve_endpoint(relay_fp)
            .await
            .ok_or_else(|| RouterError::UnresolvedEndpoint(relay_fp.to_string()))?;

        RelayMailbox::fetch_from_relay_http(&self.client, &base_url, fetch_req)
            .await
            .map_err(|e| RouterError::Network(e.to_string()))
    }

    async fn handle_topology_endpoint(&self) -> Response<ServerResponseBody> {
        let view = self.topology_view().await;
        json_response(StatusCode::OK, &view)
    }

    async fn handle_gossip_endpoint(&self, req: Request<Incoming>) -> Response<ServerResponseBody> {
        let body = match Self::read_bounded_body(req.into_body()).await {
            Ok(bytes) => bytes,
            Err(status) => return empty_box_response(status),
        };

        let packet: MeshGossipPacket = match serde_json::from_slice(&body) {
            Ok(p) => p,
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        match self.process_gossip(packet).await {
            Ok(reply) => json_response(StatusCode::OK, &reply),
            Err(err) => json_response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "error": err.to_string() }),
            ),
        }
    }

    async fn handle_forward_endpoint(&self, req: Request<Incoming>) -> Response<ServerResponseBody> {
        let body = match Self::read_bounded_body(req.into_body()).await {
            Ok(bytes) => bytes,
            Err(status) => return empty_box_response(status),
        };

        let envelope: RoutedEnvelope = match serde_json::from_slice(&body) {
            Ok(env) => env,
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        match self.forward_envelope(envelope).await {
            Ok(resp) => json_response(StatusCode::OK, &resp),
            Err(RouterError::DuplicatePacket(id)) => json_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &serde_json::json!({ "error": format!("Duplicate packet: {}", id) }),
            ),
            Err(RouterError::TtlExpired(ttl)) => json_response(
                StatusCode::UNPROCESSABLE_ENTITY,
                &serde_json::json!({ "error": format!("TTL expired (ttl={})", ttl) }),
            ),
            Err(RouterError::RelayDisabled) => json_response(
                StatusCode::FORBIDDEN,
                &serde_json::json!({ "error": "Relay forwarding disabled on this node" }),
            ),
            Err(RouterError::NoRoute(dst)) => json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({ "error": format!("No route to destination: {}", dst) }),
            ),
            Err(RouterError::UnresolvedEndpoint(fp)) => json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({ "error": format!("Unresolved endpoint for next hop: {}", fp) }),
            ),
            Err(RouterError::Network(msg)) => json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({ "error": format!("Network error: {}", msg) }),
            ),
            Err(err) => json_response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "error": err.to_string() }),
            ),
        }
    }

    async fn handle_stream_endpoint(&self, req: Request<Incoming>) -> Response<ServerResponseBody> {
        // 1. Extract stream headers
        let header = match Self::extract_stream_header(&req) {
            Ok(h) => h,
            Err(status) => return empty_box_response(status),
        };

        // 2. Check if destination is local node
        if header.dst_fp.eq_ignore_ascii_case(&self.local_fingerprint) {
            // Local Delivery: Read incoming chunks, compute SHA-256 and invoke stream sink
            let mut incoming_body = req.into_body();
            let mut hasher = Sha256::new();
            let mut total_bytes = 0u64;

            while let Some(frame_res) = incoming_body.frame().await {
                match frame_res {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data()
                            && !data.is_empty()
                        {
                            total_bytes += data.len() as u64;
                            hasher.update(&data);
                            if let Some(sink) = self.stream_sink.read().await.as_ref() {
                                sink(&header.stream_id, data);
                            }
                        }
                    }
                    Err(err) => {
                        return json_response(
                            StatusCode::BAD_REQUEST,
                            &serde_json::json!({ "error": format!("Stream read error: {}", err) }),
                        );
                    }
                }
            }

            let sha256_hex = format!("{:064X}", hasher.finalize());
            return json_response(
                StatusCode::OK,
                &StreamDeliveryStats {
                    status: "delivered".to_string(),
                    stream_id: header.stream_id,
                    bytes_received: total_bytes,
                    sha256: sha256_hex,
                    next_hop: None,
                },
            );
        }

        // 3. Relay policy check
        if !self.is_relay_enabled() {
            return json_response(
                StatusCode::FORBIDDEN,
                &serde_json::json!({ "error": "Relay forwarding is disabled on this node" }),
            );
        }

        // 4. Route resolution
        let next_hop_fp = {
            let graph = self.graph.read().await;
            match graph.resolve_route(&header.dst_fp) {
                Some(route) => route.next_hop_fp,
                None => {
                    return json_response(
                        StatusCode::BAD_GATEWAY,
                        &serde_json::json!({ "error": format!("No route to destination: {}", header.dst_fp) }),
                    );
                }
            }
        };

        // 5. Endpoint resolution
        let next_hop_url = match self.resolve_endpoint(&next_hop_fp).await {
            Some(url) => url,
            None => {
                return json_response(
                    StatusCode::BAD_GATEWAY,
                    &serde_json::json!({ "error": format!("Unresolved endpoint for next hop: {}", next_hop_fp) }),
                );
            }
        };

        // 5.5 Interceptor check
        if let Some(interceptor) = self.interceptor.read().await.as_ref() {
            let delay = match interceptor.intercept(&self.local_fingerprint, &next_hop_fp) {
                Ok(d) => d,
                Err(err) => {
                    return json_response(
                        StatusCode::BAD_GATEWAY,
                        &serde_json::json!({ "error": format!("Relay intercepted/dropped: {}", err) }),
                    );
                }
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }

        // 6. Cut-through streaming relay:
        // Pipe 128 KB chunks from inbound HTTP body to next-hop outbound HTTP stream
        // with bounded memory (< 2 MB via mpsc channel).
        let buffer_capacity = self.config.stream_buffer_chunks;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(buffer_capacity);

        let mut incoming_body = req.into_body();
        tokio::spawn(async move {
            while let Some(frame_res) = incoming_body.frame().await {
                match frame_res {
                    Ok(frame) => {
                        if let Ok(data) = frame.into_data()
                            && !data.is_empty()
                            && tx.send(Ok(data)).await.is_err()
                        {
                            // Outbound stream dropped or error occurred
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx
                            .send(Err(std::io::Error::other(err.to_string())))
                            .await;
                        break;
                    }
                }
            }
        });

        // Update route path with local node
        let mut forwarded_header = header.clone();
        forwarded_header.route_path.push(self.local_fingerprint.clone());

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let target_url = format!("{}/api/lanflare/v1/mesh/stream", next_hop_url.trim_end_matches('/'));

        let forward_req = self
            .client
            .post(&target_url)
            .header(
                "x-lanflare-stream-header",
                serde_json::to_string(&forwarded_header).unwrap_or_default(),
            )
            .header("x-lanflare-stream-id", &forwarded_header.stream_id)
            .header("x-lanflare-src-fp", &forwarded_header.src_fp)
            .header("x-lanflare-dst-fp", &forwarded_header.dst_fp)
            .header("x-lanflare-file-id", &forwarded_header.file_id)
            .header("x-lanflare-total-size", forwarded_header.total_size.to_string())
            .header("x-lanflare-route-path", forwarded_header.route_path.join(","))
            .header("x-lanflare-chunk-index", forwarded_header.chunk_index.to_string())
            .header("x-lanflare-is-last-chunk", forwarded_header.is_last_chunk.to_string())
            .header(hyper::header::CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::Body::wrap_stream(stream));

        match forward_req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let bytes = resp.bytes().await.unwrap_or_default();
                Response::builder()
                    .status(status)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(full_box_body(bytes))
                    .unwrap_or_else(|_| empty_box_response(StatusCode::INTERNAL_SERVER_ERROR))
            }
            Err(err) => json_response(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({ "error": format!("Downstream relay failure: {}", err) }),
            ),
        }
    }

    /// Extracts stream header from either custom header or individual headers.
    fn extract_stream_header(req: &Request<Incoming>) -> Result<MeshStreamHeader, StatusCode> {
        let headers = req.headers();

        // 1. Try JSON header `x-lanflare-stream-header`
        if let Some(val) = headers.get("x-lanflare-stream-header")
            && let Ok(s) = val.to_str()
            && let Ok(header) = serde_json::from_str::<MeshStreamHeader>(s)
        {
            return Ok(header);
        }

        // 2. Extract from individual headers
        let stream_id = headers
            .get("x-lanflare-stream-id")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?
            .to_string();

        let src_fp = headers
            .get("x-lanflare-src-fp")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?
            .to_string();

        let dst_fp = headers
            .get("x-lanflare-dst-fp")
            .and_then(|v| v.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?
            .to_string();

        let file_id = headers
            .get("x-lanflare-file-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let total_size = headers
            .get("x-lanflare-total-size")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let route_path = headers
            .get("x-lanflare-route-path")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
            .unwrap_or_default();

        let chunk_index = headers
            .get("x-lanflare-chunk-index")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let is_last_chunk = headers
            .get("x-lanflare-is-last-chunk")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<bool>().ok())
            .unwrap_or(true);

        Ok(MeshStreamHeader {
            stream_id,
            src_fp,
            dst_fp,
            file_id,
            total_size,
            route_path,
            chunk_index,
            is_last_chunk,
        })
    }

    /// Reads bounded request body up to `MAX_CONTROL_BODY_SIZE` (10 MB).
    async fn read_bounded_body(body: Incoming) -> Result<Bytes, StatusCode> {
        let limited = http_body_util::Limited::new(body, MAX_CONTROL_BODY_SIZE);
        match limited.collect().await {
            Ok(collected) => Ok(collected.to_bytes()),
            Err(err) => {
                if err.downcast_ref::<http_body_util::LengthLimitError>().is_some() {
                    Err(StatusCode::PAYLOAD_TOO_LARGE)
                } else {
                    Err(StatusCode::BAD_REQUEST)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use hyper_util::rt::TokioIo;
    use hyper_util::server::conn::auto::Builder;
    use lanflare_proto::mesh::E2eeEnvelope;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use super::*;

    fn dummy_e2ee_envelope(sender_fp: &str, recipient_fp: &str) -> E2eeEnvelope {
        E2eeEnvelope {
            sender_fp: sender_fp.into(),
            recipient_fp: recipient_fp.into(),
            sender_cert_pem: None,
            encrypted_key: "k9...".into(),
            nonce: "nonce123".into(),
            ciphertext: "c8...".into(),
            timestamp_ms: 1770000000000,
            signature: "sig123".into(),
        }
    }

    fn dummy_routed_envelope(
        packet_id: &str,
        src_fp: &str,
        dst_fp: &str,
        ttl: u8,
    ) -> RoutedEnvelope {
        RoutedEnvelope {
            packet_id: packet_id.into(),
            src_fp: src_fp.into(),
            dst_fp: dst_fp.into(),
            ttl,
            route_path: vec![src_fp.into()],
            payload: dummy_e2ee_envelope(src_fp, dst_fp),
        }
    }

    /// Spawns a background Hyper HTTP server running `MeshRouter::handle_hyper_request`.
    async fn spawn_test_router_server(
        router: Arc<MeshRouter>,
    ) -> (SocketAddr, oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            let auto_builder = Builder::new(hyper_util::rt::TokioExecutor::new());

            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    Ok((stream, _)) = listener.accept() => {
                        let r = router.clone();
                        let b = auto_builder.clone();
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let service = hyper::service::service_fn(move |req| {
                                let r2 = r.clone();
                                async move {
                                    Ok::<_, hyper::Error>(r2.handle_hyper_request(req, None).await)
                                }
                            });
                            let _ = b.serve_connection(io, service).await;
                        });
                    }
                }
            }
        });

        (addr, shutdown_tx)
    }

    #[tokio::test]
    async fn test_packet_dedup_and_ttl_exhaustion() {
        let local_fp = "1111111111111111111111111111111111111111111111111111111111111111";
        let router = Arc::new(MeshRouter::new(local_fp));

        // 1. Initial valid envelope to local node
        let env1 = dummy_routed_envelope("PKT-100", "SOURCE_NODE", local_fp, 3);
        let res1 = router.forward_envelope(env1.clone()).await;
        assert!(res1.is_ok(), "Initial envelope delivery should succeed");
        let delivered = res1.unwrap();
        assert_eq!(delivered.status, "delivered");
        assert_eq!(delivered.packet_id, "PKT-100");

        // 2. Duplicate packet ID check -> rejected immediately with DuplicatePacket
        let res_dup = router.forward_envelope(env1).await;
        assert!(
            matches!(res_dup, Err(RouterError::DuplicatePacket(id)) if id == "PKT-100"),
            "Duplicate packet ID should be rejected"
        );

        // 3. New packet ID -> accepted
        let env2 = dummy_routed_envelope("PKT-200", "SOURCE_NODE", local_fp, 3);
        let res2 = router.forward_envelope(env2).await;
        assert!(res2.is_ok(), "New packet ID should be accepted");

        // 4. TTL = 0 check -> rejected immediately with TtlExpired(0)
        let env_ttl0 = dummy_routed_envelope("PKT-300", "SOURCE_NODE", local_fp, 0);
        let res_ttl0 = router.forward_envelope(env_ttl0).await;
        assert!(
            matches!(res_ttl0, Err(RouterError::TtlExpired(0))),
            "TTL=0 should be rejected immediately"
        );

        // 5. TTL decrement exhaustion on forwarded packet:
        // Remote destination with TTL=1 -> decrement yields 0 -> rejected before sending
        let remote_fp = "2222222222222222222222222222222222222222222222222222222222222222";
        let env_ttl1 = dummy_routed_envelope("PKT-400", "SOURCE_NODE", remote_fp, 1);
        let res_ttl1 = router.forward_envelope(env_ttl1).await;
        assert!(
            matches!(res_ttl1, Err(RouterError::TtlExpired(0))),
            "Forwarded envelope whose TTL reaches 0 should be rejected"
        );

        // 6. Test ring buffer capacity eviction
        let mut dedup_buf = PacketDedupBuffer::new(3);
        assert!(!dedup_buf.check_and_insert("A"));
        assert!(!dedup_buf.check_and_insert("B"));
        assert!(!dedup_buf.check_and_insert("C"));
        assert!(dedup_buf.check_and_insert("A"), "A should be duplicate");

        // Insert D, causing A to be evicted
        assert!(!dedup_buf.check_and_insert("D"));
        assert_eq!(dedup_buf.len(), 3);
        assert!(!dedup_buf.check_and_insert("A"), "A was evicted so it can be inserted again");
        assert!(dedup_buf.check_and_insert("D"), "D is still in cache");
    }

    #[tokio::test]
    async fn test_relay_policy_opt_out_rejection() {
        let local_fp = "LOCAL_NODE_00000000000000000000000000000000000000000000000000000000";
        let remote_fp = "REMOTE_NODE_000000000000000000000000000000000000000000000000000000";

        // Create router with relay disabled
        let config = MeshRouterConfig {
            local_fingerprint: local_fp.into(),
            relay_enabled: false,
            ..Default::default()
        };
        let graph = Arc::new(RwLock::new(MeshTopologyGraph::new(local_fp)));
        let router = Arc::new(MeshRouter::with_config(config, graph));

        assert!(!router.is_relay_enabled(), "Relay should be disabled");

        // 1. Forwarding to remote node should be rejected with RelayDisabled
        let env_remote = dummy_routed_envelope("PKT-RELAY-1", "SOURCE", remote_fp, 4);
        let res_remote = router.forward_envelope(env_remote).await;
        assert!(
            matches!(res_remote, Err(RouterError::RelayDisabled)),
            "Relay-disabled router must reject non-local envelope forwarding"
        );

        // 2. Delivery to local node should succeed even with relay disabled
        let env_local = dummy_routed_envelope("PKT-LOCAL-1", "SOURCE", local_fp, 4);
        let res_local = router.forward_envelope(env_local).await;
        assert!(res_local.is_ok(), "Local delivery must succeed even when relay is disabled");

        // 3. Dynamic toggle: Enable relay mode
        router.set_relay_enabled(true);
        assert!(router.is_relay_enabled(), "Relay should now be enabled");

        // 4. Verify HTTP rejection through Hyper server
        router.set_relay_enabled(false);
        let (addr, _shutdown) = spawn_test_router_server(router.clone()).await;
        let client = reqwest::Client::new();

        // HTTP POST /api/lanflare/v1/mesh/forward targeting remote peer
        let env_http = dummy_routed_envelope("PKT-HTTP-1", "SOURCE", remote_fp, 4);
        let resp = client
            .post(format!("http://{}/api/lanflare/v1/mesh/forward", addr))
            .json(&env_http)
            .send()
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "HTTP forward must return 403 Forbidden when relay is disabled"
        );

        // HTTP POST /api/lanflare/v1/mesh/stream targeting remote peer
        let stream_req = client
            .post(format!("http://{}/api/lanflare/v1/mesh/stream", addr))
            .header("x-lanflare-stream-id", "stream-01")
            .header("x-lanflare-src-fp", "SOURCE")
            .header("x-lanflare-dst-fp", remote_fp)
            .body(vec![1, 2, 3, 4])
            .send()
            .await
            .unwrap();

        assert_eq!(
            stream_req.status(),
            StatusCode::FORBIDDEN,
            "HTTP stream must return 403 Forbidden when relay is disabled"
        );
    }

    #[tokio::test]
    async fn test_cut_through_stream_forwarding() {
        // Architecture: Node A (Sender) -> Node B (Relay) -> Node C (Receiver)
        let fp_a = "AAAA_NODE_00000000000000000000000000000000000000000000000000000000";
        let fp_b = "BBBB_NODE_00000000000000000000000000000000000000000000000000000000";
        let fp_c = "CCCC_NODE_00000000000000000000000000000000000000000000000000000000";

        // 1. Setup Node C (Receiver)
        let router_c = Arc::new(MeshRouter::new(fp_c));
        let (addr_c, _shutdown_c) = spawn_test_router_server(router_c.clone()).await;

        // 2. Setup Node B (Relay)
        let router_b = Arc::new(MeshRouter::new(fp_b));
        router_b.set_relay_enabled(true);

        // Register Node B -> Node C link and endpoint
        {
            let mut graph_b = router_b.graph().write().await;
            graph_b.insert_node(MeshNode::new(fp_c, "Node C", true));
            graph_b.add_bidirectional_link(fp_b, fp_c, 10, 1000);
        }
        router_b
            .register_peer_endpoint(fp_c, format!("http://{}", addr_c))
            .await;

        let (addr_b, _shutdown_b) = spawn_test_router_server(router_b.clone()).await;

        // 3. Node A prepares a 2 MB streaming test payload
        const PAYLOAD_SIZE: usize = 2 * 1024 * 1024; // 2 MB
        let mut test_bytes = Vec::with_capacity(PAYLOAD_SIZE);
        for i in 0..PAYLOAD_SIZE {
            test_bytes.push((i % 251) as u8);
        }

        let expected_sha256 = format!("{:064X}", Sha256::digest(&test_bytes));

        // 4. Node A streams payload through Node B to Node C
        let client = reqwest::Client::new();
        let stream_header = MeshStreamHeader {
            stream_id: "stream-cutthrough-001".to_string(),
            src_fp: fp_a.to_string(),
            dst_fp: fp_c.to_string(),
            file_id: "file-bench-2mb.bin".to_string(),
            total_size: PAYLOAD_SIZE as u64,
            route_path: vec![fp_a.to_string()],
            chunk_index: 0,
            is_last_chunk: true,
        };

        let response = client
            .post(format!("http://{}/api/lanflare/v1/mesh/stream", addr_b))
            .header(
                "x-lanflare-stream-header",
                serde_json::to_string(&stream_header).unwrap(),
            )
            .header("content-type", "application/octet-stream")
            .body(test_bytes)
            .send()
            .await
            .expect("Stream transfer through relay failed");

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "Stream response should be 200 OK"
        );

        let stats: StreamDeliveryStats = response.json().await.unwrap();
        assert_eq!(stats.status, "delivered");
        assert_eq!(stats.stream_id, "stream-cutthrough-001");
        assert_eq!(stats.bytes_received, PAYLOAD_SIZE as u64);
        assert_eq!(
            stats.sha256, expected_sha256,
            "Cut-through forwarded stream must match bit-for-bit SHA-256 integrity"
        );
    }

    #[tokio::test]
    async fn test_gossip_exchange_and_topology_export() {
        let fp_alpha = "ALPHA_00000000000000000000000000000000000000000000000000000000000";
        let fp_beta = "BETA_000000000000000000000000000000000000000000000000000000000000";

        let router_beta = Arc::new(MeshRouter::new(fp_beta));
        let (addr_beta, _shutdown) = spawn_test_router_server(router_beta.clone()).await;

        let client = reqwest::Client::new();

        // 1. Alpha sends gossip to Beta
        let gossip_packet = MeshGossipPacket {
            sender_fp: fp_alpha.to_string(),
            announcements: vec![MeshNodeAnnouncement {
                fingerprint: fp_alpha.to_string(),
                alias: "Alpha Node".to_string(),
                cert_pem: String::new(),
                capabilities: vec!["relay".to_string(), "chat".to_string()],
                relay_enabled: true,
                seq_no: 1,
                timestamp_ms: 1000,
                signature: String::new(),
            }],
            link_metrics: vec![MeshLinkMetric {
                src_fp: fp_alpha.to_string(),
                dst_fp: fp_beta.to_string(),
                rtt_ms: 8,
                subnet_tag: Some("lan0".to_string()),
                bandwidth_kbps: Some(100_000),
                last_seen_ms: 1000,
            }],
            route_advertisements: Vec::new(),
        };

        let res = client
            .post(format!("http://{}/api/lanflare/v1/mesh/gossip", addr_beta))
            .json(&gossip_packet)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let reply: MeshGossipPacket = res.json().await.unwrap();
        assert_eq!(reply.sender_fp, fp_beta);

        // 2. Query Beta's topology export
        let topo_res = client
            .get(format!("http://{}/api/lanflare/v1/mesh/topology", addr_beta))
            .send()
            .await
            .unwrap();

        assert_eq!(topo_res.status(), StatusCode::OK);
        let view: MeshTopologyView = topo_res.json().await.unwrap();
        assert_eq!(view.local_fingerprint, fp_beta);
        assert!(view.nodes.iter().any(|n| n.fingerprint == fp_alpha));
        assert!(view.links.iter().any(|l| l.src_fp == fp_alpha && l.dst_fp == fp_beta));
    }

    #[tokio::test]
    async fn test_multi_hop_envelope_forwarding() {
        let fp_a = "AAAA_NODE_00000000000000000000000000000000000000000000000000000000";
        let fp_b = "BBBB_NODE_00000000000000000000000000000000000000000000000000000000";
        let fp_c = "CCCC_NODE_00000000000000000000000000000000000000000000000000000000";

        // 1. Setup Node C (Receiver) with packet sink
        let router_c = Arc::new(MeshRouter::new(fp_c));
        let (delivered_tx, delivered_rx) = oneshot::channel();
        let delivered_tx = Arc::new(Mutex::new(Some(delivered_tx)));
        router_c
            .set_packet_sink(move |env| {
                if let Some(tx) = delivered_tx.lock().unwrap().take() {
                    let _ = tx.send(env);
                }
            })
            .await;
        let (addr_c, _shutdown_c) = spawn_test_router_server(router_c.clone()).await;

        // 2. Setup Node B (Relay)
        let router_b = Arc::new(MeshRouter::new(fp_b));
        router_b.set_relay_enabled(true);
        {
            let mut graph_b = router_b.graph().write().await;
            graph_b.insert_node(MeshNode::new(fp_c, "Node C", true));
            graph_b.add_bidirectional_link(fp_b, fp_c, 10, 1000);
        }
        router_b
            .register_peer_endpoint(fp_c, format!("http://{}", addr_c))
            .await;
        let (addr_b, _shutdown_b) = spawn_test_router_server(router_b.clone()).await;

        // 3. Sender sends envelope to Node B destined for Node C with TTL=4
        let client = reqwest::Client::new();
        let envelope = dummy_routed_envelope("PKT-E2E-FORWARD", fp_a, fp_c, 4);

        let res = client
            .post(format!("http://{}/api/lanflare/v1/mesh/forward", addr_b))
            .json(&envelope)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let forward_resp: ForwardResponse = res.json().await.unwrap();
        assert_eq!(forward_resp.status, "forwarded");
        assert_eq!(forward_resp.next_hop.as_deref(), Some(fp_c));
        assert_eq!(forward_resp.remaining_ttl, Some(3));

        // 4. Verify Node C received the envelope
        let delivered_env = delivered_rx.await.expect("Node C should receive delivered envelope");
        assert_eq!(delivered_env.packet_id, "PKT-E2E-FORWARD");
        assert_eq!(delivered_env.ttl, 3);
        assert!(delivered_env.route_path.contains(&fp_b.to_string()));
    }
}
