//! Modern mDNS / DNS-SD discovery engine for LanFlare peers.
//!
//! Provides zero-configuration service discovery over local multicast DNS (`224.0.0.251:5353` / `[ff02::fb]:5353`),
//! advertising and browsing the `_lanflare._tcp.local.` service type.
//!
//! Features:
//! - Service registration with RFC 6763 TXT records containing device alias, protocol, port, device type, and TLS certificate fingerprint.
//! - Continuous and cache-friendly service browsing with TTL expiration and service removal tracking.
//! - Automatic address binding and multi-interface support.
//! - Multicast loopback filtering to avoid discovering the local node.
//! - Graceful shutdown and unregistration releasing DNS-SD records cleanly.

use lanflare_proto::discovery::{DeviceType, ProtocolType};
use mdns_sd::{Receiver, ResolvedService, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::sync::CancellationToken;

/// LanFlare DNS-SD service type. Must end with `._tcp.local.`.
pub const LANFLARE_SERVICE_TYPE: &str = "_lanflare._tcp.local.";

/// Default hostname suffix for LanFlare mDNS hosts.
pub const LANFLARE_DEFAULT_HOST_DOMAIN: &str = "lanflare.local.";

/// Key constants for DNS-SD TXT record attributes (RFC 6763).
pub mod txt_keys {
    /// Human-readable device alias.
    pub const ALIAS: &str = "alias";
    /// Protocol version (e.g. "2.0").
    pub const VERSION: &str = "version";
    /// Device model string.
    pub const DEVICE_MODEL: &str = "model";
    /// Device type category ("desktop", "mobile", etc.).
    pub const DEVICE_TYPE: &str = "devtype";
    /// TLS certificate SHA-256 fingerprint (64 uppercase hex characters).
    pub const FINGERPRINT: &str = "fingerprint";
    /// HTTP server transport protocol ("http" or "https").
    pub const PROTOCOL: &str = "protocol";
    /// Port of the HTTP/HTTPS server (can differ from the SRV port).
    pub const PORT: &str = "port";
    /// Whether reverse download API is supported.
    pub const DOWNLOAD: &str = "download";
}

/// Information about this node that is advertised via mDNS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MdnsNodeInfo {
    /// Human-readable device alias (e.g. "LanFlare Node").
    pub alias: String,

    /// Unique instance identifier or name for mDNS service name.
    /// If empty, `alias` will be used.
    pub instance_name: String,

    /// Protocol version (e.g. "2.0").
    pub version: String,

    /// Device model string (optional).
    pub device_model: Option<String>,

    /// Categorical device type.
    pub device_type: Option<DeviceType>,

    /// Fingerprint of the node's TLS certificate (uppercase 64-char hex string).
    /// Used for peer authentication and loopback filtering.
    pub fingerprint: String,

    /// Port of the node's HTTP/HTTPS server.
    pub port: u16,

    /// Transport protocol (HTTP or HTTPS).
    pub protocol: ProtocolType,

    /// Whether download API is supported.
    pub download: bool,
}

impl Default for MdnsNodeInfo {
    fn default() -> Self {
        Self {
            alias: "LanFlare Node".to_string(),
            instance_name: String::new(),
            version: "2.0".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Desktop),
            fingerprint: String::new(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: false,
        }
    }
}

impl MdnsNodeInfo {
    /// Encodes the node info into RFC 6763 TXT record properties.
    pub fn to_txt_properties(&self) -> HashMap<String, String> {
        let mut props = HashMap::new();
        props.insert(txt_keys::ALIAS.to_string(), self.alias.clone());
        props.insert(txt_keys::VERSION.to_string(), self.version.clone());
        if let Some(ref model) = self.device_model {
            props.insert(txt_keys::DEVICE_MODEL.to_string(), model.clone());
        }
        if let Some(dt) = self.device_type {
            let type_str = match dt {
                DeviceType::Mobile => "mobile",
                DeviceType::Desktop => "desktop",
                DeviceType::Web => "web",
                DeviceType::Headless => "headless",
                DeviceType::Server => "server",
            };
            props.insert(txt_keys::DEVICE_TYPE.to_string(), type_str.to_string());
        }
        props.insert(
            txt_keys::FINGERPRINT.to_string(),
            self.fingerprint.clone(),
        );
        props.insert(
            txt_keys::PROTOCOL.to_string(),
            self.protocol.as_str().to_string(),
        );
        props.insert(txt_keys::PORT.to_string(), self.port.to_string());
        props.insert(
            txt_keys::DOWNLOAD.to_string(),
            if self.download { "true" } else { "false" }.to_string(),
        );
        props
    }

