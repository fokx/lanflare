//! In-process virtual network simulator (`VirtualNetworkMatrix`) and multi-subnet test harness.
//!
//! Sub-phase 7.4 implementation:
//! - `VirtualNetworkMatrix`: Interceptor enforcing virtual subnet partitions (`Subnet A`, `Subnet B`, `Subnet C`),
//!   artificial link latency/jitter, packet drop injection, and dynamic link severing.
//! - `SimulatedMeshCluster`: Spawns $N$ isolated LanFlare nodes in a single Tokio runtime on ephemeral ports,
//!   wires their `MeshRouter`s with virtual network interception, and orchestrates gossip propagation and registry synchronization.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::crypto::cert::TlsIdentity;
use crate::discovery::registry::PeerRegistry;
use crate::mesh::graph::{MeshNode, MeshTopologyGraph};
use crate::mesh::router::{MeshNetworkInterceptor, MeshRouter, MeshRouterConfig, RouterError};
use crate::server::ServerHandle;

/// Programmable link configuration for simulated virtual network links.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulatedLinkConfig {
    /// Injected round-trip latency in milliseconds.
    pub latency_ms: u32,

    /// Injected latency jitter in milliseconds (random 0..jitter_ms added to latency).
    pub jitter_ms: u32,

    /// Simulated packet drop probability (0.0 = no loss, 1.0 = 100% loss).
    pub drop_rate: f64,

    /// Whether this link is dynamically severed / cut.
    pub severed: bool,

    /// Subnet or interface tag identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet_tag: Option<String>,
}

impl Default for SimulatedLinkConfig {
    fn default() -> Self {
        Self {
            latency_ms: 0,
            jitter_ms: 0,
            drop_rate: 0.0,
            severed: false,
            subnet_tag: None,
        }
    }
}

impl SimulatedLinkConfig {
    /// Creates a new default `SimulatedLinkConfig`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the link latency in milliseconds.
    pub fn with_latency(mut self, latency_ms: u32) -> Self {
        self.latency_ms = latency_ms;
        self
    }

    /// Sets the maximum randomized jitter in milliseconds.
    pub fn with_jitter(mut self, jitter_ms: u32) -> Self {
        self.jitter_ms = jitter_ms;
        self
    }

    /// Sets the packet drop rate (clamped between 0.0 and 1.0).
    pub fn with_drop_rate(mut self, drop_rate: f64) -> Self {
        self.drop_rate = drop_rate.clamp(0.0, 1.0);
        self
    }

    /// Sets whether the link is currently severed.
    pub fn with_severed(mut self, severed: bool) -> Self {
        self.severed = severed;
        self
    }

    /// Sets the subnet tag.
    pub fn with_subnet(mut self, subnet: impl Into<String>) -> Self {
        self.subnet_tag = Some(subnet.into());
        self
    }
}

/// Metadata recorded for a node in the virtual network matrix.
#[derive(Clone, Debug)]
pub struct MatrixNodeInfo {
    /// Canonical uppercase SHA-256 fingerprint (64 characters).
    pub fingerprint: String,

    /// Node alias.
    pub alias: String,

    /// Set of subnets this node belongs to (e.g. `{"SubnetA", "SubnetB"}`).
    pub subnets: HashSet<String>,
}

/// In-process virtual network matrix controlling subnet reachability and link faults.
#[derive(Debug)]
pub struct VirtualNetworkMatrix {
    nodes: std::sync::RwLock<HashMap<String, MatrixNodeInfo>>,
    links: std::sync::RwLock<HashMap<(String, String), SimulatedLinkConfig>>,
    offline_nodes: std::sync::RwLock<HashSet<String>>,
}

impl Default for VirtualNetworkMatrix {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualNetworkMatrix {
    /// Creates a new empty `VirtualNetworkMatrix`.
    pub fn new() -> Self {
        Self {
            nodes: std::sync::RwLock::new(HashMap::new()),
            links: std::sync::RwLock::new(HashMap::new()),
            offline_nodes: std::sync::RwLock::new(HashSet::new()),
        }
    }

