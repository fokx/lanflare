//! Link-state / path-vector topology graph and Dijkstra shortest-path route resolver.
//!
//! Sub-phase 7.2 implementation:
//! - Thread-safe `MeshTopologyGraph` managing known nodes (`MeshNode`) and directed links (`MeshLink`).
//! - BGP-style path-vector loop prevention (`route_path` inspection), sequence number freshness checks,
//!   and maximum hop limit bounds (`MAX_MESH_TTL = 5`).
//! - Deterministic Dijkstra shortest-path route calculation over composite link cost:
//!   `Cost(e) = DEFAULT_HOP_PENALTY_MS + e.rtt_ms`
//! - Automatic link expiration and dynamic rerouting failover.

use std::collections::{BinaryHeap, HashMap, HashSet};

use lanflare_proto::mesh::{
    MeshLinkMetric, MeshNodeAnnouncement, MeshRouteAdvertisement, DEFAULT_HOP_PENALTY_MS,
    MAX_MESH_TTL,
};
use serde::{Deserialize, Serialize};

/// Default stale link expiration threshold (30 seconds).
pub const DEFAULT_LINK_EXPIRY_MS: u64 = 30_000;

/// Default stale node expiration threshold (120 seconds).
pub const DEFAULT_NODE_EXPIRY_MS: u64 = 120_000;

/// Errors encountered during mesh graph mutations and route resolution.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MeshGraphError {
    #[error("Loop detected in route path or origin: {0}")]
    LoopDetected(String),

    #[error("Route exceeds maximum TTL {max_ttl}: path has {hops} hops")]
    TtlExceeded { hops: usize, max_ttl: u8 },

    #[error("Stale sequence number: current {current}, received {received}")]
    StaleSequenceNumber { current: u64, received: u64 },

    #[error("Invalid node announcement: {0}")]
    InvalidAnnouncement(String),

    #[error("Target node not found or unreachable: {0}")]
    Unreachable(String),
}

/// Known mesh node in the overlay topology graph.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshNode {
    /// Canonical uppercase SHA-256 fingerprint hex string (64 characters).
    pub fingerprint: String,

    /// Human-readable device alias.
    pub alias: String,

    /// PEM-encoded X.509 self-signed certificate, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,

    /// Advertised node capabilities.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,

    /// Whether this node acts as an intermediate packet/stream relay.
    pub relay_enabled: bool,

    /// Monotonically increasing sequence number for freshness verification.
    pub seq_no: u64,

    /// Timestamp in milliseconds since UNIX epoch when this node was last observed.
    pub last_seen_ms: u64,
}

impl MeshNode {
    /// Creates a new `MeshNode` descriptor.
    pub fn new(fingerprint: impl Into<String>, alias: impl Into<String>, relay_enabled: bool) -> Self {
        Self {
            fingerprint: fingerprint.into().to_ascii_uppercase(),
            alias: alias.into(),
            cert_pem: None,
            capabilities: Vec::new(),
            relay_enabled,
            seq_no: 0,
            last_seen_ms: 0,
        }
    }

    /// Constructs a `MeshNode` from a cryptographically signed `MeshNodeAnnouncement`.
    pub fn from_announcement(ann: &MeshNodeAnnouncement) -> Self {
        Self {
            fingerprint: ann.fingerprint.to_ascii_uppercase(),
            alias: ann.alias.clone(),
            cert_pem: Some(ann.cert_pem.clone()),
            capabilities: ann.capabilities.clone(),
            relay_enabled: ann.relay_enabled,
            seq_no: ann.seq_no,
            last_seen_ms: ann.timestamp_ms,
        }
    }
}

/// Directed link between two adjacent nodes in the overlay mesh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshLink {
    /// SHA-256 fingerprint of the source node (canonical uppercase).
    pub src_fp: String,

    /// SHA-256 fingerprint of the destination node (canonical uppercase).
    pub dst_fp: String,

    /// Measured round-trip latency in milliseconds.
    pub rtt_ms: u32,

    /// Optional subnet or interface tag (e.g. "wlan0", "eth0", "192.168.1.0/24").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet_tag: Option<String>,

    /// Estimated link throughput in kbps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_kbps: Option<u32>,

    /// Timestamp in milliseconds since UNIX epoch when this link was last observed.
    pub last_seen_ms: u64,
}

impl MeshLink {
    /// Creates a new directed link between `src_fp` and `dst_fp`.
    pub fn new(src_fp: impl Into<String>, dst_fp: impl Into<String>, rtt_ms: u32, now_ms: u64) -> Self {
        Self {
            src_fp: src_fp.into().to_ascii_uppercase(),
            dst_fp: dst_fp.into().to_ascii_uppercase(),
            rtt_ms,
            subnet_tag: None,
            bandwidth_kbps: None,
            last_seen_ms: now_ms,
        }
    }