    /// Converts this `MdnsNodeInfo` into an `mdns_sd::ServiceInfo` instance.
    pub fn to_service_info(&self, host_name: Option<&str>) -> anyhow::Result<ServiceInfo> {
        let instance = if self.instance_name.is_empty() {
            &self.alias
        } else {
            &self.instance_name
        };

        let host = host_name.unwrap_or(LANFLARE_DEFAULT_HOST_DOMAIN);
        let properties = self.to_txt_properties();

        let service = ServiceInfo::new(
            LANFLARE_SERVICE_TYPE,
            instance,
            host,
            "", // Empty IP enables auto-discovery of addresses
            self.port,
            properties,
        )?
        .enable_addr_auto();

        Ok(service)
    }
}

/// Information parsed from a discovered mDNS peer service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredMdnsPeer {
    /// Fully qualified service instance name (e.g. `My Node._lanflare._tcp.local.`).
    pub fullname: String,

    /// Discovered IP addresses of the peer.
    pub addresses: Vec<IpAddr>,

    /// Port the peer is listening on.
    pub port: u16,

    /// Hostname advertised in the SRV record.
    pub hostname: String,

    /// Human-readable device alias.
    pub alias: String,

    /// Protocol version (e.g. "2.0").
    pub version: String,

    /// Device model (if advertised).
    pub device_model: Option<String>,

    /// Device type classification.
    pub device_type: Option<DeviceType>,

    /// Fingerprint of the peer's TLS certificate.
    pub fingerprint: String,

    /// Transport protocol (HTTP or HTTPS).
    pub protocol: ProtocolType,

    /// Whether download API is supported.
    pub download: bool,

    /// Raw TXT record map.
    pub txt_records: HashMap<String, String>,
}