    /// Registers a node and its attached virtual subnets.
    pub fn add_node(&self, fp: impl Into<String>, alias: impl Into<String>, subnets: Vec<String>) {
        let canonical = fp.into().to_ascii_uppercase();
        let subnets_set: HashSet<String> = subnets
            .into_iter()
            .map(|s| s.trim().to_ascii_uppercase())
            .collect();

        let info = MatrixNodeInfo {
            fingerprint: canonical.clone(),
            alias: alias.into(),
            subnets: subnets_set,
        };

        let mut nodes = self.nodes.write().unwrap();
        nodes.insert(canonical, info);
    }

    /// Updates the subnet membership for an existing node.
    pub fn set_subnets(&self, fp: &str, subnets: Vec<String>) {
        let canonical = fp.to_ascii_uppercase();
        let subnets_set: HashSet<String> = subnets
            .into_iter()
            .map(|s| s.trim().to_ascii_uppercase())
            .collect();

        let mut nodes = self.nodes.write().unwrap();
        if let Some(n) = nodes.get_mut(&canonical) {
            n.subnets = subnets_set;
        }
    }

    /// Returns the active subnets for a node.
    pub fn get_subnets(&self, fp: &str) -> Vec<String> {
        let canonical = fp.to_ascii_uppercase();
        let nodes = self.nodes.read().unwrap();
        nodes
            .get(&canonical)
            .map(|n| n.subnets.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Sets the simulated link configuration for directed link `src_fp -> dst_fp`.
    pub fn set_link_config(&self, src_fp: &str, dst_fp: &str, config: SimulatedLinkConfig) {
        let src = src_fp.to_ascii_uppercase();
        let dst = dst_fp.to_ascii_uppercase();
        let mut links = self.links.write().unwrap();
        links.insert((src, dst), config);
    }

    /// Sets the simulated link configuration symmetrically for bidirectional link `fp_a <-> fp_b`.
    pub fn set_bidirectional_link_config(&self, fp_a: &str, fp_b: &str, config: SimulatedLinkConfig) {
        let a = fp_a.to_ascii_uppercase();
        let b = fp_b.to_ascii_uppercase();
        let mut links = self.links.write().unwrap();
        links.insert((a.clone(), b.clone()), config.clone());
        links.insert((b, a), config);
    }

    /// Retrieves the active link configuration between two nodes if configured.
    pub fn get_link_config(&self, src_fp: &str, dst_fp: &str) -> Option<SimulatedLinkConfig> {
        let src = src_fp.to_ascii_uppercase();
        let dst = dst_fp.to_ascii_uppercase();
        let links = self.links.read().unwrap();
        links.get(&(src, dst)).cloned()
    }

    /// Dynamically severs the bidirectional link between `fp_a` and `fp_b`.
    pub fn sever_link(&self, fp_a: &str, fp_b: &str) {
        let a = fp_a.to_ascii_uppercase();
        let b = fp_b.to_ascii_uppercase();
        let mut links = self.links.write().unwrap();
        links.entry((a.clone(), b.clone())).or_default().severed = true;
        links.entry((b, a)).or_default().severed = true;
    }

    /// Restores connectivity across the severed bidirectional link between `fp_a` and `fp_b`.
    pub fn restore_link(&self, fp_a: &str, fp_b: &str) {
        let a = fp_a.to_ascii_uppercase();
        let b = fp_b.to_ascii_uppercase();
        let mut links = self.links.write().unwrap();
        links.entry((a.clone(), b.clone())).or_default().severed = false;
        links.entry((b, a)).or_default().severed = false;
    }

    /// Returns whether the link between `fp_a` and `fp_b` is currently severed.
    pub fn is_link_severed(&self, fp_a: &str, fp_b: &str) -> bool {
        let a = fp_a.to_ascii_uppercase();
        let b = fp_b.to_ascii_uppercase();
        let links = self.links.read().unwrap();
        links
            .get(&(a.clone(), b.clone()))
            .map(|c| c.severed)
            .unwrap_or(false)
            || links.get(&(b, a)).map(|c| c.severed).unwrap_or(false)
    }

    /// Sets whether a node is offline (unable to send or receive).
    pub fn set_node_offline(&self, fp: &str, offline: bool) {
        let canonical = fp.to_ascii_uppercase();
        let mut off = self.offline_nodes.write().unwrap();
        if offline {
            off.insert(canonical);
        } else {
            off.remove(&canonical);
        }
    }

    /// Returns true if a node is marked offline.
    pub fn is_node_offline(&self, fp: &str) -> bool {
        let canonical = fp.to_ascii_uppercase();
        let off = self.offline_nodes.read().unwrap();
        off.contains(&canonical)
    }

    /// Evaluates whether `src_fp` and `dst_fp` can communicate directly over 1-hop link.
    ///
    /// Connectivity requires:
    /// 1. Neither node is offline.
    /// 2. The link is not severed.
    /// 3. Both nodes share at least one virtual subnet.
    pub fn can_reach(&self, src_fp: &str, dst_fp: &str) -> bool {
        let src = src_fp.to_ascii_uppercase();
        let dst = dst_fp.to_ascii_uppercase();

        if src == dst {
            return true;
        }

        // 1. Offline check
        {
            let off = self.offline_nodes.read().unwrap();
            if off.contains(&src) || off.contains(&dst) {
                return false;
            }
        }

        // 2. Severed link check
        {
            let links = self.links.read().unwrap();
            if let Some(cfg) = links.get(&(src.clone(), dst.clone()))
                && cfg.severed
            {
                return false;
            }
            if let Some(cfg) = links.get(&(dst.clone(), src.clone()))
                && cfg.severed
            {
                return false;
            }
        }

        // 3. Subnet intersection check
        let nodes = self.nodes.read().unwrap();
        let src_node = match nodes.get(&src) {
            Some(n) => n,
            None => return false,
        };
        let dst_node = match nodes.get(&dst) {
            Some(n) => n,
            None => return false,
        };

        src_node.subnets.intersection(&dst_node.subnets).next().is_some()
    }
}

impl MeshNetworkInterceptor for VirtualNetworkMatrix {
    fn intercept(&self, src_fp: &str, next_hop_fp: &str) -> Result<Duration, RouterError> {
        let src = src_fp.to_ascii_uppercase();
        let dst = next_hop_fp.to_ascii_uppercase();

        if !self.can_reach(&src, &dst) {
            return Err(RouterError::Network(format!(
                "VirtualNetworkMatrix: unreachable link or subnet partition between {src} and {dst}"
            )));
        }

        if let Some(cfg) = self.get_link_config(&src, &dst) {
            if cfg.severed {
                return Err(RouterError::Network(format!(
                    "VirtualNetworkMatrix: link between {src} and {dst} is severed"
                )));
            }

            if cfg.drop_rate > 0.0 {
                let r: f64 = rand::random();
                if r < cfg.drop_rate {
                    return Err(RouterError::Network(format!(
                        "VirtualNetworkMatrix: packet dropped by simulated loss (drop_rate={})",
                        cfg.drop_rate
                    )));
                }
            }

            let mut delay_ms = cfg.latency_ms;
            if cfg.jitter_ms > 0 {
                let jitter: u32 = rand::random::<u32>() % (cfg.jitter_ms + 1);
                delay_ms = delay_ms.saturating_add(jitter);
            }

            return Ok(Duration::from_millis(delay_ms as u64));
        }

        Ok(Duration::ZERO)
    }
}

/// Simulated LanFlare node running in an isolated in-process cluster.
pub struct SimulatedNode {
    /// Canonical uppercase SHA-256 fingerprint.
    pub fingerprint: String,

    /// Node device alias.
    pub alias: String,

    /// Self-signed TLS identity with certificate and private key.
    pub tls_identity: TlsIdentity,

    /// Handle for the running Hyper HTTP server.
    pub server_handle: ServerHandle,

    /// Mesh router managing gossip, forwarding, and streaming.
    pub mesh_router: Arc<MeshRouter>,

    /// Local peer registry deduplicating 1-hop and multi-hop peers.
    pub registry: Arc<PeerRegistry>,

    /// Subnets this node is attached to.
    pub subnets: Vec<String>,

    /// Ephemeral TCP port bound by the HTTP server.
    pub bound_port: u16,

    /// Chat engine instance if chat is enabled on this node.
    pub chat_engine: Option<Arc<crate::chat::engine::ChatEngine>>,
}

impl SimulatedNode {
    /// Returns the canonical fingerprint.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the node alias.
    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// Returns the port the node server is listening on.
    pub fn bound_port(&self) -> u16 {
        self.bound_port
    }

    /// Formats the direct loopback HTTP base URL.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.bound_port)
    }

    /// Returns a reference to the node's mesh router.
    pub fn mesh_router(&self) -> &Arc<MeshRouter> {
        &self.mesh_router
    }

    /// Returns a reference to the node's peer registry.
    pub fn registry(&self) -> &Arc<PeerRegistry> {
        &self.registry
    }

    /// Returns a reference to the node's cryptographic TLS identity.
    pub fn tls_identity(&self) -> &TlsIdentity {
        &self.tls_identity
    }

    /// Returns the subnets this node belongs to.
    pub fn subnets(&self) -> &[String] {
        &self.subnets
    }

    /// Returns a reference to the chat engine if active.
    pub fn chat_engine(&self) -> Option<&Arc<crate::chat::engine::ChatEngine>> {
        self.chat_engine.as_ref()
    }
}

/// In-process simulated cluster managing $N$ isolated LanFlare nodes on localhost.
pub struct SimulatedMeshCluster {
    /// Central virtual network matrix.
    pub matrix: Arc<VirtualNetworkMatrix>,

