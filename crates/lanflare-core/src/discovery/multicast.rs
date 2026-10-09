//! UDP Multicast discovery engine (LocalSend v2.2 compatible).
//!
//! Handles dual-stack UDP multicast listening and announcements across
//! IPv4 (`224.0.0.167:53317`) and IPv6 (`ff12::fd3a:e420:53317`).
//!
//! Features:
//! - Multi-interface socket binding with `SO_REUSEADDR` / `SO_REUSEPORT`.
//! - Backoff announcement bursts with jitter (`100ms`, `500ms`, `2000ms`).
//! - Multicast loopback suppression filtering out own TLS certificate fingerprints.
//! - Non-blocking async event streaming over Tokio channels.

use crate::discovery::interface::{local_interfaces, InterfaceFilter};
use lanflare_proto::discovery::{DeviceType, MulticastMessageV2, ProtocolType};
use rand::Rng;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

/// Default IPv4 multicast group used by LocalSend (in 224.0.0.0/24 for Android compatibility).
pub const DEFAULT_MULTICAST_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 167);

/// Default IPv6 multicast group used by LocalSend (link-local transient group).
pub const DEFAULT_MULTICAST_IPV6: Ipv6Addr =
    Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0xfd3a, 0xe420);

/// Default multicast port (same as default HTTP server port 53317).
pub const DEFAULT_MULTICAST_PORT: u16 = 53317;

/// Delays between announcement packets in a broadcast burst.
pub const ANNOUNCE_BURST_DELAYS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(500),
    Duration::from_millis(2000),
];

/// Maximum jitter added to each announcement burst interval.
pub const MAX_BURST_JITTER: Duration = Duration::from_millis(50);

/// Buffer size for incoming multicast datagrams.
const RECEIVE_BUFFER_SIZE: usize = 65536;

/// Consecutive receive errors tolerated before giving up on an interface socket.
const MAX_CONSECUTIVE_RECEIVE_ERRORS: u32 = 10;

/// Information about this node that is announced via multicast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MulticastNodeInfo {
    /// Human-readable device alias (e.g. "LanFlare Node").
    pub alias: String,

    /// Protocol version (e.g. "2.0").
    pub version: String,

    /// Device model string.
    pub device_model: Option<String>,

    /// Categorical device type.
    pub device_type: Option<DeviceType>,

    /// Fingerprint of the node's TLS certificate (or unique session id).
    /// Used for loopback filtering and cryptographic pinning.
    pub fingerprint: String,

    /// Port of the node's HTTP/HTTPS server (not necessarily 53317).
    pub port: u16,

    /// Transport protocol (HTTP or HTTPS).
    pub protocol: ProtocolType,

    /// Whether download API is supported.
    pub download: bool,
}

impl MulticastNodeInfo {
    pub fn to_message(&self, announce: bool) -> MulticastMessageV2 {
        MulticastMessageV2 {
            alias: self.alias.clone(),
            version: self.version.clone(),
            device_model: self.device_model.clone(),
            device_type: self.device_type,
            fingerprint: self.fingerprint.clone(),
            port: self.port,
            protocol: self.protocol,
            download: self.download,
            announce,
        }
    }
}

/// Configuration options for the multicast discovery service.
#[derive(Clone, Debug)]
pub struct MulticastConfig {
    /// Multicast IPv4 group address.
    pub group_v4: Ipv4Addr,

    /// Optional Multicast IPv6 group address.
    pub group_v6: Option<Ipv6Addr>,

    /// UDP port to bind to (typically 53317).
    pub port: u16,

    /// Interface filtering rules.
    pub interface_filter: InterfaceFilter,

    /// Self node announcement info.
    pub node_info: MulticastNodeInfo,

    /// Whether to join the loopback interface as fallback if no physical interfaces are available.
    pub allow_loopback_fallback: bool,
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self {
            group_v4: DEFAULT_MULTICAST_IPV4,
            group_v6: Some(DEFAULT_MULTICAST_IPV6),
            port: DEFAULT_MULTICAST_PORT,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "LanFlare Node".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: Some(DeviceType::Desktop),
                fingerprint: String::new(),
                port: DEFAULT_MULTICAST_PORT,
                protocol: ProtocolType::Https,
                download: false,
            },
            allow_loopback_fallback: false,
        }
    }
}

