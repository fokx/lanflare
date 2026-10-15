//! Application state and background engine handles for Tauri desktop runtime.

use std::sync::Arc;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::{generate_self_signed, TlsIdentity};
use lanflare_core::discovery::mdns::MdnsHandle;
use lanflare_core::discovery::multicast::MulticastHandle;
use lanflare_core::discovery::registry::PeerRegistry;
use lanflare_core::server::{PrepareUploadDecision, ServerHandle};

/// Mutable or running state of discovery engines.
#[derive(Default)]
pub struct DiscoveryState {
    pub multicast_handle: Option<MulticastHandle>,
    pub mdns_handle: Option<MdnsHandle>,
    pub mc_stop_tx: Option<oneshot::Sender<()>>,
    pub mdns_stop_tx: Option<oneshot::Sender<()>>,
}

/// Incoming transfer request waiting for user decision.
pub struct PendingIncomingSession {
    pub session_id: String,
    pub sender_alias: String,
    pub sender_ip: String,
    pub files: Vec<IncomingFileInfo>,
    pub decision_tx: Option<oneshot::Sender<PrepareUploadDecision>>,
}

/// Simplified file description for incoming transfer approvals in the GUI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncomingFileInfo {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    pub file_type: String,
}

/// Active outgoing transfer session managed from the desktop GUI.
pub struct ActiveOutgoingSession {
    pub session_id: String,
    pub target_alias: String,
    pub target_ip: String,
    pub total_files: usize,
    pub total_bytes: u64,
    pub cancel_token: CancellationToken,
}

/// Shared desktop application state managed by Tauri.
pub struct DesktopState {
    pub config: RwLock<AppConfig>,
    pub identity: TlsIdentity,
    pub registry: Arc<PeerRegistry>,
    pub server_handle: RwLock<Option<ServerHandle>>,
    pub discovery: Mutex<DiscoveryState>,
    pub pending_incoming: Mutex<Option<PendingIncomingSession>>,
    pub active_outgoing: Mutex<Option<ActiveOutgoingSession>>,
}

impl DesktopState {
    /// Initializes new desktop runtime state with defaults and generated TLS identity.
    pub fn new() -> anyhow::Result<Self> {
        let identity = generate_self_signed()?;
        let config = AppConfig::default();
        let registry = PeerRegistry::new();

        Ok(Self {
            config: RwLock::new(config),
            identity,
            registry,
            server_handle: RwLock::new(None),
            discovery: Mutex::new(DiscoveryState::default()),
            pending_incoming: Mutex::new(None),
            active_outgoing: Mutex::new(None),
        })
    }
}
