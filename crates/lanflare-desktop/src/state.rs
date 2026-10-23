//! Application state and background engine handles for Tauri desktop runtime.

use std::sync::Arc;
use tokio::sync::{oneshot, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::{load_or_generate_identity, TlsIdentity};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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
    pub config: Arc<RwLock<AppConfig>>,
    pub identity: Arc<RwLock<TlsIdentity>>,
    pub registry: Arc<PeerRegistry>,
    pub server_handle: RwLock<Option<ServerHandle>>,
    pub dufs_manager: Arc<lanflare_core::server::DufsManager>,
    pub discovery: Mutex<DiscoveryState>,
    pub pending_incoming: Mutex<Option<PendingIncomingSession>>,
    pub pending_browser_downloads: Mutex<std::collections::HashMap<String, oneshot::Sender<bool>>>,
    pub active_outgoing: Mutex<Option<ActiveOutgoingSession>>,
    pub config_file: RwLock<PathBuf>,
    pub history_file: RwLock<PathBuf>,
    pub identity_file: RwLock<PathBuf>,
    pub port_conflict: Arc<std::sync::atomic::AtomicBool>,
    pub retry_running: Arc<std::sync::atomic::AtomicBool>,

    // Multi-Hop Mesh & Virtual Matrix
    pub mesh_router: Arc<lanflare_core::mesh::router::MeshRouter>,
    pub mesh_diagnostics: Arc<lanflare_core::mesh::diagnostics::MeshDiagnostics>,
    pub virtual_matrix: Arc<lanflare_core::mesh::sim::VirtualNetworkMatrix>,
    pub simulated_cluster: Mutex<Option<lanflare_core::mesh::sim::SimulatedMeshCluster>>,

    // Chat Engine & Directory
    pub chat_engine: RwLock<Option<Arc<lanflare_core::chat::engine::ChatEngine>>>,
    pub chat_dir: RwLock<PathBuf>,

    // Toolbox (Universal Clipboard Ring, Search & Collab)
    pub device_ring: Arc<lanflare_core::toolbox::clipboard::DeviceRing>,
    pub catalog: Arc<lanflare_core::toolbox::search::MeshFileCatalog>,
    pub federated_search: Arc<lanflare_core::toolbox::search::FederatedSearchEngine>,
    pub collab_manager: Arc<lanflare_core::toolbox::collab::CollabManager>,
}

use std::path::{Path, PathBuf};

/// Path to persistent JSON configuration file.
pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanflare")
        .join("config.json")
}

/// Path to persistent TLS certificate identity JSON file.
pub fn identity_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanflare")
        .join("identity.json")
}

/// Loads persisted application configuration, falling back to defaults.
pub fn load_config() -> AppConfig {
    let path = config_path();
    if let Ok(content) = std::fs::read_to_string(&path)
        && let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content)
    {
        let mut changed = false;
        if cfg.share_link_pin.trim().is_empty() {
            cfg.share_link_pin = lanflare_core::config::generate_share_link_pin();
            changed = true;
        }
        if cfg.alias.trim().is_empty() || cfg.alias == "LanFlare Node" {
            cfg.alias = lanflare_core::config::generate_random_alias();
            changed = true;
        }
        if changed {
            let _ = save_config(&cfg);
        }
        return cfg;
    }
    let default_cfg = AppConfig::default();
    let _ = save_config(&default_cfg);
    default_cfg
}

/// Saves application configuration to a specific path.
pub fn save_config_file(path: &Path, cfg: &AppConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Persists application configuration to disk.
pub fn save_config(cfg: &AppConfig) -> std::io::Result<()> {
    save_config_file(&config_path(), cfg)
}

/// Path to persistent transfer history JSON file.
pub fn history_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("lanflare")
        .join("history.json")
}

/// Loads persistent transfer history entries from a specific path.
pub fn load_history_file(path: &Path) -> Vec<serde_json::Value> {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&content)
    {
        return items;
    }
    Vec::new()
}

/// Loads persistent transfer history entries from disk.
pub fn load_history() -> Vec<serde_json::Value> {
    load_history_file(&history_path())
}