    /// Constructs a `MeshLink` from a protocol `MeshLinkMetric`.
    pub fn from_metric(metric: &MeshLinkMetric) -> Self {
        Self {
            src_fp: metric.src_fp.to_ascii_uppercase(),
            dst_fp: metric.dst_fp.to_ascii_uppercase(),
            rtt_ms: metric.rtt_ms,
            subnet_tag: metric.subnet_tag.clone(),
            bandwidth_kbps: metric.bandwidth_kbps,
            last_seen_ms: metric.last_seen_ms,
        }
    }

    /// Computes the composite link cost: `HOP_PENALTY_MS + rtt_ms`.
    pub fn cost(&self, hop_penalty_ms: u32) -> u32 {
        hop_penalty_ms.saturating_add(self.rtt_ms)
    }

    /// Checks if this link has expired given current time and expiry threshold.
    pub fn is_stale(&self, now_ms: u64, expiry_ms: u64) -> bool {
        now_ms.saturating_sub(self.last_seen_ms) > expiry_ms
    }
}

/// Resolved optimal multi-hop route returned by Dijkstra shortest-path resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRoute {
    /// Fingerprint of the immediate 1-hop neighbor to forward to.
    pub next_hop_fp: String,

    /// Full traversed path from origin to destination inclusive: `[local_fp, hop1, ..., dst_fp]`.
    pub full_route_path: Vec<String>,

    /// Total accumulated composite cost: `sum(HOP_PENALTY_MS + rtt_ms)`.
    pub total_cost_ms: u32,

    /// Total number of hops traversed (`full_route_path.len() - 1`).
    pub hop_count: u8,
}

impl ResolvedRoute {
    /// Decomposes into tuple `(next_hop_fp, full_route_path, total_cost_ms)`.
    pub fn into_parts(self) -> (String, Vec<String>, u32) {
        (self.next_hop_fp, self.full_route_path, self.total_cost_ms)
    }

    /// Returns references as a tuple `(&next_hop_fp, &full_route_path, total_cost_ms)`.
    pub fn as_tuple(&self) -> (&str, &[String], u32) {
        (&self.next_hop_fp, &self.full_route_path, self.total_cost_ms)
    }
}

impl From<ResolvedRoute> for (String, Vec<String>, u32) {
    fn from(route: ResolvedRoute) -> Self {
        (route.next_hop_fp, route.full_route_path, route.total_cost_ms)
    }
}

/// Link-state and path-vector topology graph and Dijkstra route resolver.
#[derive(Clone, Debug)]
pub struct MeshTopologyGraph {
    /// Canonical SHA-256 fingerprint of the local node.
    local_fp: String,

    /// Penalty added per hop to prevent unnecessary route length inflation.
    hop_penalty_ms: u32,

    /// Maximum Time-To-Live / hop count bound.
    max_ttl: u8,

    /// Inactivity threshold for stale link eviction (ms).
    link_expiry_ms: u64,

    /// Registered known mesh nodes, keyed by canonical uppercase fingerprint.
    nodes: HashMap<String, MeshNode>,

    /// Directed links, keyed by `(src_fp, dst_fp)`.
    links: HashMap<(String, String), MeshLink>,

    /// Outgoing adjacency index: `src_fp -> HashSet<dst_fp>`.
    adj: HashMap<String, HashSet<String>>,

    /// Stored path-vector route advertisements, keyed by `origin_fp`.
    route_advertisements: HashMap<String, MeshRouteAdvertisement>,
}

impl MeshTopologyGraph {
    /// Creates a new `MeshTopologyGraph` for the local node with standard defaults.
    pub fn new(local_fp: impl Into<String>) -> Self {
        Self::with_config(
            local_fp,
            DEFAULT_HOP_PENALTY_MS,
            MAX_MESH_TTL,
            DEFAULT_LINK_EXPIRY_MS,
        )
    }

    /// Creates a new `MeshTopologyGraph` with custom parameters.
    pub fn with_config(
        local_fp: impl Into<String>,
        hop_penalty_ms: u32,
        max_ttl: u8,
        link_expiry_ms: u64,
    ) -> Self {
        let canonical_local = local_fp.into().to_ascii_uppercase();
        let mut nodes = HashMap::new();
        nodes.insert(
            canonical_local.clone(),
            MeshNode::new(canonical_local.clone(), "Self", true),
        );

        Self {
            local_fp: canonical_local,
            hop_penalty_ms,
            max_ttl,
            link_expiry_ms,
            nodes,
            links: HashMap::new(),
            adj: HashMap::new(),
            route_advertisements: HashMap::new(),
        }
    }

    /// Returns the local node's canonical fingerprint.
    pub fn local_fingerprint(&self) -> &str {
        &self.local_fp
    }

    /// Returns the configured hop penalty in milliseconds.
    pub fn hop_penalty_ms(&self) -> u32 {
        self.hop_penalty_ms
    }

    /// Returns the configured maximum TTL / hop count.
    pub fn max_ttl(&self) -> u8 {
        self.max_ttl
    }