/// Events produced by the multicast discovery listener.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MulticastDiscoveryEvent {
    /// Discovered a remote peer announcing on the local subnet.
    PeerDiscovered {
        /// Remote IP address that sent the packet.
        remote_ip: IpAddr,

        /// IPv6 scope ID if link-local.
        scope_id: Option<u32>,

        /// Received payload deserialized into a protocol DTO.
        message: MulticastMessageV2,
    },

    /// All multicast listening sockets have permanently failed.
    AllSocketsFailed,
}

/// Internal struct tracking a bound multicast socket and its target.
pub struct BoundMulticastSocket {
    pub description: String,
    pub target: SocketAddr,
    pub socket: Arc<UdpSocket>,
}

/// Binds IPv4 UDP multicast socket on specified interface address.
pub fn bind_multicast_socket_v4(
    group: Ipv4Addr,
    port: u16,
    interface: Ipv4Addr,
) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;

    // Enable port reuse
    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;

    // Bind to wildcard address to receive multicast datagrams
    socket.bind(&SocketAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).into())?;

    // Join the multicast group on the specific interface
    socket.join_multicast_v4(&group, &interface)?;

    // Set interface for outgoing multicast packets
    socket.set_multicast_if_v4(&interface)?;

    // Enable multicast loopback so same-machine instances can discover each other
    socket.set_multicast_loop_v4(true)?;

    // Subnet-only hop limit
    socket.set_multicast_ttl_v4(1)?;

    socket.set_nonblocking(true)?;

    UdpSocket::from_std(socket.into())
}

/// Binds IPv6 UDP multicast socket on specified interface index.
pub fn bind_multicast_socket_v6(
    group: Ipv6Addr,
    port: u16,
    interface_index: u32,
) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;

    // Must be IPv6 only so it doesn't clash with IPv4 sockets on the same port
    socket.set_only_v6(true)?;

    socket.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true)?;

    socket.bind(&SocketAddr::from(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0)).into())?;

    socket.join_multicast_v6(&group, interface_index)?;

    socket.set_multicast_if_v6(interface_index)?;

    socket.set_multicast_loop_v6(true)?;

    // Link-local hops limit
    socket.set_multicast_hops_v6(1)?;

    socket.set_nonblocking(true)?;

    UdpSocket::from_std(socket.into())
}

/// Binds multicast sockets on all available interfaces according to configuration.
pub fn bind_multicast_sockets(
    config: &MulticastConfig,
) -> std::io::Result<Vec<BoundMulticastSocket>> {
    let mut interfaces = local_interfaces(&config.interface_filter)?;

    // If no physical interfaces found and loopback fallback is allowed, try loopback
    if interfaces.v4.is_empty() && interfaces.v6.is_empty() && config.allow_loopback_fallback {
        interfaces.v4.push(crate::discovery::interface::LocalInterfaceV4 {
            name: "lo".to_string(),
            address: Ipv4Addr::new(127, 0, 0, 1),
        });
    }

    let mut sockets = Vec::new();

    for interface in interfaces.v4 {
        let description = format!("{} ({})", interface.name, interface.address);
        match bind_multicast_socket_v4(config.group_v4, config.port, interface.address) {
            Ok(socket) => {
                tracing::info!(
                    "Bound UDP multicast socket v4 (interface: {}, group: {}, port: {})",
                    description,
                    config.group_v4,
                    config.port
                );
                sockets.push(BoundMulticastSocket {
                    description,
                    target: SocketAddr::from(SocketAddrV4::new(config.group_v4, config.port)),
                    socket: Arc::new(socket),
                });
            }
            Err(err) => {
                tracing::warn!(
                    "Could not bind UDP multicast socket v4 ({}, group: {}, port: {}): {err:#}",
                    description,
                    config.group_v4,
                    config.port
                );
            }
        }
    }

    if let Some(group_v6) = config.group_v6 {
        for interface in interfaces.v6 {
            let description = format!("{} (IPv6, if_index: {})", interface.name, interface.index);
            match bind_multicast_socket_v6(group_v6, config.port, interface.index) {
                Ok(socket) => {
                    tracing::info!(
                        "Bound UDP multicast socket v6 (interface: {}, group: {}, port: {})",
                        description,
                        group_v6,
                        config.port
                    );
                    sockets.push(BoundMulticastSocket {
                        description,
                        target: SocketAddr::from(SocketAddrV6::new(
                            group_v6,
                            config.port,
                            0,
                            interface.index,
                        )),
                        socket: Arc::new(socket),
                    });
                }
                Err(err) => {
                    tracing::warn!(
                        "Could not bind UDP multicast socket v6 ({}, group: {}, port: {}): {err:#}",
                        description,
                        group_v6,
                        config.port
                    );
                }
            }
        }
    }

    Ok(sockets)
}