/// Saves transfer history entries to a specific path.
pub fn save_history_file(path: &Path, items: &[serde_json::Value]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(items)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Saves transfer history entries to disk.
pub fn save_history(items: &[serde_json::Value]) -> std::io::Result<()> {
    save_history_file(&history_path(), items)
}

impl DesktopState {
    /// Initializes new desktop runtime state with defaults and persisted TLS identity.
    pub fn new() -> anyhow::Result<Self> {
        let id_path = identity_path();
        let identity = load_or_generate_identity(&id_path)?;
        let config = load_config();
        let registry = PeerRegistry::new();

        // Mesh Router & Diagnostics
        let graph = Arc::new(tokio::sync::RwLock::new(
            lanflare_core::mesh::graph::MeshTopologyGraph::new(identity.fingerprint.clone()),
        ));
        let router_cfg = lanflare_core::mesh::router::MeshRouterConfig {
            local_fingerprint: identity.fingerprint.clone(),
            local_alias: config.alias.clone(),
            relay_enabled: true,
            ..Default::default()
        };
        let mesh_router = Arc::new(lanflare_core::mesh::router::MeshRouter::with_config(
            router_cfg,
            graph,
        ));
        let mesh_diagnostics = Arc::new(lanflare_core::mesh::diagnostics::MeshDiagnostics::new(&config.alias));
        let virtual_matrix = Arc::new(lanflare_core::mesh::sim::VirtualNetworkMatrix::new());

        // Toolbox: Personal Device Ring
        let secret = lanflare_core::toolbox::clipboard::RingSecret::derive_from_pin("lanflare_ring", "000000");
        let device_ring = lanflare_core::toolbox::clipboard::DeviceRing::new(
            "lanflare_ring",
            "My Devices",
            identity.clone(),
            secret,
        );
        device_ring.attach_mesh_router(mesh_router.clone());

        // Toolbox: Federated Search Catalog & Engine
        let catalog = Arc::new(lanflare_core::toolbox::search::MeshFileCatalog::new(
            &identity.fingerprint,
            &config.alias,
        ));
        let dufs_manager = Arc::new(lanflare_core::server::DufsManager::new());
        let federated_search = Arc::new(
            lanflare_core::toolbox::search::FederatedSearchEngine::new(
                catalog.clone(),
                &identity.fingerprint,
                &config.alias,
                config.port,
            )
            .with_registry(registry.clone())
            .with_mesh_router(mesh_router.clone())
            .with_dufs_manager(dufs_manager.clone()),
        );

        // Toolbox: CRDT Collaborative Scratchpad
        let collab_manager = Arc::new(lanflare_core::toolbox::collab::CollabManager::new(&identity.fingerprint));

        let chat_dir = dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("lanflare")
            .join("chat");

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            identity: Arc::new(RwLock::new(identity)),
            registry,
            server_handle: RwLock::new(None),
            dufs_manager,
            discovery: Mutex::new(DiscoveryState::default()),
            pending_incoming: Mutex::new(None),
            pending_browser_downloads: Mutex::new(std::collections::HashMap::new()),
            active_outgoing: Mutex::new(None),
            config_file: RwLock::new(config_path()),
            history_file: RwLock::new(history_path()),
            identity_file: RwLock::new(id_path),
            port_conflict: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retry_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            mesh_router,
            mesh_diagnostics,
            virtual_matrix,
            simulated_cluster: Mutex::new(None),
            chat_engine: RwLock::new(None),
            chat_dir: RwLock::new(chat_dir),
            device_ring,
            catalog,
            federated_search,
            collab_manager,
        })
    }

    /// Initializes runtime paths from Tauri application context (critical for Android storage).
    pub async fn init_paths(
        &self,
        config_dir: Option<PathBuf>,
        data_dir: Option<PathBuf>,
        #[allow(unused_variables)] download_dir: Option<PathBuf>,
    ) {

        if let Some(cfg_dir) = config_dir {
            let path = cfg_dir.join("lanflare").join("config.json");
            *self.config_file.write().await = path.clone();
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(mut cfg) = serde_json::from_str::<AppConfig>(&content)
            {
                let mut changed = false;
                if cfg.alias.trim().is_empty() || cfg.alias == "LanFlare Node" {
                    cfg.alias = lanflare_core::config::generate_random_alias();
                    changed = true;
                }
                if changed {
                    let _ = save_config_file(&path, &cfg);
                }
                *self.config.write().await = cfg;
            }

            let id_path = cfg_dir.join("lanflare").join("identity.json");
            *self.identity_file.write().await = id_path.clone();
            if let Ok(ident) = load_or_generate_identity(&id_path) {
                *self.identity.write().await = ident;
            }
        }
        if let Some(d_dir) = data_dir {
            let path = d_dir.join("lanflare").join("history.json");
            *self.history_file.write().await = path;
            let c_dir = d_dir.join("lanflare").join("chat");
            *self.chat_dir.write().await = c_dir;
        }
        #[cfg(target_os = "android")]
        {

            let mut cfg = self.config.write().await;
            let public_download = PathBuf::from("/storage/emulated/0/Download");
            let old_data_prefix = "/storage/emulated/0/Android/data";
            if cfg.download_dir.to_string_lossy().contains(old_data_prefix)
                || cfg.download_dir.as_os_str() == "."
                || cfg.download_dir.as_os_str().is_empty()
            {
                cfg.download_dir = public_download;
                let file_path = self.config_file.read().await.clone();
                let _ = save_config_file(&file_path, &cfg);
            }
        }

        #[cfg(not(target_os = "android"))]
        if let Some(dl_dir) = download_dir {
            let mut cfg = self.config.write().await;
            if cfg.download_dir.as_os_str() == "." || cfg.download_dir.as_os_str().is_empty() {
                cfg.download_dir = dl_dir;
                let file_path = self.config_file.read().await.clone();
                let _ = save_config_file(&file_path, &cfg);
            }
        }

    }

    /// Saves configuration to the configured path.
    pub async fn save_config_current(&self, cfg: &AppConfig) -> std::io::Result<()> {
        let path = self.config_file.read().await.clone();
        save_config_file(&path, cfg)
    }

    /// Reads transfer history from the configured path.
    pub async fn load_history_current(&self) -> Vec<serde_json::Value> {
        let path = self.history_file.read().await.clone();
        load_history_file(&path)
    }

    /// Saves transfer history to the configured path.
    pub async fn save_history_current(&self, items: &[serde_json::Value]) -> std::io::Result<()> {
        let path = self.history_file.read().await.clone();
        save_history_file(&path, items)
    }

    /// Returns the active `ChatEngine`, initializing it lazily if not yet created.
    pub async fn get_or_init_chat_engine(&self) -> Result<Arc<lanflare_core::chat::engine::ChatEngine>, String> {
        {
            let guard = self.chat_engine.read().await;
            if let Some(ref ce) = *guard {
                return Ok(ce.clone());
            }
        }

        let mut guard = self.chat_engine.write().await;
        if let Some(ref ce) = *guard {
            return Ok(ce.clone());
        }

        let ident = self.identity.read().await.clone();
        let cfg = self.config.read().await;
        let chat_dir = self.chat_dir.read().await.clone();
        let chat_cfg = lanflare_core::chat::engine::ChatEngineConfig::new(&cfg.alias, &chat_dir);

        let engine = lanflare_core::chat::engine::ChatEngine::new(ident, chat_cfg, self.registry.clone())
            .await
            .map_err(|e| format!("Failed to initialize ChatEngine: {e}"))?;

        engine.attach_mesh_router(self.mesh_router.clone()).await;

        *guard = Some(engine.clone());
        Ok(engine)
    }

    /// Returns a reference to the delay-tolerant OutboxDispatcher if ChatEngine is initialized.
    pub async fn outbox(&self) -> Result<Arc<lanflare_core::chat::outbox::OutboxDispatcher>, String> {
        let ce = self.get_or_init_chat_engine().await?;
        Ok(ce.outbox().clone())
    }
}