    /// Returns the link expiry threshold in milliseconds.
    pub fn link_expiry_ms(&self) -> u64 {
        self.link_expiry_ms
    }

    /// Returns the total count of known nodes.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Returns the total count of directed links.
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Returns the total count of stored route advertisements.
    pub fn route_advertisements_count(&self) -> usize {
        self.route_advertisements.len()
    }

    /// Looks up a node by its fingerprint.
    pub fn get_node(&self, fp: &str) -> Option<&MeshNode> {
        let canonical = fp.to_ascii_uppercase();
        self.nodes.get(&canonical)
    }

    /// Looks up a directed link between `src_fp` and `dst_fp`.
    pub fn get_link(&self, src_fp: &str, dst_fp: &str) -> Option<&MeshLink> {
        let key = (src_fp.to_ascii_uppercase(), dst_fp.to_ascii_uppercase());
        self.links.get(&key)
    }

    /// Looks up a stored path-vector route advertisement by origin fingerprint.
    pub fn get_route_advertisement(&self, origin_fp: &str) -> Option<&MeshRouteAdvertisement> {
        let canonical = origin_fp.to_ascii_uppercase();
        self.route_advertisements.get(&canonical)
    }

    /// Iterates over all known nodes.
    pub fn nodes(&self) -> impl Iterator<Item = &MeshNode> {
        self.nodes.values()
    }

    /// Iterates over all directed links.
    pub fn links(&self) -> impl Iterator<Item = &MeshLink> {
        self.links.values()
    }

    /// Iterates over all stored route advertisements.
    pub fn route_advertisements(&self) -> impl Iterator<Item = &MeshRouteAdvertisement> {
        self.route_advertisements.values()
    }

    /// Registers or updates a node descriptor.
    pub fn insert_node(&mut self, mut node: MeshNode) {
        node.fingerprint = node.fingerprint.to_ascii_uppercase();
        self.nodes.insert(node.fingerprint.clone(), node);
    }

    /// Removes a node and all its incident links from the graph.
    pub fn remove_node(&mut self, fp: &str) -> Option<MeshNode> {
        let canonical = fp.to_ascii_uppercase();
        let removed = self.nodes.remove(&canonical);
        self.route_advertisements.remove(&canonical);

        // Remove outgoing links
        if let Some(targets) = self.adj.remove(&canonical) {
            for target in targets {
                self.links.remove(&(canonical.clone(), target));
            }
        }

        // Remove incoming links
        let incoming_srcs: Vec<String> = self
            .adj
            .iter()
            .filter(|(_, dsts)| dsts.contains(&canonical))
            .map(|(src, _)| src.clone())
            .collect();

        for src in incoming_srcs {
            if let Some(dsts) = self.adj.get_mut(&src) {
                dsts.remove(&canonical);
            }
            self.links.remove(&(src, canonical.clone()));
        }

        removed
    }

    /// Registers or updates a directed link in the topology graph.
    pub fn insert_link(&mut self, link: MeshLink) {
        let key = (link.src_fp.clone(), link.dst_fp.clone());
        self.adj
            .entry(link.src_fp.clone())
            .or_default()
            .insert(link.dst_fp.clone());
        self.links.insert(key, link);
    }

    /// Registers a symmetric bidirectional link with the same RTT in both directions.
    pub fn add_bidirectional_link(&mut self, fp_a: &str, fp_b: &str, rtt_ms: u32, now_ms: u64) {
        let link_ab = MeshLink::new(fp_a, fp_b, rtt_ms, now_ms);
        let link_ba = MeshLink::new(fp_b, fp_a, rtt_ms, now_ms);
        self.insert_link(link_ab);
        self.insert_link(link_ba);
    }

    /// Removes a directed link between `src_fp` and `dst_fp`.
    pub fn remove_link(&mut self, src_fp: &str, dst_fp: &str) -> Option<MeshLink> {
        let src = src_fp.to_ascii_uppercase();
        let dst = dst_fp.to_ascii_uppercase();
        if let Some(neighbors) = self.adj.get_mut(&src) {
            neighbors.remove(&dst);
        }
        self.links.remove(&(src, dst))
    }

    /// Removes a symmetric bidirectional link between `fp_a` and `fp_b`.
    pub fn remove_bidirectional_link(&mut self, fp_a: &str, fp_b: &str) {
        self.remove_link(fp_a, fp_b);
        self.remove_link(fp_b, fp_a);
    }

    /// Ingests a signed `MeshNodeAnnouncement` descriptor.
    ///
    /// Validates sequence number freshness against any existing node record.
    /// Returns `Ok(true)` if inserted or updated, `Ok(false)` if duplicate,
    /// or `Err(MeshGraphError::StaleSequenceNumber)` if stale.
    pub fn process_node_announcement(
        &mut self,
        ann: &MeshNodeAnnouncement,
    ) -> Result<bool, MeshGraphError> {
        let canonical_fp = ann.fingerprint.to_ascii_uppercase();

        if let Some(existing) = self.nodes.get(&canonical_fp) {
            if existing.seq_no > ann.seq_no {
                return Err(MeshGraphError::StaleSequenceNumber {
                    current: existing.seq_no,
                    received: ann.seq_no,
                });
            }
            if existing.seq_no == ann.seq_no {
                return Ok(false);
            }
        }

        let node = MeshNode::from_announcement(ann);
        self.nodes.insert(canonical_fp, node);
        Ok(true)
    }