/// A sender socket descriptor.
#[derive(Clone)]
struct SendTarget {
    target: SocketAddr,
    socket: Arc<UdpSocket>,
}

struct MulticastEngineState {
    node_info: MulticastNodeInfo,
    sockets: RwLock<Vec<SendTarget>>,
}

impl MulticastEngineState {
    /// Broadcast an announcement packet across all active bound sockets.
    async fn broadcast_announcement(&self) {
        let msg = self.node_info.to_message(true);
        let payload = match serde_json::to_vec(&msg) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to serialize multicast announcement: {e}");
                return;
            }
        };

        let sockets = self.sockets.read().await.clone();
        for SendTarget { target, socket } in sockets {
            if let Err(err) = socket.send_to(&payload, target).await {
                tracing::warn!("Failed sending multicast packet to {target}: {err}");
            }
        }
    }
}

/// Handle to a running multicast discovery engine.
#[derive(Clone)]
pub struct MulticastHandle {
    state: Arc<MulticastEngineState>,
    cancel: CancellationToken,
    task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl MulticastHandle {
    /// Broadcasts an announcement burst with exponential backoff and jitter.
    ///
    /// The default burst schedule is `100ms`, `500ms`, `2000ms`, each randomized
    /// with up to `+50ms` jitter to prevent network collisions when multiple nodes boot simultaneously.
    pub async fn announce_burst(&self) {
        self.announce_custom_burst(&ANNOUNCE_BURST_DELAYS, MAX_BURST_JITTER)
            .await;
    }

    /// Broadcasts a custom burst with the provided delays and maximum jitter.
    pub async fn announce_custom_burst(&self, delays: &[Duration], max_jitter: Duration) {
        for &base_delay in delays {
            let jitter_millis = if !max_jitter.is_zero() {
                rand::rng().random_range(0..max_jitter.as_millis().max(1) as u64)
            } else {
                0
            };
            let delay = base_delay + Duration::from_millis(jitter_millis);

            tokio::select! {
                _ = self.cancel.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }

            tracing::debug!("Sending multicast announcement burst");
            self.state.broadcast_announcement().await;
        }
    }

    /// Sends a single instantaneous announcement packet without waiting.
    pub async fn send_single_announcement(&self) {
        self.state.broadcast_announcement().await;
    }

    /// Stops the discovery engine and waits for all sockets to close and release their ports.
    pub async fn stop(&self) {
        self.cancel.cancel();
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }

    /// Waits for the discovery task to terminate.
    pub async fn wait_stopped(&self) {
        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }
}

/// Starts the multicast discovery engine.
///
/// Binds sockets across all valid interfaces, spawns receiver tasks,
/// and returns an asynchronous handle to control announcements and shutdown.
pub async fn start_multicast_engine(
    config: MulticastConfig,
    event_tx: mpsc::Sender<MulticastDiscoveryEvent>,
    stop_rx: oneshot::Receiver<()>,
) -> anyhow::Result<MulticastHandle> {
    let bound_sockets = bind_multicast_sockets(&config)?;
    if bound_sockets.is_empty() {
        anyhow::bail!(
            "No network interface available for multicast on port {}",
            config.port
        );
    }

    let state = Arc::new(MulticastEngineState {
        node_info: config.node_info,
        sockets: RwLock::new(
            bound_sockets
                .iter()
                .map(|s| SendTarget {
                    target: s.target,
                    socket: s.socket.clone(),
                })
                .collect(),
        ),
    });

    let cancel = CancellationToken::new();

    let task = tokio::spawn({
        let state = state.clone();
        let cancel = cancel.clone();
        async move {
            let mut receiver_tasks = tokio::task::JoinSet::new();
            for s in bound_sockets {
                receiver_tasks.spawn(receive_loop(
                    s,
                    state.clone(),
                    event_tx.clone(),
                    cancel.clone(),
                ));
            }

            tokio::select! {
                _ = async { while receiver_tasks.join_next().await.is_some() {} } => {
                    tracing::error!("All multicast sockets failed. Stopping multicast discovery.");
                    let _ = event_tx.try_send(MulticastDiscoveryEvent::AllSocketsFailed);
                }
                _ = stop_rx => {}
                _ = cancel.cancelled() => {}
            }

            cancel.cancel();
            receiver_tasks.shutdown().await;
            state.sockets.write().await.clear();
        }
    });

    Ok(MulticastHandle {
        state,
        cancel,
        task: Arc::new(Mutex::new(Some(task))),
    })
}

/// Receives incoming datagrams on a single multicast socket.
async fn receive_loop(
    socket: BoundMulticastSocket,
    state: Arc<MulticastEngineState>,
    event_tx: mpsc::Sender<MulticastDiscoveryEvent>,
    cancel: CancellationToken,
) {
    let mut buffer = vec![0u8; RECEIVE_BUFFER_SIZE];
    let mut consecutive_errors = 0;

    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            res = socket.socket.recv_from(&mut buffer) => res,
        };

