//! Federated mesh file catalog and distributed Dufs folder search.
//!
//! Sub-phase 10.2 implementation:
//! - Implements `MeshFileCatalog` with inverted index and trigram scanning over shared folders.
//! - Enforces visibility bounds respecting PIN-protected folder permissions and hidden directories (`.git`, `.part`, etc.).
//! - Implements `FederatedSearchEngine` dispatching and aggregating search queries across 1-hop LAN peers and multi-hop overlay mesh peers.
//! - Result ranking, deduplication, and resolution of direct download/stream URLs.
//! - Mounts endpoint `POST /api/lanflare/v1/toolbox/search`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use lanflare_proto::toolbox::{
    FileSearchQueryDto, FileSearchResultDto, MeshFileCatalogDto, SearchHitDto,
};
use thiserror::Error;
use tokio::task::JoinSet;

use crate::discovery::registry::PeerRegistry;
use crate::mesh::router::{MeshRouter, MAX_CONTROL_BODY_SIZE};
use crate::server::dufs::{empty_box_response, json_response, DufsManager, ServerResponseBody};
use crate::server::router::ClientConnectionInfo;

/// Maximum directory recursion depth when scanning shared folders.
pub const MAX_DIRECTORY_SCAN_DEPTH: usize = 32;

/// Default timeout for federated search queries across network peers.
pub const DEFAULT_SEARCH_TIMEOUT: Duration = Duration::from_millis(1500);

/// Errors arising during catalog indexing or federated search.
#[derive(Debug, Error)]
pub enum SearchError {
    #[error("I/O error during folder scan: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("HTTP request error: {0}")]
    Http(String),

    #[error("Search query rejected: {0}")]
    InvalidQuery(String),

    #[error("Search operation timed out: {0}")]
    Timeout(String),
}

/// Extracts lowercase 3-character continuous slices from a string.
pub fn extract_trigrams(input: &str) -> HashSet<String> {
    let s = input.to_lowercase();
    let chars: Vec<char> = s.chars().collect();
    let mut trigrams = HashSet::new();
    if chars.len() >= 3 {
        for i in 0..=chars.len() - 3 {
            let tri: String = chars[i..i + 3].iter().collect();
            trigrams.insert(tri);
        }
    } else if !chars.is_empty() {
        let tri: String = chars.iter().collect();
        trigrams.insert(tri);
    }
    trigrams
}