    /// Ingests a measured 1-hop `MeshLinkMetric`.
    pub fn process_link_metric(&mut self, metric: &MeshLinkMetric) {
        let link = MeshLink::from_metric(metric);
        self.insert_link(link);
    }

    /// Ingests and validates a path-vector `MeshRouteAdvertisement`.
    ///
    /// **Loop Prevention Invariant**:
    /// Immediately drops advertisements containing the local node's fingerprint in `route_path`,
    /// having `origin_fp == local_fp`, having internal loops, exceeding maximum TTL,
    /// or carrying stale sequence numbers.
    ///
    /// Discarded advertisements cause zero graph mutations.
    pub fn process_route_advertisement(
        &mut self,
        adv: &MeshRouteAdvertisement,
        _now_ms: u64,
    ) -> Result<(), MeshGraphError> {
        let canonical_origin = adv.origin_fp.to_ascii_uppercase();

        // 1. Loop check: origin must not be self
        if canonical_origin == self.local_fp {
            return Err(MeshGraphError::LoopDetected(format!(
                "Origin {} is self",
                self.local_fp
            )));
        }

        // 2. Loop check: local node must not appear in route_path
        if adv
            .route_path
            .iter()
            .any(|fp| fp.eq_ignore_ascii_case(&self.local_fp))
        {
            return Err(MeshGraphError::LoopDetected(format!(
                "Local node {} present in route_path {:?}",
                self.local_fp, adv.route_path
            )));
        }

        // 3. Loop check: route_path must not contain internal duplicate hops
        let mut seen = HashSet::with_capacity(adv.route_path.len());
        for hop in &adv.route_path {
            let canonical_hop = hop.to_ascii_uppercase();
            if !seen.insert(canonical_hop) {
                return Err(MeshGraphError::LoopDetected(format!(
                    "Internal cycle detected in route_path: duplicate hop {}",
                    hop
                )));
            }
        }

        // 4. TTL and hop count check
        if adv.ttl == 0 {
            return Err(MeshGraphError::TtlExceeded {
                hops: 0,
                max_ttl: self.max_ttl,
            });
        }
        if adv.route_path.len() > self.max_ttl as usize {
            return Err(MeshGraphError::TtlExceeded {
                hops: adv.route_path.len(),
                max_ttl: self.max_ttl,
            });
        }

        // 5. Sequence number freshness check
        if let Some(existing_node) = self.nodes.get(&canonical_origin)
            && existing_node.seq_no > adv.seq_no
        {
            return Err(MeshGraphError::StaleSequenceNumber {
                current: existing_node.seq_no,
                received: adv.seq_no,
            });
        }
        if let Some(existing_adv) = self.route_advertisements.get(&canonical_origin)
            && existing_adv.seq_no > adv.seq_no
        {
            return Err(MeshGraphError::StaleSequenceNumber {
                current: existing_adv.seq_no,
                received: adv.seq_no,
            });
        }

        // Valid advertisement: mutate graph
        self.nodes
            .entry(canonical_origin.clone())
            .and_modify(|n| {
                if adv.seq_no > n.seq_no {
                    n.seq_no = adv.seq_no;
                }
            })
            .or_insert_with(|| {
                let mut node = MeshNode::new(canonical_origin.clone(), canonical_origin.clone(), true);
                node.seq_no = adv.seq_no;
                node
            });

        let mut canonical_adv = adv.clone();
        canonical_adv.origin_fp = canonical_origin.clone();
        canonical_adv.route_path = adv.route_path.iter().map(|p| p.to_ascii_uppercase()).collect();

        // Synthesize directed links along the advertised path if not already present
        let mut full_path = adv.route_path.clone();
        if !full_path.iter().any(|h| h.eq_ignore_ascii_case(&canonical_origin)) {
            full_path.push(canonical_origin.clone());
        }

        let hop_count = full_path.len().saturating_sub(1).max(1) as u32;
        let avg_hop_rtt = (adv.cost / hop_count).saturating_sub(self.hop_penalty_ms).max(1);
        for window in full_path.windows(2) {
            let u = window[0].to_ascii_uppercase();
            let v = window[1].to_ascii_uppercase();
            if u != v && !self.links.contains_key(&(u.clone(), v.clone())) {
                self.insert_link(MeshLink::new(u, v, avg_hop_rtt, _now_ms));
            }
        }

        self.route_advertisements.insert(canonical_origin, canonical_adv);
        Ok(())
    }