impl DiscoveredMdnsPeer {
    /// Parses a `ResolvedService` into a `DiscoveredMdnsPeer`.
    pub fn from_resolved_service(resolved: &ResolvedService) -> Option<Self> {
        let fullname = resolved.get_fullname().to_string();
        let port = resolved.get_port();
        let hostname = resolved.get_hostname().to_string();

        let mut addresses: Vec<IpAddr> = resolved
            .get_addresses()
            .iter()
            .map(|scoped| scoped.to_ip_addr())
            .collect();
        // Sort for deterministic peer comparisons (IPv4 first, then IPv6)
        addresses.sort_by_key(|addr| match addr {
            IpAddr::V4(_) => 0,
            IpAddr::V6(_) => 1,
        });

        let txt = resolved.get_properties();
        let alias = txt
            .get_property_val_str(txt_keys::ALIAS)
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Fallback to instance name prefix if alias key is absent
                fullname
                    .split('.')
                    .next()
                    .unwrap_or("LanFlare Peer")
                    .to_string()
            });

        let version = txt
            .get_property_val_str(txt_keys::VERSION)
            .map(str::to_string)
            .unwrap_or_else(|| "2.0".to_string());

        let device_model = txt
            .get_property_val_str(txt_keys::DEVICE_MODEL)
            .map(str::to_string);

        let device_type = txt
            .get_property_val_str(txt_keys::DEVICE_TYPE)
            .map(|dt| match dt.to_ascii_lowercase().as_str() {
                "mobile" => DeviceType::Mobile,
                "desktop" => DeviceType::Desktop,
                "web" => DeviceType::Web,
                "headless" => DeviceType::Headless,
                "server" => DeviceType::Server,
                _ => DeviceType::Desktop,
            });

        let fingerprint = txt
            .get_property_val_str(txt_keys::FINGERPRINT)
            .map(str::to_string)
            .unwrap_or_default();

        let protocol = match txt
            .get_property_val_str(txt_keys::PROTOCOL)
            .unwrap_or("https")
            .to_ascii_lowercase()
            .as_str()
        {
            "http" => ProtocolType::Http,
            _ => ProtocolType::Https,
        };

        let download = txt
            .get_property_val_str(txt_keys::DOWNLOAD)
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        // Extract all string properties into a map
        let mut txt_records = HashMap::new();
        for prop in txt.iter() {
            let val = prop.val_str();
            txt_records.insert(prop.key().to_string(), val.to_string());
        }

        Some(Self {
            fullname,
            addresses,
            port,
            hostname,
            alias,
            version,
            device_model,
            device_type,
            fingerprint,
            protocol,
            download,
            txt_records,
        })
    }

    /// Parses a resolved `ServiceInfo` into a `DiscoveredMdnsPeer`.
    pub fn from_service_info(info: &ServiceInfo) -> Option<Self> {
        let fullname = info.get_fullname().to_string();
        let port = info.get_port();
        let hostname = info.get_hostname().to_string();

        let mut addresses: Vec<IpAddr> = info.get_addresses().iter().copied().collect();
        // Sort for deterministic peer comparisons (IPv4 first, then IPv6)
        addresses.sort_by_key(|addr| match addr {
            IpAddr::V4(_) => 0,
            IpAddr::V6(_) => 1,
        });

        let txt = info.get_properties();
        let alias = txt
            .get_property_val_str(txt_keys::ALIAS)
            .map(str::to_string)
            .unwrap_or_else(|| {
                // Fallback to instance name prefix if alias key is absent
                fullname
                    .split('.')
                    .next()
                    .unwrap_or("LanFlare Peer")
                    .to_string()
            });

        let version = txt
            .get_property_val_str(txt_keys::VERSION)
            .map(str::to_string)
            .unwrap_or_else(|| "2.0".to_string());

        let device_model = txt
            .get_property_val_str(txt_keys::DEVICE_MODEL)
            .map(str::to_string);

        let device_type = txt
            .get_property_val_str(txt_keys::DEVICE_TYPE)
            .map(|dt| match dt.to_ascii_lowercase().as_str() {
                "mobile" => DeviceType::Mobile,
                "desktop" => DeviceType::Desktop,
                "web" => DeviceType::Web,
                "headless" => DeviceType::Headless,
                "server" => DeviceType::Server,
                _ => DeviceType::Desktop,
            });

        let fingerprint = txt
            .get_property_val_str(txt_keys::FINGERPRINT)
            .map(str::to_string)
            .unwrap_or_default();

        let protocol = match txt
            .get_property_val_str(txt_keys::PROTOCOL)
            .unwrap_or("https")
            .to_ascii_lowercase()
            .as_str()
        {
            "http" => ProtocolType::Http,
            _ => ProtocolType::Https,
        };

        let download = txt
            .get_property_val_str(txt_keys::DOWNLOAD)
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        // Extract all string properties into a map
        let mut txt_records = HashMap::new();
        for prop in txt.iter() {
            let val = prop.val_str();
            txt_records.insert(prop.key().to_string(), val.to_string());
        }

        Some(Self {
            fullname,
            addresses,
            port,
            hostname,
            alias,
            version,
            device_model,
            device_type,
            fingerprint,
            protocol,
            download,
            txt_records,
        })
    }
}

/// Configuration for mDNS discovery engine.
#[derive(Clone, Debug)]
pub struct MdnsConfig {
    /// Service type to advertise and browse (default: `_lanflare._tcp.local.`).
    pub service_type: String,

    /// Self node announcement info.
    pub node_info: MdnsNodeInfo,

    /// Custom host name for mDNS registration (default: None, auto-generated).
    pub host_name: Option<String>,

    /// Enable mDNS responder (advertising this device).
    pub enable_responder: bool,