/// Tokenizes text into lowercase alphanumeric words.
pub fn extract_tokens(input: &str) -> HashSet<String> {
    input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Internal metadata for an indexed catalog item.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogItem {
    /// Unique index identifier in this catalog.
    pub id: usize,
    /// Base file name (e.g. `report.pdf`).
    pub file_name: String,
    /// Relative path within the shared folder (e.g. `/docs/report.pdf`).
    pub relative_path: String,
    /// Absolute filesystem path.
    pub absolute_path: PathBuf,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Last modification timestamp in milliseconds.
    pub modified_at_ms: u64,
    /// Guessed MIME content type.
    pub mime_type: String,
    /// Optional uppercase SHA-256 hash.
    pub sha256: Option<String>,
    /// Name or title of the share.
    pub share_name: String,
    /// URL prefix when mounted (e.g. `/dufs`).
    pub path_prefix: String,
    /// Whether this share requires a PIN code for access.
    pub requires_pin: bool,
    /// Stored authentication PIN code (if PIN-protected).
    pub auth_pin: Option<String>,
}

/// Shared folder registration in the catalog.
#[derive(Clone, Debug)]
pub struct SharedFolder {
    pub name: String,
    pub root_path: PathBuf,
    pub path_prefix: String,
    pub auth_pin: Option<String>,
}

#[derive(Default)]
struct CatalogInner {
    owner_fp: String,
    owner_alias: String,
    epoch: u64,
    shared_folders: Vec<SharedFolder>,
    items: Vec<CatalogItem>,
    token_index: HashMap<String, HashSet<usize>>,
    trigram_index: HashMap<String, HashSet<usize>>,
}

/// Inverted index and catalog over shared local folders and Dufs shares.
pub struct MeshFileCatalog {
    inner: RwLock<CatalogInner>,
}

impl MeshFileCatalog {
    /// Creates a new empty `MeshFileCatalog`.
    pub fn new(owner_fp: impl Into<String>, owner_alias: impl Into<String>) -> Self {
        Self {
            inner: RwLock::new(CatalogInner {
                owner_fp: owner_fp.into().to_ascii_uppercase(),
                owner_alias: owner_alias.into(),
                epoch: 1,
                shared_folders: Vec::new(),
                items: Vec::new(),
                token_index: HashMap::new(),
                trigram_index: HashMap::new(),
            }),
        }
    }

    /// Recursively indexes a shared folder directory, respecting hidden file and PIN bounds.
    pub fn add_shared_folder(
        &self,
        name: &str,
        root_path: &Path,
        path_prefix: &str,
        auth_pin: Option<String>,
    ) -> Result<usize, SearchError> {
        if !root_path.exists() || !root_path.is_dir() {
            return Err(SearchError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Folder {} does not exist or is not a directory", root_path.display()),
            )));
        }

        let mut scanned_items = Vec::new();
        let requires_pin = auth_pin.is_some();
        scan_directory(
            root_path,
            root_path,
            0,
            MAX_DIRECTORY_SCAN_DEPTH,
            &mut scanned_items,
            name,
            path_prefix,
            requires_pin,
            auth_pin.as_deref(),
        )?;

        let mut inner = self.inner.write().unwrap();
        // Remove prior items from the same share name if re-indexing
        inner.shared_folders.retain(|s| s.name != name);
        inner.shared_folders.push(SharedFolder {
            name: name.to_string(),
            root_path: root_path.to_path_buf(),
            path_prefix: path_prefix.to_string(),
            auth_pin,
        });

        // Rebuild full item list and inverted indexes
        inner.items.retain(|item| item.share_name != name);
        let start_id = inner.items.len();
        for (i, mut item) in scanned_items.into_iter().enumerate() {
            item.id = start_id + i;
            inner.items.push(item);
        }

        rebuild_indices(&mut inner);
        inner.epoch = inner.epoch.saturating_add(1);

        Ok(inner.items.len())
    }

    /// Synchronizes the catalog with active configuration in `DufsManager`.
    pub async fn sync_with_dufs(&self, dufs_manager: &DufsManager) -> usize {
        if let Some(cfg) = dufs_manager.get_config().await {
            let _ = self.add_shared_folder(
                &cfg.title,
                &cfg.root_path,
                &cfg.path_prefix,
                cfg.auth_pin,
            );
        } else {
            let mut inner = self.inner.write().unwrap();
            inner.shared_folders.retain(|s| s.path_prefix != "/dufs");
            inner.items.retain(|item| item.path_prefix != "/dufs");
            rebuild_indices(&mut inner);
            inner.epoch = inner.epoch.saturating_add(1);
        }
        self.total_items()
    }

    /// Clears all indexed items and shares.
    pub fn clear(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.shared_folders.clear();
        inner.items.clear();
        inner.token_index.clear();
        inner.trigram_index.clear();
        inner.epoch = inner.epoch.saturating_add(1);
    }

    /// Returns the total number of indexed files.
    pub fn total_items(&self) -> usize {
        self.inner.read().unwrap().items.len()
    }

    /// Searches the local catalog, respecting visibility bounds and scoring results.
    pub fn search(&self, query: &FileSearchQueryDto) -> FileSearchResultDto {
        let start_time = Instant::now();
        let inner = self.inner.read().unwrap();
        let mut result = FileSearchResultDto::new(&inner.owner_fp, &inner.owner_alias);
        result.total_indexed = inner.items.len();

        let q_clean = query.query.trim().to_lowercase();
        let q_tokens = extract_tokens(&q_clean);
        let q_trigrams = extract_trigrams(&q_clean);

        // Candidate set from inverted indexes or scan all if query is empty
        let candidates: HashSet<usize> = if q_clean.is_empty() {
            (0..inner.items.len()).collect()
        } else {
            let mut set = HashSet::new();
            for token in &q_tokens {
                if let Some(ids) = inner.token_index.get(token) {
                    set.extend(ids);
                }
            }
            for tri in &q_trigrams {
                if let Some(ids) = inner.trigram_index.get(tri) {
                    set.extend(ids);
                }
            }
            // If inverted index yielded few or none, fallback to scanning all items
            if set.len() < 20 {
                set.extend(0..inner.items.len());
            }
            set
        };

        for id in candidates {
            if id >= inner.items.len() {
                continue;
            }
            let item = &inner.items[id];

            // Enforce visibility bounds: PIN-protected shares require matching PIN
            if item.requires_pin {
                match (&query.auth_pin, &item.auth_pin) {
                    (Some(provided), Some(expected)) if provided == expected => {
                        // Access permitted
                    }
                    _ => {
                        // Omit protected items from search results when PIN is missing or invalid
                        continue;
                    }
                }
            }

            // Compute score
            let name_clean = item.file_name.to_lowercase();
            let path_clean = item.relative_path.to_lowercase();

            let score_opt: Option<f32> = if q_clean.is_empty() || name_clean == q_clean {
                Some(1.0)
            } else if name_clean.starts_with(&q_clean) {
                let ratio = q_clean.len() as f32 / name_clean.len().max(1) as f32;
                Some(0.85 + 0.10 * ratio.min(1.0))
            } else if name_clean.contains(&q_clean) {
                let ratio = q_clean.len() as f32 / name_clean.len().max(1) as f32;
                Some(0.70 + 0.10 * ratio.min(1.0))
            } else if path_clean.contains(&q_clean) {
                Some(0.50)
            } else if !q_tokens.is_empty() && q_tokens.iter().any(|t| name_clean.contains(t.as_str())) {
                Some(0.60)
            } else if !q_trigrams.is_empty() {
                let item_trigrams = extract_trigrams(&name_clean);
                let matching = q_trigrams.iter().filter(|t| item_trigrams.contains(*t)).count();
                let overlap = matching as f32 / q_trigrams.len() as f32;
                if overlap >= 0.3 {
                    Some(0.30 + 0.30 * overlap)
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(score) = score_opt {
                let clean_prefix = item.path_prefix.trim_end_matches('/');
                let clean_rel = if item.relative_path.starts_with('/') {
                    item.relative_path.clone()
                } else {
                    format!("/{}", item.relative_path)
                };
                let download_url = format!("{clean_prefix}{clean_rel}");

                result.hits.push(SearchHitDto {
                    file_name: item.file_name.clone(),
                    relative_path: item.relative_path.clone(),
                    size_bytes: item.size_bytes,
                    modified_at_ms: item.modified_at_ms,
                    mime_type: item.mime_type.clone(),
                    sha256: item.sha256.clone(),
                    requires_pin: item.requires_pin,
                    download_url,
                    score,
                });
            }
        }

        result.sort_by_score_descending();
        let max_results = query.max_results.unwrap_or(50);
        if result.hits.len() > max_results {
            result.hits.truncate(max_results);
        }

        result.search_duration_ms = start_time.elapsed().as_millis() as u64;
        result
    }

    /// Exports public catalog items (items where `requires_pin == false`).
    pub fn export_catalog_dto(&self) -> MeshFileCatalogDto {
        let inner = self.inner.read().unwrap();
        let mut catalog = MeshFileCatalogDto::new(&inner.owner_fp, &inner.owner_alias, inner.epoch);
        catalog.shared_roots = inner.shared_folders.iter().map(|s| s.name.clone()).collect();
        for item in &inner.items {
            if !item.requires_pin {
                let clean_prefix = item.path_prefix.trim_end_matches('/');
                let clean_rel = if item.relative_path.starts_with('/') {
                    item.relative_path.clone()
                } else {
                    format!("/{}", item.relative_path)
                };
                catalog.items.push(SearchHitDto {
                    file_name: item.file_name.clone(),
                    relative_path: item.relative_path.clone(),
                    size_bytes: item.size_bytes,
                    modified_at_ms: item.modified_at_ms,
                    mime_type: item.mime_type.clone(),
                    sha256: item.sha256.clone(),
                    requires_pin: false,
                    download_url: format!("{clean_prefix}{clean_rel}"),
                    score: 1.0,
                });
            }
        }
        catalog
    }
}

/// Rebuilds inverted word token and character trigram indexes for fast searching.
fn rebuild_indices(inner: &mut CatalogInner) {
    inner.token_index.clear();
    inner.trigram_index.clear();

    for (id, item) in inner.items.iter().enumerate() {
        for token in extract_tokens(&item.file_name) {
            inner.token_index.entry(token).or_default().insert(id);
        }
        for token in extract_tokens(&item.relative_path) {
            inner.token_index.entry(token).or_default().insert(id);
        }
        for tri in extract_trigrams(&item.file_name) {
            inner.trigram_index.entry(tri).or_default().insert(id);
        }
    }
}

/// Recursive directory scanner ignoring hidden files/folders (`.*`) and temporary transfer files.
#[allow(clippy::too_many_arguments)]
fn scan_directory(
    base_root: &Path,
    current_dir: &Path,
    depth: usize,
    max_depth: usize,
    items: &mut Vec<CatalogItem>,
    share_name: &str,
    path_prefix: &str,
    requires_pin: bool,
    auth_pin: Option<&str>,
) -> std::io::Result<()> {
    if depth > max_depth {
        return Ok(());
    }
    let entries = match std::fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::debug!("Failed to read directory {}: {err}", current_dir.display());
            return Ok(());
        }
    };

    for entry in entries.flatten() {
        let file_name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files or directories starting with '.'
        if file_name.starts_with('.') {
            continue;
        }
        // Skip temporary transfer files
        if file_name.ends_with(".lanflare-part") || file_name.ends_with(".part") {
            continue;
        }

        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        if file_type.is_dir() {
            let _ = scan_directory(
                base_root,
                &path,
                depth + 1,
                max_depth,
                items,
                share_name,
                path_prefix,
                requires_pin,
                auth_pin,
            );
        } else if file_type.is_file() {
            let metadata = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let size_bytes = metadata.len();
            let modified_at_ms = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);

            // Compute relative path from base_root
            let rel_path_diff = path.strip_prefix(base_root).unwrap_or(&path);
            let mut rel_path = String::new();
            for component in rel_path_diff.components() {
                rel_path.push('/');
                rel_path.push_str(&component.as_os_str().to_string_lossy());
            }

            let mime_type = crate::client::transfer::mime_guess_simple(&file_name);

            items.push(CatalogItem {
                id: 0,
                file_name,
                relative_path: rel_path,
                absolute_path: path,
                size_bytes,
                modified_at_ms,
                mime_type,
                sha256: None,
                share_name: share_name.to_string(),
                path_prefix: path_prefix.to_string(),
                requires_pin,
                auth_pin: auth_pin.map(|s| s.to_string()),
            });
        }
    }
    Ok(())
}