    /// Evicts links whose `last_seen_ms` is older than `link_expiry_ms` threshold.
    ///
    /// Returns the number of links evicted.
    pub fn evict_stale_links(&mut self, now_ms: u64) -> usize {
        let stale_keys: Vec<(String, String)> = self
            .links
            .iter()
            .filter(|(_, link)| link.is_stale(now_ms, self.link_expiry_ms))
            .map(|(k, _)| k.clone())
            .collect();

        let count = stale_keys.len();
        for (src, dst) in stale_keys {
            self.remove_link(&src, &dst);
        }

        count
    }

    /// Evicts inactive nodes whose `last_seen_ms` is older than `node_expiry_ms`.
    /// Does not evict the local node.
    pub fn evict_stale_nodes(&mut self, now_ms: u64, node_expiry_ms: u64) -> usize {
        let stale_fps: Vec<String> = self
            .nodes
            .iter()
            .filter(|(fp, node)| {
                *fp != &self.local_fp && now_ms.saturating_sub(node.last_seen_ms) > node_expiry_ms
            })
            .map(|(fp, _)| fp.clone())
            .collect();

        let count = stale_fps.len();
        for fp in stale_fps {
            self.remove_node(&fp);
        }

        count
    }

    /// Single periodic tick: evicts stale links.
    pub fn tick(&mut self, now_ms: u64) -> usize {
        self.evict_stale_links(now_ms)
    }

    /// Resolves the optimal route from local node to `target_fp` using Dijkstra shortest path.
    pub fn resolve_route(&self, target_fp: &str) -> Option<ResolvedRoute> {
        self.resolve_route_from(&self.local_fp, target_fp)
    }

    /// Resolves the optimal route from arbitrary `from_fp` to `target_fp` using Dijkstra shortest path.
    ///
    /// Minimizes composite path cost: `sum(HOP_PENALTY_MS + link.rtt_ms)`.
    /// Respects `max_ttl` hop bounds and intermediate node `relay_enabled` policies.
    pub fn resolve_route_from(&self, from_fp: &str, target_fp: &str) -> Option<ResolvedRoute> {
        let src = from_fp.to_ascii_uppercase();
        let dst = target_fp.to_ascii_uppercase();

        if src == dst {
            return None;
        }

        // Priority queue state for Dijkstra min-heap
        #[derive(Clone, Eq, PartialEq)]
        struct State {
            cost: u32,
            hops: u8,
            fp: String,
        }

        impl Ord for State {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                // Min-heap ordering: lowest cost, then fewer hops, then lexicographical
                other
                    .cost
                    .cmp(&self.cost)
                    .then_with(|| other.hops.cmp(&self.hops))
                    .then_with(|| other.fp.cmp(&self.fp))
            }
        }

        impl PartialOrd for State {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut heap = BinaryHeap::new();
        // Track best cost per (fingerprint, hop_count) to support hop-constrained shortest path
        let mut dist: HashMap<(String, u8), u32> = HashMap::new();
        // Track predecessor for path reconstruction: (curr_fp, curr_hops) -> (prev_fp, prev_hops)
        let mut prev: HashMap<(String, u8), (String, u8)> = HashMap::new();

        dist.insert((src.clone(), 0), 0);
        heap.push(State {
            cost: 0,
            hops: 0,
            fp: src.clone(),
        });

        while let Some(State { cost, hops, fp }) = heap.pop() {
            // Target reached! Because edge costs are strictly positive (>0), the first
            // pop of the target node guarantees the minimal composite path cost.
            if fp == dst {
                let mut path = Vec::new();
                let mut curr = (fp.clone(), hops);

                while curr.0 != src {
                    path.push(curr.0.clone());
                    if let Some(parent) = prev.get(&curr) {
                        curr = parent.clone();
                    } else {
                        break;
                    }
                }
                path.push(src.clone());
                path.reverse();

                if path.len() < 2 {
                    return None;
                }

                let next_hop = path[1].clone();
                return Some(ResolvedRoute {
                    next_hop_fp: next_hop,
                    full_route_path: path,
                    total_cost_ms: cost,
                    hop_count: hops,
                });
            }

            // If we have already seen a strictly lower cost for this (node, hop), skip
            if let Some(&best) = dist.get(&(fp.clone(), hops))
                && cost > best
            {
                continue;
            }

            // Cannot forward beyond max_ttl
            if hops >= self.max_ttl {
                continue;
            }

            // Intermediate nodes must have relaying enabled
            if fp != src && !self.can_relay(&fp) {
                continue;
            }

            // Expand adjacent links
            if let Some(neighbors) = self.adj.get(&fp) {
                for next in neighbors {
                    let next_hops = hops + 1;
                    if next_hops > self.max_ttl {
                        continue;
                    }

                    if let Some(link) = self.links.get(&(fp.clone(), next.clone())) {
                        let link_cost = link.cost(self.hop_penalty_ms);
                        let next_cost = cost.saturating_add(link_cost);

                        let dominated = dist
                            .get(&(next.clone(), next_hops))
                            .is_some_and(|&c| c <= next_cost);

                        if !dominated {
                            dist.insert((next.clone(), next_hops), next_cost);
                            prev.insert((next.clone(), next_hops), (fp.clone(), hops));
                            heap.push(State {
                                cost: next_cost,
                                hops: next_hops,
                                fp: next.clone(),
                            });
                        }
                    }
                }
            }
        }