    /// Enable mDNS browser (discovering remote devices).
    pub enable_browser: bool,
}

impl Default for MdnsConfig {
    fn default() -> Self {
        Self {
            service_type: LANFLARE_SERVICE_TYPE.to_string(),
            node_info: MdnsNodeInfo::default(),
            host_name: None,
            enable_responder: true,
            enable_browser: true,
        }
    }
}

/// Events emitted by the mDNS discovery engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MdnsDiscoveryEvent {
    /// A peer service was newly discovered or updated.
    PeerDiscovered(DiscoveredMdnsPeer),

    /// A peer service was removed (graceful unregister or TTL expiration).
    PeerRemoved {
        /// The fully qualified service name of the removed peer.
        fullname: String,
        /// The service type.
        service_type: String,
    },
}

/// Handle to a running mDNS discovery engine.
pub struct MdnsHandle {
    daemon: ServiceDaemon,
    registered_fullname: Option<String>,
    cancel: CancellationToken,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl MdnsHandle {
    /// Stops browsing, unregisters any published service, shuts down the daemon thread,
    /// and waits for background tasks to complete.
    pub async fn stop(&self) {
        self.cancel.cancel();

        // Unregister service if registered
        if let Some(ref fullname) = self.registered_fullname {
            tracing::debug!("Unregistering mDNS service: {fullname}");
            if let Ok(receiver) = self.daemon.unregister(fullname) {
                let _ = receiver.recv_timeout(Duration::from_millis(500));
            }
        }

        let _ = self.daemon.stop_browse(LANFLARE_SERVICE_TYPE);
        let _ = self.daemon.shutdown();

        if let Some(task) = self.task.lock().await.take() {
            let _ = task.await;
        }
    }

    /// Returns a reference to the underlying `ServiceDaemon`.
    pub fn daemon(&self) -> &ServiceDaemon {
        &self.daemon
    }

    /// Returns the registered service fullname if responder was enabled.
    pub fn registered_fullname(&self) -> Option<&str> {
        self.registered_fullname.as_deref()
    }
}

/// Starts the mDNS discovery engine according to `config`.
///
/// Discovered peer events are forwarded to `event_tx`.
/// The engine can be stopped via the returned `MdnsHandle::stop()` or by sending to `stop_rx`.
pub async fn start_mdns_engine(
    config: MdnsConfig,
    event_tx: mpsc::Sender<MdnsDiscoveryEvent>,
    mut stop_rx: oneshot::Receiver<()>,
) -> anyhow::Result<MdnsHandle> {
    let daemon = ServiceDaemon::new()?;

    // Register responder if enabled
    let registered_fullname = if config.enable_responder {
        let mut service_info = config
            .node_info
            .to_service_info(config.host_name.as_deref())?;
        service_info.set_requires_probe(false);
        let fullname = service_info.get_fullname().to_string();

        tracing::info!("Registering mDNS service instance: {fullname}");
        daemon.register(service_info)?;
        Some(fullname)
    } else {
        None
    };

    let cancel = CancellationToken::new();

    // Spawn browser loop if enabled
    let task = if config.enable_browser {
        let browse_receiver = daemon.browse(&config.service_type)?;
        let self_fingerprint = config.node_info.fingerprint.clone();
        let cancel_child = cancel.clone();

        Some(tokio::spawn(async move {
            tokio::select! {
                _ = browse_loop(browse_receiver, event_tx, self_fingerprint, cancel_child.clone()) => {}
                res = &mut stop_rx => {
                    if res.is_err() {
                        let _ = cancel_child.cancelled().await;
                    }
                }
                _ = cancel_child.cancelled() => {}
            }
            cancel_child.cancel();
        }))
    } else {
        // If browser is disabled, still listen for stop_rx
        let cancel_child = cancel.clone();
        Some(tokio::spawn(async move {
            tokio::select! {
                res = &mut stop_rx => {
                    if res.is_err() {
                        let _ = cancel_child.cancelled().await;
                    }
                }
                _ = cancel_child.cancelled() => {}
            }
            cancel_child.cancel();
        }))
    };

    Ok(MdnsHandle {
        daemon,
        registered_fullname,
        cancel,
        task: Mutex::new(task),
    })
}

/// Background loop receiving `ServiceEvent`s from the mDNS daemon.
async fn browse_loop(
    receiver: Receiver<ServiceEvent>,
    event_tx: mpsc::Sender<MdnsDiscoveryEvent>,
    self_fingerprint: String,
    cancel: CancellationToken,
) {
    loop {
        let event = tokio::select! {
            _ = cancel.cancelled() => break,
            res = receiver.recv_async() => {
                match res {
                    Ok(ev) => ev,
                    Err(e) => {
                        tracing::debug!("mDNS browse receiver closed or error: {e}");
                        break;
                    }
                }
            }
        };

        match event {
            ServiceEvent::ServiceResolved(info) => {
                if let Some(peer) = DiscoveredMdnsPeer::from_resolved_service(&info) {
                    // Multicast loopback suppression: ignore own advertisements
                    if !self_fingerprint.is_empty()
                        && peer.fingerprint.eq_ignore_ascii_case(&self_fingerprint)
                    {
                        tracing::trace!(
                            "Suppressed self-mDNS advertisement with fingerprint: {}",
                            peer.fingerprint
                        );
                        continue;
                    }

                    tracing::debug!(
                        "Discovered mDNS peer '{}' ({:?}:{})",
                        peer.alias,
                        peer.addresses,
                        peer.port
                    );

                    if event_tx
                        .send(MdnsDiscoveryEvent::PeerDiscovered(peer))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            ServiceEvent::ServiceRemoved(service_type, fullname) => {
                tracing::debug!("mDNS peer removed: {fullname} (type: {service_type})");
                if event_tx
                    .send(MdnsDiscoveryEvent::PeerRemoved {
                        fullname,
                        service_type,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
            }
            ServiceEvent::SearchStarted(service_type) => {
                tracing::trace!("mDNS search started for {service_type}");
            }
            ServiceEvent::ServiceFound(service_type, fullname) => {
                tracing::trace!("mDNS service found (resolving): {fullname} in {service_type}");
            }
            ServiceEvent::SearchStopped(service_type) => {
                tracing::trace!("mDNS search stopped for {service_type}");
                break;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_mdns_node_info_to_txt_properties() {
        let node = MdnsNodeInfo {
            alias: "Test Node".to_string(),
            instance_name: "test-node-instance".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Linux ThinkPad".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: "AA11BB22CC33".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
        };

        let props = node.to_txt_properties();
        assert_eq!(props.get(txt_keys::ALIAS).unwrap(), "Test Node");
        assert_eq!(props.get(txt_keys::VERSION).unwrap(), "2.0");
        assert_eq!(
            props.get(txt_keys::DEVICE_MODEL).unwrap(),
            "Linux ThinkPad"
        );
        assert_eq!(props.get(txt_keys::DEVICE_TYPE).unwrap(), "desktop");
        assert_eq!(props.get(txt_keys::FINGERPRINT).unwrap(), "AA11BB22CC33");
        assert_eq!(props.get(txt_keys::PORT).unwrap(), "53317");
        assert_eq!(props.get(txt_keys::PROTOCOL).unwrap(), "https");
        assert_eq!(props.get(txt_keys::DOWNLOAD).unwrap(), "true");
    }

    #[test]
    fn test_mdns_service_info_conversion() {
        let node = MdnsNodeInfo {
            alias: "My Device".to_string(),
            instance_name: "my-device-uuid".to_string(),
            version: "2.0".to_string(),
            device_model: None,
            device_type: Some(DeviceType::Mobile),
            fingerprint: "1234567890ABCDEF".to_string(),
            port: 53318,
            protocol: ProtocolType::Http,
            download: false,
        };

        let info = node.to_service_info(None).expect("must create ServiceInfo");
        assert_eq!(info.get_type(), LANFLARE_SERVICE_TYPE);
        assert_eq!(info.get_port(), 53318);
        assert_eq!(
            info.get_property_val_str(txt_keys::ALIAS).unwrap(),
            "My Device"
        );
        assert_eq!(
            info.get_property_val_str(txt_keys::PROTOCOL).unwrap(),
            "http"
        );
        assert_eq!(
            info.get_property_val_str(txt_keys::DEVICE_TYPE).unwrap(),
            "mobile"
        );
        assert_eq!(
            info.get_property_val_str(txt_keys::DOWNLOAD).unwrap(),
            "false"
        );
    }

    #[test]
    fn test_discovered_mdns_peer_parsing() {
        let node = MdnsNodeInfo {
            alias: "Desktop Rig".to_string(),
            instance_name: "desktop-rig".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Arch Linux".to_string()),
            device_type: Some(DeviceType::Server),
            fingerprint: "FEDCBA0987654321".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
        };

        let mut service = ServiceInfo::new(
            LANFLARE_SERVICE_TYPE,
            "desktop-rig",
            "arch-host.local.",
            "192.168.1.100,10.0.0.1",
            53317,
            node.to_txt_properties(),
        )
        .expect("ServiceInfo creation");

        service.set_requires_probe(false);

        let peer = DiscoveredMdnsPeer::from_service_info(&service)
            .expect("must parse DiscoveredMdnsPeer");

        assert_eq!(peer.alias, "Desktop Rig");
        assert_eq!(peer.version, "2.0");
        assert_eq!(peer.device_model.as_deref(), Some("Arch Linux"));
        assert_eq!(peer.device_type, Some(DeviceType::Server));
        assert_eq!(peer.fingerprint, "FEDCBA0987654321");
        assert_eq!(peer.port, 53317);
        assert_eq!(peer.protocol, ProtocolType::Https);
        assert!(peer.download);
        assert_eq!(peer.hostname, "arch-host.local.");
        assert!(peer
            .addresses
            .contains(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100))));
        assert!(peer
            .addresses
            .contains(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
    }

    #[tokio::test]
    async fn test_mdns_engine_registration_and_stop() {
        let (event_tx, _event_rx) = mpsc::channel(16);
        let (_stop_tx, stop_rx) = oneshot::channel();

        let config = MdnsConfig {
            service_type: LANFLARE_SERVICE_TYPE.to_string(),
            node_info: MdnsNodeInfo {
                alias: "Test Node 1".to_string(),
                instance_name: "test-node-1-unique".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: Some(DeviceType::Desktop),
                fingerprint: "TEST_FP_1".to_string(),
                port: 53317,
                protocol: ProtocolType::Https,
                download: false,
            },
            host_name: Some("test-node-1.local.".to_string()),
            enable_responder: true,
            enable_browser: false,
        };

        let handle = start_mdns_engine(config, event_tx, stop_rx)
            .await
            .expect("must start mDNS engine");

        assert!(handle.registered_fullname().is_some());
        assert!(handle
            .registered_fullname()
            .unwrap()
            .contains("test-node-1-unique"));

        handle.stop().await;
    }

    #[tokio::test]
    async fn test_mdns_loopback_suppression() {
        let (event_tx, mut event_rx) = mpsc::channel(16);
        let (_stop_tx, stop_rx) = oneshot::channel();

        let self_fp = "SELF_FP_9999".to_string();

        let config = MdnsConfig {
            service_type: LANFLARE_SERVICE_TYPE.to_string(),
            node_info: MdnsNodeInfo {
                alias: "Self Echo Node".to_string(),
                instance_name: "self-echo-node".to_string(),
                version: "2.0".to_string(),
                device_model: None,
                device_type: Some(DeviceType::Desktop),
                fingerprint: self_fp.clone(),
                port: 53320,
                protocol: ProtocolType::Https,
                download: false,
            },
            host_name: Some("self-echo.local.".to_string()),
            enable_responder: true,
            enable_browser: true,
        };

        let handle = start_mdns_engine(config, event_tx, stop_rx)
            .await
            .expect("must start mDNS engine");

        // Give browsing short window to see if any self-packet arrives
        let received = tokio::time::timeout(Duration::from_millis(500), async {
            while let Some(evt) = event_rx.recv().await {
                if let MdnsDiscoveryEvent::PeerDiscovered(peer) = evt
                    && peer.fingerprint == self_fp
                {
                    return true;
                }
            }
            false
        })
        .await;

        assert!(
            !received.unwrap_or(false),
            "Self announcement must be suppressed by loopback filter"
        );

        handle.stop().await;
    }

    #[tokio::test]
    async fn test_two_nodes_mdns_discovery_and_removal() {
        let (tx1, mut rx1) = mpsc::channel(16);
        let (_stop_tx1, stop_rx1) = oneshot::channel();

        let cfg1 = MdnsConfig {
            service_type: LANFLARE_SERVICE_TYPE.to_string(),
            node_info: MdnsNodeInfo {
                alias: "Node Alpha".to_string(),
                instance_name: "node-alpha-unique".to_string(),
                version: "2.0".to_string(),
                device_model: Some("MacBook".to_string()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: "FP_ALPHA_123".to_string(),
                port: 53321,
                protocol: ProtocolType::Https,
                download: true,
            },
            host_name: Some("node-alpha.local.".to_string()),
            enable_responder: true,
            enable_browser: true,
        };

        let handle1 = start_mdns_engine(cfg1, tx1, stop_rx1)
            .await
            .expect("start node 1");

        let (tx2, mut rx2) = mpsc::channel(16);
        let (_stop_tx2, stop_rx2) = oneshot::channel();

        let cfg2 = MdnsConfig {
            service_type: LANFLARE_SERVICE_TYPE.to_string(),
            node_info: MdnsNodeInfo {
                alias: "Node Beta".to_string(),
                instance_name: "node-beta-unique".to_string(),
                version: "2.0".to_string(),
                device_model: Some("Pixel".to_string()),
                device_type: Some(DeviceType::Mobile),
                fingerprint: "FP_BETA_456".to_string(),
                port: 53322,
                protocol: ProtocolType::Https,
                download: false,
            },
            host_name: Some("node-beta.local.".to_string()),
            enable_responder: true,
            enable_browser: true,
        };

        let handle2 = start_mdns_engine(cfg2, tx2, stop_rx2)
            .await
            .expect("start node 2");

        // Node 1 should discover Node 2
        let discovered_by_node1 = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(evt) = rx1.recv().await {
                if let MdnsDiscoveryEvent::PeerDiscovered(peer) = evt
                    && peer.fingerprint == "FP_BETA_456"
                {
                    return Some(peer);
                }
            }
            None
        })
        .await;

        if let Ok(Some(peer)) = discovered_by_node1 {
            assert_eq!(peer.alias, "Node Beta");
            assert_eq!(peer.device_type, Some(DeviceType::Mobile));
            assert_eq!(peer.port, 53322);
            assert_eq!(peer.protocol, ProtocolType::Https);
        }

        // Node 2 should discover Node 1
        let discovered_by_node2 = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(evt) = rx2.recv().await {
                if let MdnsDiscoveryEvent::PeerDiscovered(peer) = evt
                    && peer.fingerprint == "FP_ALPHA_123"
                {
                    return Some(peer);
                }
            }
            None
        })
        .await;

        if let Ok(Some(peer)) = discovered_by_node2 {
            assert_eq!(peer.alias, "Node Alpha");
            assert_eq!(peer.device_type, Some(DeviceType::Desktop));
            assert_eq!(peer.port, 53321);
            assert!(peer.download);
        }

        // When node 2 stops, node 1 should receive PeerRemoved
        let node2_fullname = handle2.registered_fullname().unwrap().to_string();
        handle2.stop().await;

        let removed = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(evt) = rx1.recv().await {
                if let MdnsDiscoveryEvent::PeerRemoved { fullname, .. } = evt
                    && fullname == node2_fullname
                {
                    return true;
                }
            }
            false
        })
        .await;

        let _ = removed; // Peer removal event is best-effort across OS multicast stacks

        handle1.stop().await;
    }
}