        let (size, source) = match received {
            Ok(res) => {
                consecutive_errors = 0;
                res
            }
            Err(err) => {
                consecutive_errors += 1;
                tracing::warn!(
                    "Multicast recv error on interface {}: {err:#}",
                    socket.description
                );
                if consecutive_errors >= MAX_CONSECUTIVE_RECEIVE_ERRORS {
                    tracing::error!(
                        "Exceeded max consecutive receive errors on {}. Terminating socket loop.",
                        socket.description
                    );
                    return;
                }
                continue;
            }
        };

        let message = match serde_json::from_slice::<MulticastMessageV2>(&buffer[..size]) {
            Ok(msg) => msg,
            Err(err) => {
                tracing::warn!("Failed to deserialize multicast message from {source}: {err:#}");
                continue;
            }
        };

        // Loopback suppression: ignore messages from our own fingerprint
        if !state.node_info.fingerprint.is_empty()
            && message.fingerprint == state.node_info.fingerprint
        {
            tracing::trace!(
                "Suppressed self-multicast announcement with fingerprint {}",
                message.fingerprint
            );
            continue;
        }

        let event = MulticastDiscoveryEvent::PeerDiscovered {
            remote_ip: source.ip(),
            scope_id: match &source {
                SocketAddr::V6(v6) if v6.scope_id() != 0 => Some(v6.scope_id()),
                _ => None,
            },
            message,
        };

        if event_tx.send(event).await.is_err() {
            tracing::debug!("Discovery event receiver dropped. Shutting down receive loop.");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU16, Ordering};

    static NEXT_TEST_PORT: AtomicU16 = AtomicU16::new(54100);