        None
    }

    /// Generates an outgoing `MeshRouteAdvertisement` to propagate route to `target_fp` to 1-hop neighbors.
    pub fn create_route_advertisement(&self, target_fp: &str) -> Option<MeshRouteAdvertisement> {
        let route = self.resolve_route(target_fp)?;
        let canonical_target = target_fp.to_ascii_uppercase();
        let target_seq = self
            .nodes
            .get(&canonical_target)
            .map(|n| n.seq_no)
            .unwrap_or(1);

        let remaining_ttl = self.max_ttl.saturating_sub(route.hop_count);
        if remaining_ttl == 0 {
            return None;
        }

        Some(MeshRouteAdvertisement {
            origin_fp: canonical_target,
            seq_no: target_seq,
            cost: route.total_cost_ms,
            route_path: route.full_route_path,
            ttl: remaining_ttl,
        })
    }

    /// Generates route advertisements for all reachable nodes in the graph.
    pub fn generate_all_route_advertisements(&self) -> Vec<MeshRouteAdvertisement> {
        let mut ads = Vec::new();
        for node in self.nodes.values() {
            if node.fingerprint == self.local_fp {
                continue;
            }
            if let Some(adv) = self.create_route_advertisement(&node.fingerprint) {
                ads.push(adv);
            }
        }
        ads
    }

    /// Internal check if a node can serve as an intermediate multi-hop relay.
    fn can_relay(&self, fp: &str) -> bool {
        if let Some(node) = self.nodes.get(fp) {
            node.relay_enabled
        } else {
            true // Allow ad-hoc links unless explicitly registered with relay_enabled=false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_vector_loop_rejection() {
        let mut graph = MeshTopologyGraph::new("NODE_LOCAL");
        graph.insert_node(MeshNode::new("NODE_LOCAL", "Local", true));
        graph.insert_node(MeshNode::new("NODE_B", "Node B", true));

        let nodes_before = graph.node_count();
        let links_before = graph.link_count();
        let advs_before = graph.route_advertisements_count();

        // 1. Route advertisement containing local node in route_path
        let adv_with_local = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 1,
            cost: 50,
            route_path: vec!["NODE_B".into(), "NODE_LOCAL".into(), "NODE_D".into()],
            ttl: 3,
        };
        let err = graph.process_route_advertisement(&adv_with_local, 1000).unwrap_err();
        assert!(matches!(err, MeshGraphError::LoopDetected(_)));

        // Invariant: ZERO graph mutation on loop rejection
        assert_eq!(graph.node_count(), nodes_before);
        assert_eq!(graph.link_count(), links_before);
        assert_eq!(graph.route_advertisements_count(), advs_before);
        assert!(graph.get_route_advertisement("NODE_D").is_none());

        // 2. Case-insensitive local node in route_path
        let adv_case_insensitive = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 1,
            cost: 50,
            route_path: vec!["node_local".into(), "NODE_D".into()],
            ttl: 3,
        };
        let err = graph
            .process_route_advertisement(&adv_case_insensitive, 1000)
            .unwrap_err();
        assert!(matches!(err, MeshGraphError::LoopDetected(_)));
        assert_eq!(graph.node_count(), nodes_before);

        // 3. Origin is self
        let adv_self_origin = MeshRouteAdvertisement {
            origin_fp: "NODE_LOCAL".into(),
            seq_no: 1,
            cost: 50,
            route_path: vec!["NODE_B".into()],
            ttl: 3,
        };
        let err = graph.process_route_advertisement(&adv_self_origin, 1000).unwrap_err();
        assert!(matches!(err, MeshGraphError::LoopDetected(_)));
        assert_eq!(graph.node_count(), nodes_before);