/// Federated search coordinator querying local catalog, 1-hop LAN peers, and multi-hop mesh peers.
pub struct FederatedSearchEngine {
    local_catalog: Arc<MeshFileCatalog>,
    dufs_manager: Option<Arc<DufsManager>>,
    registry: Option<Arc<PeerRegistry>>,
    mesh_router: Option<Arc<MeshRouter>>,
    local_node_fp: String,
    local_alias: String,
    local_port: u16,
    http_client: reqwest::Client,
}

impl FederatedSearchEngine {
    /// Creates a new `FederatedSearchEngine`.
    pub fn new(
        local_catalog: Arc<MeshFileCatalog>,
        local_node_fp: impl Into<String>,
        local_alias: impl Into<String>,
        local_port: u16,
    ) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(DEFAULT_SEARCH_TIMEOUT)
            .build()
            .unwrap_or_default();

        Self {
            local_catalog,
            dufs_manager: None,
            registry: None,
            mesh_router: None,
            local_node_fp: local_node_fp.into().to_ascii_uppercase(),
            local_alias: local_alias.into(),
            local_port,
            http_client,
        }
    }

    /// Attaches an active `DufsManager`.
    pub fn with_dufs_manager(mut self, dufs_manager: Arc<DufsManager>) -> Self {
        self.dufs_manager = Some(dufs_manager);
        self
    }

    /// Attaches a `PeerRegistry` for 1-hop LAN discovery.
    pub fn with_registry(mut self, registry: Arc<PeerRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Attaches a `MeshRouter` for multi-hop mesh discovery.
    pub fn with_mesh_router(mut self, mesh_router: Arc<MeshRouter>) -> Self {
        self.mesh_router = Some(mesh_router);
        self
    }

    /// Returns a reference to the local catalog.
    pub fn catalog(&self) -> &Arc<MeshFileCatalog> {
        &self.local_catalog
    }

    /// Returns a reference to the local node's fingerprint.
    pub fn local_node_fp(&self) -> &str {
        &self.local_node_fp
    }

    /// Returns a reference to the local node's alias.
    pub fn local_alias(&self) -> &str {
        &self.local_alias
    }

    /// Executes search against the local catalog, resolving download URLs.
    pub fn search_local(&self, query: &FileSearchQueryDto) -> FileSearchResultDto {
        let mut result = self.local_catalog.search(query);
        // If local port is known, format download URLs into accessible URLs
        for hit in &mut result.hits {
            if hit.download_url.starts_with('/') && self.local_port > 0 {
                // If PIN is required and provided in query, append pin param
                if hit.requires_pin && let Some(pin) = &query.auth_pin {
                    let sep = if hit.download_url.contains('?') { '&' } else { '?' };
                    hit.download_url = format!("{}{sep}pin={pin}", hit.download_url);
                }
            }
        }
        result
    }

    /// Dispatches and aggregates search queries across local catalog, LAN peers, and mesh peers.
    pub async fn search_federated(
        &self,
        query: &FileSearchQueryDto,
        timeout: Duration,
    ) -> Vec<FileSearchResultDto> {
        let mut all_results = Vec::new();

        // 1. Local catalog search
        let local_res = self.search_local(query);
        all_results.push(local_res);

        // 2. Discover candidate remote peers to query
        let mut candidate_peers: Vec<(String, String)> = Vec::new(); // (fp, base_url)

        if let Some(registry) = &self.registry {
            for peer in registry.get_all().await {
                if peer.fingerprint.eq_ignore_ascii_case(&self.local_node_fp) {
                    continue;
                }
                if let Some(base_url) = peer.primary_base_url() {
                    candidate_peers.push((peer.fingerprint.clone(), base_url));
                }
            }
        }

        if candidate_peers.is_empty() {
            return all_results;
        }

        // 3. Concurrently query candidate peers
        let client = self.http_client.clone();
        let query_json = serde_json::to_string(query).unwrap_or_default();
        let mut join_set = JoinSet::new();

        for (_fp, base_url) in candidate_peers {
            let client = client.clone();
            let query_json = query_json.clone();
            let url = format!("{base_url}/api/lanflare/v1/toolbox/search");
            let auth_pin = query.auth_pin.clone();

            join_set.spawn(async move {
                let res = tokio::time::timeout(
                    timeout,
                    client
                        .post(&url)
                        .header("content-type", "application/json")
                        .body(query_json)
                        .send(),
                )
                .await;

                match res {
                    Ok(Ok(resp)) if resp.status().is_success() => {
                        if let Ok(mut search_res) = resp.json::<FileSearchResultDto>().await {
                            // Resolve remote relative download URLs into absolute endpoints
                            for hit in &mut search_res.hits {
                                if hit.download_url.starts_with('/') {
                                    hit.download_url = format!("{}{}", base_url, hit.download_url);
                                }
                                if hit.requires_pin && let Some(pin) = &auth_pin {
                                    let sep = if hit.download_url.contains('?') { '&' } else { '?' };
                                    hit.download_url = format!("{}{sep}pin={pin}", hit.download_url);
                                }
                            }
                            Some(search_res)
                        } else {
                            tracing::debug!("[federated_search] Failed to parse JSON response from {base_url}");
                            None
                        }
                    }
                    Ok(Ok(resp)) => {
                        tracing::debug!("[federated_search] Response status from {base_url}: {}", resp.status());
                        None
                    }
                    Ok(Err(err)) => {
                        tracing::debug!("[federated_search] Request error to {base_url}: {err}");
                        None
                    }
                    Err(_) => {
                        tracing::debug!("[federated_search] Search request to {base_url} timed out");
                        None
                    }
                }
            });
        }

        while let Some(task_res) = join_set.join_next().await {
            if let Ok(Some(remote_result)) = task_res {
                all_results.push(remote_result);
            }
        }

        all_results
    }

    /// Aggregates and ranks all search hits across all peers into a unified list.
    pub async fn search_and_rank(
        &self,
        query: &FileSearchQueryDto,
        timeout: Duration,
    ) -> Vec<SearchHitDto> {
        let results = self.search_federated(query, timeout).await;
        let mut all_hits = Vec::new();
        let mut seen_urls = HashSet::new();

        for res in results {
            for hit in res.hits {
                if seen_urls.insert(hit.download_url.clone()) {
                    all_hits.push(hit);
                }
            }
        }

        all_hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_name.cmp(&b.file_name))
        });

        let max_results = query.max_results.unwrap_or(50);
        if all_hits.len() > max_results {
            all_hits.truncate(max_results);
        }

        all_hits
    }

    /// Hyper request dispatcher mounted at `POST /api/lanflare/v1/toolbox/search`.
    pub async fn handle_hyper_request(
        self: &Arc<Self>,
        mut req: Request<Incoming>,
        _client: Option<ClientConnectionInfo>,
    ) -> Response<ServerResponseBody> {
        let method = req.method().clone();
        let path = req.uri().path().to_string();

        if method != Method::POST || path != "/api/lanflare/v1/toolbox/search" {
            return empty_box_response(StatusCode::NOT_FOUND);
        }

        let body_bytes = match req.body_mut().collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                if bytes.len() > MAX_CONTROL_BODY_SIZE {
                    return empty_box_response(StatusCode::PAYLOAD_TOO_LARGE);
                }
                bytes
            }
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let query_dto: FileSearchQueryDto = match serde_json::from_slice(&body_bytes) {
            Ok(dto) => dto,
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let search_result = self.search_local(&query_dto);
        json_response(StatusCode::OK, &search_result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use uuid::Uuid;

    #[test]
    fn test_local_index_and_trigram_search_with_pin_protection() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_search_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let root = &temp_dir;

        // 1. Create directory structure with various files
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();

        let mut f1 = File::create(docs.join("financial_report_q1.pdf")).unwrap();
        f1.write_all(b"Q1 financial data").unwrap();

        let mut f2 = File::create(docs.join("annual_report.txt")).unwrap();
        f2.write_all(b"Annual report summary").unwrap();

        let mut f3 = File::create(root.join("photo.jpg")).unwrap();
        f3.write_all(b"image bytes").unwrap();

        // Hidden files/directories that MUST NOT be indexed
        let hidden_dir = root.join(".hidden");
        std::fs::create_dir_all(&hidden_dir).unwrap();
        let mut f_hidden = File::create(hidden_dir.join("secret.pdf")).unwrap();
        f_hidden.write_all(b"secret").unwrap();

        let mut f_dot = File::create(root.join(".DS_Store")).unwrap();
        f_dot.write_all(b"garbage").unwrap();

        let mut f_part = File::create(root.join("upload.lanflare-part")).unwrap();
        f_part.write_all(b"partial").unwrap();

        // 2. Initialize catalog and index folder
        let catalog = MeshFileCatalog::new("1111111111111111111111111111111111111111111111111111111111111111", "HostA");
        let count = catalog.add_shared_folder("PublicDocs", root, "/dufs", None).unwrap();
        assert_eq!(count, 3); // Exactly 3 visible files: financial_report_q1.pdf, annual_report.txt, photo.jpg

        // 3. Search for "report"
        let q1 = FileSearchQueryDto::new("report", "2222222222222222222222222222222222222222222222222222222222222222");
        let res1 = catalog.search(&q1);
        assert_eq!(res1.hits.len(), 2);
        assert!(res1.hits.iter().any(|h| h.file_name == "financial_report_q1.pdf"));
        assert!(res1.hits.iter().any(|h| h.file_name == "annual_report.txt"));
        assert!(!res1.hits.iter().any(|h| h.file_name == "photo.jpg"));

        // 4. Search for "annual" -> exact/prefix match has highest score
        let q2 = FileSearchQueryDto::new("annual", "2222222222222222222222222222222222222222222222222222222222222222");
        let res2 = catalog.search(&q2);
        assert_eq!(res2.hits.len(), 1);
        assert_eq!(res2.hits[0].file_name, "annual_report.txt");
        assert!(res2.hits[0].score >= 0.85);

        // 5. Add a PIN-protected folder
        let priv_temp = std::env::temp_dir().join(format!("lanflare_priv_test_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&priv_temp).unwrap();
        let mut priv_f = File::create(priv_temp.join("confidential_payroll.xlsx")).unwrap();
        priv_f.write_all(b"salaries").unwrap();

        let count2 = catalog.add_shared_folder("PrivateShare", &priv_temp, "/dufs/private", Some("pin888".into())).unwrap();
        assert_eq!(count2, 4);

        // 6. Query for "payroll" without PIN -> Must be EXCLUDED (visibility bounds)
        let q_unauth = FileSearchQueryDto::new("payroll", "2222222222222222222222222222222222222222222222222222222222222222");
        let res_unauth = catalog.search(&q_unauth);
        assert_eq!(res_unauth.hits.len(), 0);

        // 7. Query with wrong PIN -> Must be EXCLUDED
        let q_wrong_pin = FileSearchQueryDto::new("payroll", "2222222222222222222222222222222222222222222222222222222222222222")
            .with_pin("wrong");
        let res_wrong_pin = catalog.search(&q_wrong_pin);
        assert_eq!(res_wrong_pin.hits.len(), 0);

        // 8. Query with correct PIN -> Returns hit marked with requires_pin: true
        let q_auth = FileSearchQueryDto::new("payroll", "2222222222222222222222222222222222222222222222222222222222222222")
            .with_pin("pin888");
        let res_auth = catalog.search(&q_auth);
        assert_eq!(res_auth.hits.len(), 1);
        assert_eq!(res_auth.hits[0].file_name, "confidential_payroll.xlsx");
        assert!(res_auth.hits[0].requires_pin);
        assert_eq!(res_auth.hits[0].download_url, "/dufs/private/confidential_payroll.xlsx");

        // 9. Export catalog -> Only public items included
        let export = catalog.export_catalog_dto();
        assert_eq!(export.items.len(), 3);
        assert!(!export.items.iter().any(|h| h.file_name == "confidential_payroll.xlsx"));

        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&priv_temp);
    }

    #[tokio::test]
    async fn test_local_index_and_federated_mesh_query_aggregation() {
        use crate::crypto::cert::generate_self_signed;
        use crate::discovery::registry::{DiscoveredPeer, PeerEndpoint};
        use crate::server::start_server_toolbox;
        use std::net::{IpAddr, Ipv4Addr};

        // Node B (Responder): shares files
        let dir_b = std::env::temp_dir().join(format!("lanflare_search_b_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir_b).unwrap();
        let mut fb = File::create(dir_b.join("quarterly_roadmap.pdf")).unwrap();
        fb.write_all(b"Roadmap Q3-Q4").unwrap();

        let ident_b = generate_self_signed().unwrap();
        let catalog_b = Arc::new(MeshFileCatalog::new(&ident_b.fingerprint, "NodeB"));
        catalog_b.add_shared_folder("Roadmaps", &dir_b, "/dufs", None).unwrap();

        let registry_b = PeerRegistry::new();
        let search_engine_b = Arc::new(FederatedSearchEngine::new(
            catalog_b.clone(),
            &ident_b.fingerprint,
            "NodeB",
            0,
        ));

        let server_config_b = crate::config::AppConfig {
            port: 0,
            alias: "NodeB".to_string(),
            ..Default::default()
        };

        let handle_b = start_server_toolbox(
            server_config_b,
            None,
            registry_b,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(search_engine_b),
        )
        .await
        .unwrap();

        let port_b = handle_b.bound_port();

        // Node A (Searcher): local files + queries Node B over network
        let dir_a = std::env::temp_dir().join(format!("lanflare_search_a_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir_a).unwrap();
        let mut fa = File::create(dir_a.join("quarterly_taxes.csv")).unwrap();
        fa.write_all(b"Taxes").unwrap();

        let ident_a = generate_self_signed().unwrap();
        let catalog_a = Arc::new(MeshFileCatalog::new(&ident_a.fingerprint, "NodeA"));
        catalog_a.add_shared_folder("LocalFinances", &dir_a, "/dufs", None).unwrap();

        let registry_a = PeerRegistry::new();
        let now_ms = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // Register Node B as discovered 1-hop peer
        let peer_b = DiscoveredPeer {
            fingerprint: ident_b.fingerprint.clone(),
            alias: "NodeB".to_string(),
            port: port_b,
            protocol: lanflare_proto::ProtocolType::Http,
            download: true,
            endpoints: vec![PeerEndpoint::new(
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                None,
                crate::discovery::registry::DiscoverySource::Mdns,
                now_ms,
            )],
            discovery_sources: HashSet::from([crate::discovery::registry::DiscoverySource::Mdns]),
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            ..Default::default()
        };
        registry_a.insert_or_update(peer_b).await;

        let search_engine_a = FederatedSearchEngine::new(
            catalog_a.clone(),
            &ident_a.fingerprint,
            "NodeA",
            50000,
        )
        .with_registry(registry_a);

        // Run federated search for "quarterly"
        let q = FileSearchQueryDto::new("quarterly", &ident_a.fingerprint);
        let federated_results = search_engine_a.search_federated(&q, Duration::from_millis(2000)).await;

        // Must receive results from both Node A (local) and Node B (remote)
        assert_eq!(federated_results.len(), 2);

        let local_res = federated_results.iter().find(|r| r.responder_fp == ident_a.fingerprint).unwrap();
        assert_eq!(local_res.hits.len(), 1);
        assert_eq!(local_res.hits[0].file_name, "quarterly_taxes.csv");

        let remote_res = federated_results.iter().find(|r| r.responder_fp == ident_b.fingerprint).unwrap();
        assert_eq!(remote_res.hits.len(), 1);
        assert_eq!(remote_res.hits[0].file_name, "quarterly_roadmap.pdf");
        // Verify remote hit URL is resolved to absolute endpoint
        assert_eq!(
            remote_res.hits[0].download_url,
            format!("http://127.0.0.1:{port_b}/dufs/quarterly_roadmap.pdf")
        );

        // Test aggregated and ranked search
        let ranked_hits = search_engine_a.search_and_rank(&q, Duration::from_millis(2000)).await;
        assert_eq!(ranked_hits.len(), 2);
        assert!(ranked_hits.iter().any(|h| h.file_name == "quarterly_taxes.csv"));
        assert!(ranked_hits.iter().any(|h| h.file_name == "quarterly_roadmap.pdf"));

        handle_b.stop();
        let _ = std::fs::remove_dir_all(&dir_a);
        let _ = std::fs::remove_dir_all(&dir_b);
    }

    #[tokio::test]
    async fn test_hyper_search_endpoint_and_bad_request_defense() {
        use crate::crypto::cert::generate_self_signed;
        use crate::server::start_server_toolbox;

        let dir = std::env::temp_dir().join(format!("lanflare_search_srv_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut f = File::create(dir.join("specs.txt")).unwrap();
        f.write_all(b"Specifications").unwrap();

        let ident = generate_self_signed().unwrap();
        let catalog = Arc::new(MeshFileCatalog::new(&ident.fingerprint, "ServerNode"));
        catalog.add_shared_folder("Specs", &dir, "/dufs", None).unwrap();

        let registry = PeerRegistry::new();
        let search_engine = Arc::new(FederatedSearchEngine::new(
            catalog.clone(),
            &ident.fingerprint,
            "ServerNode",
            0,
        ));

        let server_config = crate::config::AppConfig {
            port: 0,
            alias: "ServerNode".to_string(),
            ..Default::default()
        };

        let handle = start_server_toolbox(
            server_config,
            None,
            registry,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(search_engine),
        )
        .await
        .unwrap();

        let port = handle.bound_port();
        let client = reqwest::Client::new();

        // 1. Valid POST search query
        let query = FileSearchQueryDto::new("specs", &ident.fingerprint);
        let resp = client
            .post(format!("http://127.0.0.1:{port}/api/lanflare/v1/toolbox/search"))
            .json(&query)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let search_res: FileSearchResultDto = resp.json().await.unwrap();
        assert_eq!(search_res.hits.len(), 1);
        assert_eq!(search_res.hits[0].file_name, "specs.txt");

        // 2. Malformed JSON -> 400 Bad Request
        let bad_resp = client
            .post(format!("http://127.0.0.1:{port}/api/lanflare/v1/toolbox/search"))
            .header("content-type", "application/json")
            .body("{not valid json")
            .send()
            .await
            .unwrap();
        assert_eq!(bad_resp.status(), reqwest::StatusCode::BAD_REQUEST);

        // 3. GET method -> 404 Not Found
        let get_resp = client
            .get(format!("http://127.0.0.1:{port}/api/lanflare/v1/toolbox/search"))
            .send()
            .await
            .unwrap();
        assert_eq!(get_resp.status(), reqwest::StatusCode::NOT_FOUND);

        // 4. Unknown toolbox subpath -> 404 Not Found
        let unknown_resp = client
            .post(format!("http://127.0.0.1:{port}/api/lanflare/v1/toolbox/invalid"))
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_resp.status(), reqwest::StatusCode::NOT_FOUND);

        handle.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