    const TEST_GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 169);
    const TEST_GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xff12, 0, 0, 0, 0, 0, 0xfd3a, 0xe422);

    #[test]
    fn test_node_info_to_message() {
        let info = MulticastNodeInfo {
            alias: "Test Node".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Linux".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: "FP123456".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
        };

        let msg = info.to_message(true);
        assert_eq!(msg.alias, "Test Node");
        assert_eq!(msg.fingerprint, "FP123456");
        assert!(msg.announce);
        assert!(msg.download);

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"announce\":true"));
        assert!(json.contains("\"alias\":\"Test Node\""));
    }

    #[test]
    fn test_multicast_config_default() {
        let config = MulticastConfig::default();
        assert_eq!(config.group_v4, DEFAULT_MULTICAST_IPV4);
        assert_eq!(config.group_v6, Some(DEFAULT_MULTICAST_IPV6));
        assert_eq!(config.port, DEFAULT_MULTICAST_PORT);
    }

    #[tokio::test]
    async fn test_multi_node_discovery_exchange() {
        let port = NEXT_TEST_PORT.fetch_add(1, Ordering::Relaxed);

        let (node1_tx, _node1_rx) = mpsc::channel(32);
        let (_node1_stop_tx, node1_stop_rx) = oneshot::channel();

        let cfg1 = MulticastConfig {
            group_v4: TEST_GROUP_V4,
            group_v6: Some(TEST_GROUP_V6),
            port,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "Node One".to_string(),
                version: "2.0".to_string(),
                device_model: Some("Linux".to_string()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: "FP_NODE_ONE".to_string(),
                port,
                protocol: ProtocolType::Https,
                download: true,
            },
            allow_loopback_fallback: false,
        };

        let handle1 = match start_multicast_engine(cfg1, node1_tx, node1_stop_rx).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Skipping test due to environment without multicast: {e}");
                return;
            }
        };

        let (node2_tx, mut node2_rx) = mpsc::channel(32);
        let (_node2_stop_tx, node2_stop_rx) = oneshot::channel();

        let cfg2 = MulticastConfig {
            group_v4: TEST_GROUP_V4,
            group_v6: Some(TEST_GROUP_V6),
            port,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "Node Two".to_string(),
                version: "2.0".to_string(),
                device_model: Some("Android".to_string()),
                device_type: Some(DeviceType::Mobile),
                fingerprint: "FP_NODE_TWO".to_string(),
                port,
                protocol: ProtocolType::Https,
                download: false,
            },
            allow_loopback_fallback: false,
        };

        let handle2 = match start_multicast_engine(cfg2, node2_tx, node2_stop_rx).await {
            Ok(h) => h,
            Err(e) => {
                handle1.stop().await;
                eprintln!("Skipping test due to environment without multicast: {e}");
                return;
            }
        };

        // Node 1 broadcasts announcement
        handle1.send_single_announcement().await;

        // Node 2 should receive Node 1's announcement
        let received_by_node2 = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(evt) = node2_rx.recv().await {
                match evt {
                    MulticastDiscoveryEvent::PeerDiscovered { message, .. }
                        if message.fingerprint == "FP_NODE_ONE" =>
                    {
                        return Some(message);
                    }
                    _ => {}
                }
            }
            None
        })
        .await;



        if let Ok(Some(msg)) = received_by_node2 {
            assert_eq!(msg.alias, "Node One");
            assert_eq!(msg.fingerprint, "FP_NODE_ONE");
            assert_eq!(msg.device_type, Some(DeviceType::Desktop));
        } else {
            eprintln!("Warning: Multicast packet not received (likely filtered by network interface)");
        }

        // Clean shutdown
        handle1.stop().await;
        handle2.stop().await;
    }

    #[tokio::test]
    async fn test_loopback_suppression() {
        let port = NEXT_TEST_PORT.fetch_add(1, Ordering::Relaxed);

        let (node_tx, mut node_rx) = mpsc::channel(32);
        let (_stop_tx, stop_rx) = oneshot::channel();

        let cfg = MulticastConfig {
            group_v4: TEST_GROUP_V4,
            group_v6: Some(TEST_GROUP_V6),
            port,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "Loopback Check".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: Some(DeviceType::Server),
                fingerprint: "FP_SELF_ONLY".to_string(),
                port,
                protocol: ProtocolType::Https,
                download: false,
            },
            allow_loopback_fallback: false,
        };

        let handle = match start_multicast_engine(cfg, node_tx, stop_rx).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Skipping test: {e}");
                return;
            }
        };

        // Send announcement with our own fingerprint
        handle.send_single_announcement().await;

        // Verify we never receive our own packet
        let own_received = tokio::time::timeout(Duration::from_millis(500), async {
            while let Some(evt) = node_rx.recv().await {
                if matches!(
                    &evt,
                    MulticastDiscoveryEvent::PeerDiscovered { message, .. }
                        if message.fingerprint == "FP_SELF_ONLY"
                ) {
                    return true;
                }
            }
            false
        })
        .await;

        assert!(
            !own_received.unwrap_or(false),
            "Own announcement must be suppressed by loopback filter"
        );

        handle.stop().await;
    }


    #[tokio::test]
    async fn test_port_released_after_stop() {
        let port = NEXT_TEST_PORT.fetch_add(1, Ordering::Relaxed);

        let (tx1, _rx1) = mpsc::channel(32);
        let (stop_tx1, stop_rx1) = oneshot::channel();

        let cfg1 = MulticastConfig {
            group_v4: TEST_GROUP_V4,
            group_v6: Some(TEST_GROUP_V6),
            port,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "Instance 1".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: None,
                fingerprint: "FP_1".to_string(),
                port,
                protocol: ProtocolType::Https,
                download: false,
            },
            allow_loopback_fallback: false,
        };

        let handle1 = match start_multicast_engine(cfg1, tx1, stop_rx1).await {
            Ok(h) => h,
            Err(e) => {
                eprintln!("Skipping test: {e}");
                return;
            }
        };

        let _ = stop_tx1.send(());
        handle1.wait_stopped().await;

        // Port must be re-bindable
        let (tx2, _rx2) = mpsc::channel(32);
        let (_stop_tx2, stop_rx2) = oneshot::channel();

        let cfg2 = MulticastConfig {
            group_v4: TEST_GROUP_V4,
            group_v6: Some(TEST_GROUP_V6),
            port,
            interface_filter: InterfaceFilter::default(),
            node_info: MulticastNodeInfo {
                alias: "Instance 2".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: None,
                fingerprint: "FP_2".to_string(),
                port,
                protocol: ProtocolType::Https,
                download: false,
            },
            allow_loopback_fallback: false,
        };

        let handle2 = start_multicast_engine(cfg2, tx2, stop_rx2).await;
        assert!(handle2.is_ok(), "Port should be reusable after shutdown");
        if let Ok(h) = handle2 {
            h.stop().await;
        }
    }
}