        // 4. Internal cycle in route_path: [B, C, B, D]
        let adv_internal_loop = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 1,
            cost: 50,
            route_path: vec!["NODE_B".into(), "NODE_C".into(), "NODE_B".into(), "NODE_D".into()],
            ttl: 3,
        };
        let err = graph.process_route_advertisement(&adv_internal_loop, 1000).unwrap_err();
        assert!(matches!(err, MeshGraphError::LoopDetected(_)));
        assert_eq!(graph.node_count(), nodes_before);

        // 5. TTL = 0
        let adv_zero_ttl = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 1,
            cost: 50,
            route_path: vec!["NODE_B".into(), "NODE_D".into()],
            ttl: 0,
        };
        let err = graph.process_route_advertisement(&adv_zero_ttl, 1000).unwrap_err();
        assert!(matches!(err, MeshGraphError::TtlExceeded { .. }));
        assert_eq!(graph.node_count(), nodes_before);

        // 6. Valid advertisement accepts and updates graph
        let valid_adv = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 5,
            cost: 50,
            route_path: vec!["NODE_B".into(), "NODE_D".into()],
            ttl: 4,
        };
        assert!(graph.process_route_advertisement(&valid_adv, 1000).is_ok());
        assert_eq!(graph.get_route_advertisement("NODE_D").unwrap().seq_no, 5);

        // 7. Stale sequence number (seq_no 3 < current 5) rejected with zero mutation
        let stale_adv = MeshRouteAdvertisement {
            origin_fp: "NODE_D".into(),
            seq_no: 3,
            cost: 40,
            route_path: vec!["NODE_B".into(), "NODE_D".into()],
            ttl: 4,
        };
        let err = graph.process_route_advertisement(&stale_adv, 1000).unwrap_err();
        assert!(matches!(
            err,
            MeshGraphError::StaleSequenceNumber {
                current: 5,
                received: 3
            }
        ));
        assert_eq!(graph.get_route_advertisement("NODE_D").unwrap().seq_no, 5);
    }

    #[test]
    fn test_dijkstra_lowest_cost_multi_hop_selection() {
        // Setup 4-node diamond topology:
        //
        //          Node B (Relay, RTT: 10ms)
        //        /                           \
        //  Node A                             Node D (Target)
        //        \                           /
        //          Node C (Relay, RTT: 40ms)
        //
        let mut graph = MeshTopologyGraph::new("NODE_A");
        graph.insert_node(MeshNode::new("NODE_A", "Node A", true));
        graph.insert_node(MeshNode::new("NODE_B", "Node B", true));
        graph.insert_node(MeshNode::new("NODE_C", "Node C", true));
        graph.insert_node(MeshNode::new("NODE_D", "Node D", true));

        // Fast path via Node B:
        // Link A <-> B (10ms): cost = 15 + 10 = 25ms
        // Link B <-> D (10ms): cost = 15 + 10 = 25ms
        // Total cost A -> B -> D = 50ms (2 hops)
        graph.add_bidirectional_link("NODE_A", "NODE_B", 10, 1000);
        graph.add_bidirectional_link("NODE_B", "NODE_D", 10, 1000);

        // Slower path via Node C:
        // Link A <-> C (40ms): cost = 15 + 40 = 55ms
        // Link C <-> D (40ms): cost = 15 + 40 = 55ms
        // Total cost A -> C -> D = 110ms (2 hops)
        graph.add_bidirectional_link("NODE_A", "NODE_C", 40, 1000);
        graph.add_bidirectional_link("NODE_C", "NODE_D", 40, 1000);

        // Dijkstra route resolution
        let route = graph.resolve_route("NODE_D").expect("route must resolve");

        assert_eq!(route.next_hop_fp, "NODE_B");
        assert_eq!(
            route.full_route_path,
            vec!["NODE_A".to_string(), "NODE_B".to_string(), "NODE_D".to_string()]
        );
        assert_eq!(route.total_cost_ms, 50);
        assert_eq!(route.hop_count, 2);

        // Tuple extraction verification
        let (next_hop, path, cost) = route.into_parts();
        assert_eq!(next_hop, "NODE_B");
        assert_eq!(path, vec!["NODE_A", "NODE_B", "NODE_D"]);
        assert_eq!(cost, 50);

        // Create advertisement for neighbors
        let adv = graph.create_route_advertisement("NODE_D").expect("adv generated");
        assert_eq!(adv.origin_fp, "NODE_D");
        assert_eq!(adv.cost, 50);
        assert_eq!(adv.route_path, vec!["NODE_A", "NODE_B", "NODE_D"]);
        assert_eq!(adv.ttl, MAX_MESH_TTL - 2);
    }

    #[test]
    fn test_relay_link_failure_automatic_reroute() {
        // Setup the same diamond topology with timestamp tracking
        let mut graph = MeshTopologyGraph::with_config("NODE_A", 15, 5, 10_000);
        graph.insert_node(MeshNode::new("NODE_A", "Node A", true));
        graph.insert_node(MeshNode::new("NODE_B", "Node B", true));
        graph.insert_node(MeshNode::new("NODE_C", "Node C", true));
        graph.insert_node(MeshNode::new("NODE_D", "Node D", true));

        // Primary path via Node B last observed at t = 1000
        graph.add_bidirectional_link("NODE_A", "NODE_B", 10, 1000);
        graph.add_bidirectional_link("NODE_B", "NODE_D", 10, 1000);

        // Backup path via Node C recently active at t = 12000
        graph.add_bidirectional_link("NODE_A", "NODE_C", 40, 12000);
        graph.add_bidirectional_link("NODE_C", "NODE_D", 40, 12000);

        // Initially, route selects Node B (cost 50 vs 110)
        let initial_route = graph.resolve_route("NODE_D").unwrap();
        assert_eq!(initial_route.next_hop_fp, "NODE_B");
        assert_eq!(initial_route.total_cost_ms, 50);

        // At t = 15000, 14 seconds have passed since Node B was last seen.
        // Threshold is 10_000ms. Node B links expire!
        let evicted = graph.tick(15000);
        assert_eq!(evicted, 4); // 2 bidirectional links = 4 directed edges

        // Failover within 1 tick: automatically reroutes to Node C!
        let failover_route = graph
            .resolve_route("NODE_D")
            .expect("should failover to surviving relay C");

        assert_eq!(failover_route.next_hop_fp, "NODE_C");
        assert_eq!(
            failover_route.full_route_path,
            vec!["NODE_A".to_string(), "NODE_C".to_string(), "NODE_D".to_string()]
        );
        assert_eq!(failover_route.total_cost_ms, 110);
        assert_eq!(failover_route.hop_count, 2);
    }

    #[test]
    fn test_relay_disabled_node_bypass() {
        let mut graph = MeshTopologyGraph::new("NODE_A");
        graph.insert_node(MeshNode::new("NODE_A", "Node A", true));
        // Node B has relaying disabled!
        graph.insert_node(MeshNode::new("NODE_B", "Node B (No Relay)", false));
        graph.insert_node(MeshNode::new("NODE_C", "Node C (Relay)", true));
        graph.insert_node(MeshNode::new("NODE_D", "Node D", false)); // Destination does not need relaying

        // Path via B is faster (10ms)
        graph.add_bidirectional_link("NODE_A", "NODE_B", 10, 1000);
        graph.add_bidirectional_link("NODE_B", "NODE_D", 10, 1000);

        // Path via C is slower (40ms)
        graph.add_bidirectional_link("NODE_A", "NODE_C", 40, 1000);
        graph.add_bidirectional_link("NODE_C", "NODE_D", 40, 1000);

        // Because Node B cannot act as a relay, Dijkstra MUST bypass Node B and route via Node C!
        let route = graph.resolve_route("NODE_D").expect("route via C must succeed");
        assert_eq!(route.next_hop_fp, "NODE_C");
        assert_eq!(route.full_route_path, vec!["NODE_A", "NODE_C", "NODE_D"]);
        assert_eq!(route.total_cost_ms, 110);
    }

    #[test]
    fn test_max_ttl_hop_count_bounding() {
        let mut graph = MeshTopologyGraph::with_config("N0", 15, 3, 30_000); // max_ttl = 3

        // Linear chain: N0 <-> N1 <-> N2 <-> N3 <-> N4
        for i in 0..=4 {
            graph.insert_node(MeshNode::new(format!("N{i}"), format!("Node {i}"), true));
        }
        for i in 0..4 {
            graph.add_bidirectional_link(&format!("N{i}"), &format!("N{}", i + 1), 10, 1000);
        }

        // 1 hop: N1 reachable
        assert!(graph.resolve_route("N1").is_some());
        // 2 hops: N2 reachable
        assert!(graph.resolve_route("N2").is_some());
        // 3 hops: N3 reachable (3 hops == max_ttl)
        let r3 = graph.resolve_route("N3").expect("3 hops matches max_ttl");
        assert_eq!(r3.hop_count, 3);
        // 4 hops: N4 unreachable (4 hops > max_ttl 3)
        assert!(graph.resolve_route("N4").is_none());
    }

    #[test]
    fn test_node_announcement_processing() {
        let mut graph = MeshTopologyGraph::new("LOCAL");

        let ann = MeshNodeAnnouncement {
            fingerprint: "PEER_1".into(),
            alias: "Peer One".into(),
            cert_pem: "-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----".into(),
            capabilities: vec!["relay".into(), "chat".into()],
            relay_enabled: true,
            seq_no: 10,
            timestamp_ms: 5000,
            signature: "sig123".into(),
        };

        // First ingestion succeeds
        assert!(graph.process_node_announcement(&ann).unwrap());
        let node = graph.get_node("PEER_1").unwrap();
        assert_eq!(node.alias, "Peer One");
        assert_eq!(node.seq_no, 10);
        assert!(node.relay_enabled);

        // Duplicate ingestion returns false
        assert!(!graph.process_node_announcement(&ann).unwrap());

        // Stale sequence number returns error
        let mut stale_ann = ann.clone();
        stale_ann.seq_no = 8;
        let err = graph.process_node_announcement(&stale_ann).unwrap_err();
        assert!(matches!(
            err,
            MeshGraphError::StaleSequenceNumber {
                current: 10,
                received: 8
            }
        ));

        // Fresh sequence number updates node
        let mut fresh_ann = ann.clone();
        fresh_ann.seq_no = 11;
        fresh_ann.alias = "Peer One Updated".into();
        assert!(graph.process_node_announcement(&fresh_ann).unwrap());
        assert_eq!(graph.get_node("PEER_1").unwrap().alias, "Peer One Updated");
        assert_eq!(graph.get_node("PEER_1").unwrap().seq_no, 11);
    }
}