    /// Active simulated nodes keyed by canonical uppercase fingerprint.
    nodes: HashMap<String, SimulatedNode>,
}

impl SimulatedMeshCluster {
    /// Creates a new cluster using the provided virtual network matrix.
    pub fn new(matrix: Arc<VirtualNetworkMatrix>) -> Self {
        Self {
            matrix,
            nodes: HashMap::new(),
        }
    }

    /// Spawns a new isolated LanFlare node listening on an ephemeral port.
    /// Returns the node's canonical uppercase SHA-256 fingerprint.
    pub async fn spawn_node(
        &mut self,
        alias: &str,
        subnets: Vec<String>,
        relay_enabled: bool,
    ) -> anyhow::Result<String> {
        self.spawn_node_ext(alias, subnets, relay_enabled, false).await
    }

    /// Spawns a new isolated LanFlare node with an active ChatEngine attached.
    pub async fn spawn_node_with_chat(
        &mut self,
        alias: &str,
        subnets: Vec<String>,
        relay_enabled: bool,
    ) -> anyhow::Result<String> {
        self.spawn_node_ext(alias, subnets, relay_enabled, true).await
    }

    /// Extended node spawner supporting optional ChatEngine.
    pub async fn spawn_node_ext(
        &mut self,
        alias: &str,
        subnets: Vec<String>,
        relay_enabled: bool,
        enable_chat: bool,
    ) -> anyhow::Result<String> {
        let ident = crate::crypto::cert::generate_self_signed()?;
        let canonical_fp = ident.fingerprint.clone();

        let registry = PeerRegistry::new();
        let graph = Arc::new(tokio::sync::RwLock::new(MeshTopologyGraph::new(
            canonical_fp.clone(),
        )));

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Insert self node descriptor into its own graph
        {
            let mut g = graph.write().await;
            let mut self_node = MeshNode::new(canonical_fp.clone(), alias, relay_enabled);
            self_node.cert_pem = Some(ident.cert_pem.clone());
            self_node.seq_no = 1;
            self_node.last_seen_ms = now_ms;
            g.insert_node(self_node);
        }

        let config = MeshRouterConfig {
            local_fingerprint: canonical_fp.clone(),
            relay_enabled,
            local_alias: alias.to_string(),
            ..Default::default()
        };

        let mesh_router = Arc::new(MeshRouter::with_config(config, graph));
        mesh_router.set_registry(registry.clone()).await;
        mesh_router.set_interceptor(self.matrix.clone()).await;

        self.matrix
            .add_node(&canonical_fp, alias, subnets.clone());

        let temp_download_dir =
            std::env::temp_dir().join(format!("lanflare_sim_{}_{}", alias, uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_download_dir).await?;

        let app_cfg = crate::config::AppConfig {
            alias: alias.to_string(),
            port: 0, // ephemeral port
            auto_accept: true,
            download_dir: temp_download_dir.clone(),
            ..Default::default()
        };

        let chat_engine = if enable_chat {
            let chat_cfg = crate::chat::ChatEngineConfig::new(alias, &temp_download_dir);
            let ce = crate::chat::ChatEngine::new(ident.clone(), chat_cfg, registry.clone()).await?;
            ce.attach_mesh_router(mesh_router.clone()).await;
            Some(ce)
        } else {
            None
        };

        let server_handle = crate::server::start_server_all(
            app_cfg,
            None, // Plain HTTP on loopback for lightweight simulator execution
            registry.clone(),
            None,
            None,
            None,
            Some(mesh_router.clone()),
            chat_engine.clone(),
        )
        .await?;

        let bound_port = server_handle.bound_port();

        let node = SimulatedNode {
            fingerprint: canonical_fp.clone(),
            alias: alias.to_string(),
            tls_identity: ident,
            server_handle,
            mesh_router,
            registry,
            subnets,
            bound_port,
            chat_engine,
        };

        self.nodes.insert(canonical_fp.clone(), node);
        Ok(canonical_fp)
    }

    /// Connects all node pairs that share at least one subnet and whose link is not severed.
    ///
    /// Establishes 1-hop base URL endpoint mappings, adds bidirectional topology links,
    /// and registers 1-hop peers into `PeerRegistry`.
    pub async fn establish_direct_links(&self) -> usize {
        struct NodeLinkInfo {
            fp: String,
            alias: String,
            port: u16,
            router: Arc<MeshRouter>,
            registry: Arc<PeerRegistry>,
            identity: TlsIdentity,
        }

        let mut count = 0;
        let node_list: Vec<NodeLinkInfo> = self
            .nodes
            .values()
            .map(|n| NodeLinkInfo {
                fp: n.fingerprint.clone(),
                alias: n.alias.clone(),
                port: n.bound_port,
                router: n.mesh_router.clone(),
                registry: n.registry.clone(),
                identity: n.tls_identity.clone(),
            })
            .collect();

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        for i in 0..node_list.len() {
            for j in 0..node_list.len() {
                if i == j {
                    continue;
                }
                let node_a = &node_list[i];
                let node_b = &node_list[j];

                if self.matrix.can_reach(&node_a.fp, &node_b.fp) {
                    let base_url_b = format!("http://127.0.0.1:{}", node_b.port);
                    node_a
                        .router
                        .register_peer_endpoint(&node_b.fp, &base_url_b)
                        .await;

                    let link_cfg = self.matrix.get_link_config(&node_a.fp, &node_b.fp);
                    let rtt = link_cfg.map(|c| c.latency_ms.max(5)).unwrap_or(10);

                    let mut b_node = MeshNode::new(
                        node_b.fp.clone(),
                        node_b.alias.clone(),
                        node_b.router.is_relay_enabled(),
                    );
                    b_node.cert_pem = Some(node_b.identity.cert_pem.clone());
                    b_node.last_seen_ms = now_ms;

                    {
                        let mut graph = node_a.router.graph().write().await;
                        graph.insert_node(b_node.clone());
                        graph.add_bidirectional_link(&node_a.fp, &node_b.fp, rtt, now_ms);
                    }

                    let route = {
                        let graph = node_a.router.graph().read().await;
                        graph.resolve_route(&node_b.fp)
                    };

                    node_a
                        .registry
                        .handle_mesh_node(&b_node, route.as_ref(), now_ms)
                        .await;
                    count += 1;
                }
            }
        }

        count
    }

    /// Executes one round of gossip exchange between all directly reachable neighbor pairs.
    pub async fn exchange_gossip_all(&self) -> anyhow::Result<usize> {
        let mut count = 0;
        let fps: Vec<String> = self.nodes.keys().cloned().collect();

        for i in 0..fps.len() {
            for j in 0..fps.len() {
                if i == j {
                    continue;
                }
                let fp_a = &fps[i];
                let fp_b = &fps[j];

                if self.matrix.can_reach(fp_a, fp_b) {
                    let node_a = self.nodes.get(fp_a).unwrap();
                    if node_a.mesh_router.send_gossip(fp_b).await.is_ok() {
                        count += 1;
                    }
                }
            }
        }

        Ok(count)
    }

    /// Executes multiple sequential rounds of gossip exchange to propagate multi-hop routes.
    pub async fn exchange_gossip_rounds(&self, rounds: usize) -> anyhow::Result<()> {
        for _ in 0..rounds {
            self.exchange_gossip_all().await?;
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        self.sync_all_registries().await;
        Ok(())
    }

    /// Synchronizes peer registries for all nodes from their respective topology graphs.
    pub async fn sync_all_registries(&self) {
        for node in self.nodes.values() {
            node.mesh_router.sync_registry().await;
        }
    }

    /// Looks up a node by its canonical fingerprint or case-insensitive alias.
    pub fn get_node(&self, fp_or_alias: &str) -> Option<&SimulatedNode> {
        let canonical = fp_or_alias.to_ascii_uppercase();
        if let Some(node) = self.nodes.get(&canonical) {
            return Some(node);
        }
        self.nodes
            .values()
            .find(|n| n.alias.eq_ignore_ascii_case(fp_or_alias))
    }

    /// Returns the number of simulated nodes in the cluster.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Iterates over all simulated nodes.
    pub fn nodes(&self) -> impl Iterator<Item = &SimulatedNode> {
        self.nodes.values()
    }

    /// Gracefully stops all node servers.
    pub fn shutdown(&mut self) {
        for node in self.nodes.values() {
            node.server_handle.stop();
        }
    }
}

impl Drop for SimulatedMeshCluster {
    fn drop(&mut self) {
        for node in self.nodes.values() {
            node.server_handle.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_matrix_subnet_partition_and_reachability() {
        let matrix = VirtualNetworkMatrix::new();
        matrix.add_node("FP_A", "Node A", vec!["SubnetA".into()]);
        matrix.add_node("FP_B", "Relay B", vec!["SubnetA".into(), "SubnetB".into()]);
        matrix.add_node("FP_C", "Node C", vec!["SubnetB".into()]);

        // A and B share SubnetA -> can reach
        assert!(matrix.can_reach("FP_A", "FP_B"));
        assert!(matrix.can_reach("FP_B", "FP_A"));

        // B and C share SubnetB -> can reach
        assert!(matrix.can_reach("FP_B", "FP_C"));
        assert!(matrix.can_reach("FP_C", "FP_B"));

        // A and C share no subnets -> partition blocked!
        assert!(!matrix.can_reach("FP_A", "FP_C"));
        assert!(!matrix.can_reach("FP_C", "FP_A"));
    }

    #[test]
    fn test_virtual_matrix_link_severing_and_restoration() {
        let matrix = VirtualNetworkMatrix::new();
        matrix.add_node("FP_A", "Node A", vec!["SubnetA".into()]);
        matrix.add_node("FP_B", "Node B", vec!["SubnetA".into()]);

        assert!(matrix.can_reach("FP_A", "FP_B"));

        // Sever link A <-> B
        matrix.sever_link("FP_A", "FP_B");
        assert!(matrix.is_link_severed("FP_A", "FP_B"));
        assert!(!matrix.can_reach("FP_A", "FP_B"));
        assert!(!matrix.can_reach("FP_B", "FP_A"));

        // Restore link
        matrix.restore_link("FP_A", "FP_B");
        assert!(!matrix.is_link_severed("FP_A", "FP_B"));
        assert!(matrix.can_reach("FP_A", "FP_B"));
    }

    #[test]
    fn test_virtual_matrix_fault_injection_and_interceptor() {
        let matrix = VirtualNetworkMatrix::new();
        matrix.add_node("FP_A", "Node A", vec!["SubnetA".into()]);
        matrix.add_node("FP_B", "Node B", vec!["SubnetA".into()]);

        matrix.set_bidirectional_link_config(
            "FP_A",
            "FP_B",
            SimulatedLinkConfig::new()
                .with_latency(15)
                .with_jitter(5)
                .with_drop_rate(0.0),
        );

        let delay = matrix.intercept("FP_A", "FP_B").expect("must reach");
        assert!(delay.as_millis() >= 15 && delay.as_millis() <= 20);

        // Offline node
        matrix.set_node_offline("FP_B", true);
        assert!(matrix.is_node_offline("FP_B"));
        assert!(matrix.intercept("FP_A", "FP_B").is_err());
    }
}
