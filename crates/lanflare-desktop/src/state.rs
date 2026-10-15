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

use std::path::PathBuf;

/// Path to persistent JSON configuration file.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanflare")
        .join("config.json")
}

/// Loads persisted application configuration, falling back to defaults.
pub fn load_config() -> AppConfig {
    let path = config_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<AppConfig>(&content) {
            return cfg;
        }
    }
    let default_cfg = AppConfig::default();
    let _ = save_config(&default_cfg);
    default_cfg
}

/// Persists application configuration to disk.
pub fn save_config(cfg: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Path to persistent transfer history JSON file.
pub fn history_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanflare")
        .join("history.json")
}

/// Loads persistent transfer history entries from disk.
pub fn load_history() -> Vec<serde_json::Value> {
    let path = history_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
            return items;
        }
    }
    Vec::new()
}

/// Saves transfer history entries to disk.
pub fn save_history(items: &[serde_json::Value]) -> std::io::Result<()> {
    let path = history_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(items)?;
    std::fs::write(&path, json)?;
    Ok(())
}

impl DesktopState {
    /// Initializes new desktop runtime state with defaults and generated TLS identity.
    pub fn new() -> anyhow::Result<Self> {
        let identity = generate_self_signed()?;
        let config = load_config();
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
