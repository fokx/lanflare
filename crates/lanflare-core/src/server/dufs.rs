//! Dufs-alike static file and directory server for LanFlare peers on the local network.
//!
//! Provides:
//! - Recursive directory browsing with breadcrumbs and fast search/filter
//! - Streaming file downloads with HTTP Range support (`206 Partial Content`)
//! - On-the-fly directory ZIP streaming (`?zip`) with bounded RAM and zero temporary files
//! - Optional PIN protection and peer upload support
//! - JSON directory listing API (`?json` or `Accept: application/json`)
//! - Robust path validation with Android storage/symlink compatibility

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures_util::TryStreamExt;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::{Frame, Incoming};
use hyper::header::{
    ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
    CONTENT_TYPE, RANGE, WWW_AUTHENTICATE,
};
use hyper::{Method, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncSeekExt;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;

use crate::client::transfer::mime_guess_simple;

/// Standard boxed HTTP response body for hyper v1.
pub type ServerResponseBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// Helper creating a boxed full body from bytes.
pub fn full_box_body<B: Into<Bytes>>(bytes: B) -> ServerResponseBody {
    Full::new(bytes.into())
        .map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
        .boxed()
}

/// Helper converting a `Full<Bytes>` to `ServerResponseBody`.
pub fn full_to_box_body(full: Full<Bytes>) -> ServerResponseBody {
    full.map_err(|never| -> Box<dyn std::error::Error + Send + Sync> { match never {} })
        .boxed()
}

/// Custom Body wrapper ensuring `Send + Sync + 'static` for boxed hyper v1 responses.
pub struct SyncStreamBody<S>(pub std::sync::Mutex<S>);

impl<S, T, E> hyper::body::Body for SyncStreamBody<S>
where
    S: futures_util::Stream<Item = Result<T, E>> + Unpin,
    T: hyper::body::Buf,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Data = T;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut guard = match self.0.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        match std::pin::Pin::new(&mut *guard).poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(data))) => {
                std::task::Poll::Ready(Some(Ok(Frame::data(data))))
            }
            std::task::Poll::Ready(Some(Err(err))) => {
                std::task::Poll::Ready(Some(Err(err.into())))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

/// Helper creating a boxed stream body.
pub fn stream_box_body<S>(stream: S) -> ServerResponseBody
where
    S: futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Unpin + Send + 'static,
{
    SyncStreamBody(std::sync::Mutex::new(stream)).boxed()
}

/// Helper building an empty response.
pub fn empty_box_response(status: StatusCode) -> Response<ServerResponseBody> {
    Response::builder()
        .status(status)
        .body(full_box_body(Bytes::new()))
        .unwrap()
}

/// Helper building an HTML or text response.
pub fn html_response(status: StatusCode, html: String) -> Response<ServerResponseBody> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .body(full_box_body(html))
        .unwrap()
}

/// Helper building a JSON response.
pub fn json_response<T: Serialize>(status: StatusCode, data: &T) -> Response<ServerResponseBody> {
    let json_bytes = serde_json::to_vec(data).unwrap_or_default();
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json; charset=utf-8")
        .body(full_box_body(json_bytes))
        .unwrap()
}

/// Configuration options for the Dufs-alike file server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DufsConfig {
    /// Root directory path being served.
    pub root_path: PathBuf,
    /// Path prefix when mounted on a sub-route (e.g. "" or "/dufs").
    #[serde(default)]
    pub path_prefix: String,
    /// Whether peers on the network may upload files to this folder.
    #[serde(default)]
    pub allow_upload: bool,
    /// Whether peers on the network may delete files/folders.
    #[serde(default)]
    pub allow_delete: bool,
    /// Optional security PIN/password required for access.
    #[serde(default)]
    pub auth_pin: Option<String>,
    /// Display title for the web portal.
    #[serde(default = "default_dufs_title")]
    pub title: String,
}

fn default_dufs_title() -> String {
    "LanFlare File Share".to_string()
}

/// Directory entry DTO returned by JSON API and used by HTML renderer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DufsEntryDto {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
    pub mime_type: String,
}

/// Manager coordinating the active Dufs server instance.
#[derive(Debug, Default)]
pub struct DufsManager {
    active_config: RwLock<Option<DufsConfig>>,
}

impl DufsManager {
    pub fn new() -> Self {
        Self {
            active_config: RwLock::new(None),
        }
    }

    /// Sets or replaces the active dufs folder sharing configuration.
    pub async fn set_config(&self, config: DufsConfig) {
        let mut guard = self.active_config.write().await;
        *guard = Some(config);
    }

    /// Clears the active dufs configuration, disabling folder sharing.
    pub async fn clear_config(&self) {
        let mut guard = self.active_config.write().await;
        *guard = None;
    }

    /// Retrieves the current dufs configuration if active.
    pub async fn get_config(&self) -> Option<DufsConfig> {
        self.active_config.read().await.clone()
    }

    /// Checks whether folder sharing is currently active.
    pub async fn is_active(&self) -> bool {
        self.active_config.read().await.is_some()
    }
}

/// Handle to a standalone running Dufs HTTP server.
pub struct DufsServerHandle {
    pub port: u16,
    pub cancel_token: CancellationToken,
    pub join_handle: tokio::task::JoinHandle<()>,
}

impl DufsServerHandle {
    pub fn stop(&self) {
        self.cancel_token.cancel();
    }
}

/// Percent-decodes a URL path string.
pub fn percent_decode(input: &str) -> String {
    let mut result = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h1), Some(h2)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            )
        {
            result.push((h1 * 16 + h2) as u8);
            i += 3;
            continue;
        }
        result.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(result).unwrap_or_else(|_| input.to_string())
}

/// Safely resolves and validates a URL path against a root directory.
///
/// Special care for Android:
/// 1. Strips any path prefix.
/// 2. Normalizes path components lexically (prevents `..` traversal).
/// 3. Handles Android symlinks/mount namespaces (`/storage/emulated/0` vs `/data/media/0`).
pub fn resolve_and_validate_path(
    root: &Path,
    url_path: &str,
    prefix: &str,
) -> Result<PathBuf, StatusCode> {
    let decoded = percent_decode(url_path);

    let stripped = if !prefix.is_empty() && decoded.starts_with(prefix) {
        &decoded[prefix.len()..]
    } else {
        &decoded
    };

    let cleaned_relative = Path::new(stripped.trim_start_matches('/'));

    // Check every component: no parent `..` or root component allowed
    let mut safe_rel = PathBuf::new();
    for comp in cleaned_relative.components() {
        match comp {
            Component::Normal(part) => {
                let part_str = part.to_string_lossy();
                // Reject null characters or malicious control characters
                if part_str.contains('\0') {
                    return Err(StatusCode::BAD_REQUEST);
                }
                safe_rel.push(part);
            }
            Component::CurDir => continue,
            Component::ParentDir => {
                // Illegal directory traversal
                return Err(StatusCode::FORBIDDEN);
            }
            Component::RootDir | Component::Prefix(_) => {
                // Reject absolute components
                return Err(StatusCode::FORBIDDEN);
            }
        }
    }

    let candidate = root.join(&safe_rel);

    // Canonicalization check with Android fallback:
    if let (Ok(canon_cand), Ok(canon_root)) = (candidate.canonicalize(), root.canonicalize()) {
        if canon_cand.starts_with(&canon_root) {
            return Ok(canon_cand);
        }
        // Special Android check: /sdcard vs /storage/emulated/0 vs /data/media/0
        #[cfg(target_os = "android")]
        {
            let cand_str = canon_cand.to_string_lossy();
            let root_str = canon_root.to_string_lossy();
            if cand_str.contains("/0/") && root_str.contains("/0/") {
                let cand_suffix = cand_str.split_once("/0/").map(|x| x.1).unwrap_or("");
                let root_suffix = root_str.split_once("/0/").map(|x| x.1).unwrap_or("");
                if cand_suffix.starts_with(root_suffix) {
                    return Ok(canon_cand);
                }
            }
        }
        return Err(StatusCode::FORBIDDEN);
    }

    // If candidate does not exist yet (e.g. for upload), ensure candidate lexically starts with root
    if candidate.starts_with(root) {
        Ok(candidate)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Helper parsing HTTP Range header: `bytes=start-end` or `bytes=start-`.
pub fn parse_range_header(header_val: &str, file_size: u64) -> Option<(u64, u64)> {
    let trimmed = header_val.trim();
    if !trimmed.starts_with("bytes=") {
        return None;
    }
    let spec = &trimmed[6..];
    if let Some((start_str, end_str)) = spec.split_once('-') {
        let start_str = start_str.trim();
        let end_str = end_str.trim();

        if start_str.is_empty() {
            // Suffix range: bytes=-500 (last 500 bytes)
            if let Ok(suffix_len) = end_str.parse::<u64>() {
                if suffix_len == 0 || file_size == 0 {
                    return None;
                }
                let start = file_size.saturating_sub(suffix_len);
                return Some((start, file_size - 1));
            }
        } else if let Ok(start) = start_str.parse::<u64>() {
            if start >= file_size {
                return None;
            }
            let end = if end_str.is_empty() {
                file_size - 1
            } else if let Ok(parsed_end) = end_str.parse::<u64>() {
                std::cmp::min(parsed_end, file_size - 1)
            } else {
                return None;
            };

            if start <= end {
                return Some((start, end));
            }
        }
    }
    None
}

/// Checks PIN authentication for the request (via Basic Auth or query param ?pin=).
pub fn check_dufs_auth(req: &Request<Incoming>, config: &DufsConfig) -> bool {
    let expected_pin = match &config.auth_pin {
        Some(pin) if !pin.is_empty() => pin,
        _ => return true, // No PIN required
    };

    // 1. Check query parameter: `?pin=<pin>`
    if let Some(query) = req.uri().query() {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                if k == "pin" && percent_decode(v) == *expected_pin {
                    return true;
                }
            }
        }
    }

    // 2. Check Authorization: Basic <base64>
    if let Some(auth_val) = req.headers().get(hyper::header::AUTHORIZATION)
        && let Ok(auth_str) = auth_val.to_str()
        && auth_str.starts_with("Basic ")
    {
        use base64::Engine;
        let b64 = &auth_str[6..].trim();
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(b64)
            && let Ok(decoded_str) = String::from_utf8(decoded)
        {
            // Allow PIN in either password or username field (in case user filled username prompt)
            let pass = if let Some((user, pass)) = decoded_str.split_once(':') {
                if pass.is_empty() && !user.is_empty() {
                    user
                } else {
                    pass
                }
            } else {
                &decoded_str
            };
            if pass == expected_pin {
                return true;
            }
        }
    }

    false
}

/// Returns a 401 Unauthorized response prompting for PIN.
pub fn unauthorized_response(title: &str) -> Response<ServerResponseBody> {
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            WWW_AUTHENTICATE,
            format!("Basic realm=\"Enter Password (Leave Username Blank / Not Required) - {}\"", title),
        )
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_box_body("401 Unauthorized - Enter password only (no username required)\n"))
        .unwrap()
}

/// Helper converting byte size into human-readable string.
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Escapes HTML special characters.
pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Streams an entire folder as a `.zip` archive on the fly.
///
/// Streams an entire folder as a `.zip` archive on the fly.
///
/// Compresses using a temporary file in temp_dir / cache_dir and streams in 64KB chunks
/// with Content-Length header, automatically cleaning up upon completion.
pub async fn stream_zip_folder(
    dir_path: PathBuf,
    zip_name: String,
) -> Result<Response<ServerResponseBody>, StatusCode> {
    if !dir_path.is_dir() {
        return Err(StatusCode::NOT_FOUND);
    }

    let temp_zip_path = std::env::temp_dir().join(format!(
        "lanflare_zip_{}_{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    let out_file = match std::fs::File::create(&temp_zip_path) {
        Ok(f) => f,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let walk_dir = dir_path.clone();
    let zip_res = tokio::task::spawn_blocking(move || {
        let mut zip = zip::ZipWriter::new(out_file);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        fn add_dir_contents<W: Write + std::io::Seek>(
            zip: &mut zip::ZipWriter<W>,
            dir: &Path,
            base_dir: &Path,
            options: zip::write::SimpleFileOptions,
        ) -> std::io::Result<()> {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let rel_path = match path.strip_prefix(base_dir) {
                    Ok(p) => p.to_string_lossy().replace('\\', "/"),
                    Err(_) => continue,
                };

                if path.is_dir() {
                    zip.add_directory(format!("{rel_path}/"), options)?;
                    add_dir_contents(zip, &path, base_dir, options)?;
                } else if path.is_file() {
                    zip.start_file(rel_path, options)?;
                    let mut file = std::fs::File::open(&path)?;
                    std::io::copy(&mut file, zip)?;
                }
            }
            Ok(())
        }

        let res = add_dir_contents(&mut zip, &walk_dir, &walk_dir, options);
        let _ = zip.finish();
        res
    })
    .await;

    if zip_res.is_err() || zip_res.unwrap().is_err() {
        let _ = tokio::fs::remove_file(&temp_zip_path).await;
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let meta = match tokio::fs::metadata(&temp_zip_path).await {
        Ok(m) => m,
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp_zip_path).await;
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let file_size = meta.len();

    let async_file = match tokio::fs::File::open(&temp_zip_path).await {
        Ok(f) => f,
        Err(_) => {
            let _ = tokio::fs::remove_file(&temp_zip_path).await;
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Auto-remove temp file after streaming finishes or drops
    struct TempCleaner {
        path: PathBuf,
    }
    impl Drop for TempCleaner {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
    let cleaner = Arc::new(TempCleaner {
        path: temp_zip_path,
    });

    let reader_stream = ReaderStream::with_capacity(async_file, 64 * 1024);
    let cleaned_stream = reader_stream.map_ok(move |bytes| {
        let _guard = &cleaner;
        bytes
    });

    let body = stream_box_body(cleaned_stream);

    let clean_zip_name = zip_name.trim_end_matches(".zip");
    let disposition = format!("attachment; filename=\"{clean_zip_name}.zip\"");

    let res = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/zip")
        .header(CONTENT_LENGTH, file_size)
        .header(CONTENT_DISPOSITION, disposition)
        .body(body)
        .unwrap();

    Ok(res)
}

/// Serves a single file with streaming and HTTP Range support.
pub async fn serve_file(
    file_path: &Path,
    req: &Request<Incoming>,
) -> Result<Response<ServerResponseBody>, StatusCode> {
    let meta = match tokio::fs::metadata(file_path).await {
        Ok(m) => m,
        Err(_) => return Err(StatusCode::NOT_FOUND),
    };

    let file_size = meta.len();
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    let mime_type = mime_guess_simple(file_name);

    let mut file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let is_download_attachment = req
        .uri()
        .query()
        .map(|q| q.contains("download"))
        .unwrap_or(false);

    let disposition = if is_download_attachment {
        format!("attachment; filename=\"{}\"", file_name.replace('"', "\\\""))
    } else {
        format!("inline; filename=\"{}\"", file_name.replace('"', "\\\""))
    };

    // Check Range header
    let range_header = req
        .headers()
        .get(RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| parse_range_header(h, file_size));

    if let Some((start, end)) = range_header {
        let length = end - start + 1;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return Err(StatusCode::RANGE_NOT_SATISFIABLE);
        }

        use tokio::io::AsyncReadExt;
        let stream = ReaderStream::with_capacity(file.take(length), 64 * 1024);
        let body = stream_box_body(stream);

        let res = Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, mime_type)
            .header(CONTENT_RANGE, format!("bytes {start}-{end}/{file_size}"))
            .header(CONTENT_LENGTH, length)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_DISPOSITION, disposition)
            .body(body)
            .unwrap();

        Ok(res)
    } else {
        let stream = ReaderStream::with_capacity(file, 64 * 1024);
        let body = stream_box_body(stream);

        let res = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, mime_type)
            .header(CONTENT_LENGTH, file_size)
            .header(ACCEPT_RANGES, "bytes")
            .header(CONTENT_DISPOSITION, disposition)
            .body(body)
            .unwrap();

        Ok(res)
    }
}

/// Generates directory breadcrumb navigation items.
fn build_breadcrumbs(relative_path: &Path, prefix: &str) -> Vec<(String, String)> {
    let mut crumbs = Vec::new();
    let root_url = if prefix.is_empty() {
        "/".to_string()
    } else {
        format!("{prefix}/")
    };
    crumbs.push(("Home".to_string(), root_url));

    let mut current_accum = String::new();
    for comp in relative_path.components() {
        if let Component::Normal(seg) = comp {
            let seg_str = seg.to_string_lossy().to_string();
            current_accum.push('/');
            current_accum.push_str(&seg_str);

            let url = if prefix.is_empty() {
                format!("{current_accum}/")
            } else {
                format!("{prefix}{current_accum}/")
            };
            crumbs.push((seg_str, url));
        }
    }
    crumbs
}

/// Serves a directory listing as HTML or JSON.
pub async fn serve_directory(
    dir_path: &Path,
    root_path: &Path,
    req: &Request<Incoming>,
    config: &DufsConfig,
) -> Result<Response<ServerResponseBody>, StatusCode> {
    // 1. If query is ?zip, stream the directory as a zip
    if let Some(query) = req.uri().query()
        && (query == "zip" || query.starts_with("zip&") || query.ends_with("&zip"))
    {
        let folder_name = dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("folder")
            .to_string();
        return stream_zip_folder(dir_path.to_path_buf(), folder_name).await;
    }

    // 2. Read directory entries
    let mut dir_reader = match tokio::fs::read_dir(dir_path).await {
        Ok(r) => r,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut entries = Vec::new();
    let rel_to_root = dir_path.strip_prefix(root_path).unwrap_or(Path::new(""));

    while let Ok(Some(entry)) = dir_reader.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden files starting with '.'
        if name.starts_with('.') {
            continue;
        }

        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let is_dir = meta.is_dir();
        let size = if is_dir { 0 } else { meta.len() };
        let modified = meta
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH)
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mime = if is_dir {
            "inode/directory".to_string()
        } else {
            mime_guess_simple(&name)
        };

        let sub_rel = if rel_to_root.as_os_str().is_empty() {
            name.clone()
        } else {
            format!("{}/{}", rel_to_root.to_string_lossy(), name)
        };

        let url_path = if config.path_prefix.is_empty() {
            format!("/{sub_rel}")
        } else {
            format!("{}{}/{sub_rel}", config.path_prefix, "")
        };

        entries.push(DufsEntryDto {
            name,
            path: url_path,
            is_dir,
            size,
            modified,
            mime_type: mime,
        });
    }

    // Sort: directories first, then alphabetical by name
    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    // 3. Return JSON if requested
    let accepts_json = req
        .headers()
        .get(hyper::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("application/json"))
        .unwrap_or(false)
        || req.uri().query().map(|q| q.contains("json")).unwrap_or(false);

    if accepts_json {
        return Ok(json_response(StatusCode::OK, &entries));
    }

    // 4. Render HTML page
    let breadcrumbs = build_breadcrumbs(rel_to_root, &config.path_prefix);

    let mut breadcrumbs_html = String::new();
    for (i, (crumb_name, crumb_url)) in breadcrumbs.iter().enumerate() {
        if i > 0 {
            breadcrumbs_html.push_str(r#"<span class="crumb-sep">/</span>"#);
        }
        let encoded_name = escape_html(crumb_name);
        breadcrumbs_html.push_str(&format!(
            r#"<a class="crumb-link" href="{crumb_url}">{encoded_name}</a>"#
        ));
    }

    let mut parent_row_html = String::new();
    if rel_to_root.components().count() > 0 {
        let parent_url = if let Some(parent) = rel_to_root.parent() {
            if parent.as_os_str().is_empty() {
                if config.path_prefix.is_empty() {
                    "/".to_string()
                } else {
                    format!("{}/", config.path_prefix)
                }
            } else {
                format!("{}/{}/", config.path_prefix, parent.to_string_lossy())
            }
        } else {
            "/".to_string()
        };

        parent_row_html = format!(
            r#"<tr class="entry-row parent-row">
                <td class="col-icon">
                    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M19 12H5M12 19l-7-7 7-7"/></svg>
                </td>
                <td class="col-name"><a href="{parent_url}" class="name-link">..</a></td>
                <td class="col-size">-</td>
                <td class="col-date">-</td>
                <td class="col-actions"></td>
            </tr>"#
        );
    }

    let mut rows_html = String::new();
    for entry in &entries {
        let encoded_name = escape_html(&entry.name);
        let href = if entry.is_dir {
            format!("{}/", entry.path)
        } else {
            entry.path.clone()
        };

        let size_str = if entry.is_dir {
            "-".to_string()
        } else {
            format_size(entry.size)
        };

        let date_str = {
            let dt = chrono::DateTime::from_timestamp(entry.modified as i64, 0)
                .unwrap_or_default();
            dt.format("%Y-%m-%d %H:%M").to_string()
        };

        let icon_svg = if entry.is_dir {
            r#"<svg class="icon-folder" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>"#
        } else if entry.mime_type.starts_with("image/") {
            r#"<svg class="icon-image" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="3" width="18" height="18" rx="2"/><circle cx="8.5" cy="8.5" r="1.5"/><polyline points="21 15 16 10 5 21"/></svg>"#
        } else if entry.mime_type.starts_with("video/") {
            r#"<svg class="icon-video" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="23 7 16 12 23 17 23 7"/><rect x="1" y="5" width="15" height="14" rx="2" ry="2"/></svg>"#
        } else if entry.mime_type.starts_with("audio/") {
            r#"<svg class="icon-audio" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>"#
        } else if entry.mime_type == "application/pdf" {
            r#"<svg class="icon-pdf" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><polyline points="14 2 14 8 20 8"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><polyline points="10 9 9 9 8 9"/></svg>"#
        } else if entry.mime_type == "application/zip" || entry.mime_type.contains("zip") || entry.mime_type.contains("tar") {
            r#"<svg class="icon-zip" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/><polyline points="3.27 6.96 12 12.01 20.73 6.96"/><line x1="12" y1="22.08" x2="12" y2="12"/></svg>"#
        } else {
            r#"<svg class="icon-file" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z"/><polyline points="14 2 14 8 20 8"/></svg>"#
        };

        let action_btn = if entry.is_dir {
            format!(
                r#"<a href="{href}?zip" class="action-btn" title="Download folder as ZIP">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
                    ZIP
                </a>"#
            )
        } else {
            format!(
                r#"<a href="{href}?download" download="{encoded_name}" class="action-btn" title="Download file">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
                    Download
                </a>"#
            )
        };

        rows_html.push_str(&format!(
            r#"<tr class="entry-row" data-name="{encoded_name}" data-isdir="{}">
                <td class="col-icon">{icon_svg}</td>
                <td class="col-name"><a href="{href}" class="name-link">{encoded_name}</a></td>
                <td class="col-size" data-bytes="{}">{size_str}</td>
                <td class="col-date" data-time="{}">{date_str}</td>
                <td class="col-actions">{action_btn}</td>
            </tr>"#,
            if entry.is_dir { "true" } else { "false" },
            entry.size,
            entry.modified
        ));
    }

    let upload_section_html = if config.allow_upload {
        r#"<div class="upload-bar" id="uploadBar">
            <input type="file" id="fileInput" multiple style="display:none;" />
            <button class="btn btn-primary" onclick="document.getElementById('fileInput').click()">
                <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="17 8 12 3 7 8"/><line x1="12" y1="3" x2="12" y2="15"/></svg>
                Upload Files
            </button>
            <span class="upload-tip">or drag & drop files anywhere</span>
        </div>"#
    } else {
        ""
    };

    let title_escaped = escape_html(&config.title);

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=5.0">
    <title>{title_escaped}</title>
    <style>
        :root {{
            --bg-body: #0b0f19;
            --bg-card: #111827;
            --bg-row-hover: #1f293d;
            --border: #1e293b;
            --text-main: #f8fafc;
            --text-muted: #94a3b8;
            --accent: #6366f1;
            --accent-hover: #4f46e5;
            --folder-color: #fbbf24;
            --file-color: #818cf8;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
            background: var(--bg-body);
            color: var(--text-main);
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            padding: 16px;
        }}
        .container {{
            max-width: 960px;
            width: 100%;
            margin: 0 auto;
            display: flex;
            flex-direction: column;
            gap: 16px;
        }}
        .header {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            flex-wrap: wrap;
            gap: 12px;
            padding: 16px 20px;
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 16px;
            box-shadow: 0 4px 20px rgba(0,0,0,0.3);
        }}
        .brand {{
            display: flex;
            align-items: center;
            gap: 12px;
        }}
        .brand-logo {{
            width: 36px;
            height: 36px;
            border-radius: 10px;
            background: linear-gradient(135deg, #6366f1, #8b5cf6);
            display: flex;
            align-items: center;
            justify-content: center;
            color: white;
            box-shadow: 0 2px 10px rgba(99,102,241,0.4);
        }}
        .brand-title {{
            font-size: 17px;
            font-weight: 700;
            color: var(--text-main);
        }}
        .brand-badge {{
            font-size: 10px;
            font-weight: 600;
            padding: 2px 6px;
            border-radius: 6px;
            background: rgba(99, 102, 241, 0.2);
            color: #818cf8;
            border: 1px solid rgba(99, 102, 241, 0.4);
            margin-left: 6px;
        }}
        .top-actions {{
            display: flex;
            align-items: center;
            gap: 10px;
            flex-wrap: wrap;
        }}
        .search-box {{
            position: relative;
            display: flex;
            align-items: center;
        }}
        .search-box input {{
            background: #0f172a;
            border: 1px solid var(--border);
            color: var(--text-main);
            padding: 8px 12px 8px 34px;
            border-radius: 10px;
            font-size: 13px;
            outline: none;
            width: 180px;
            transition: all 0.2s;
        }}
        .search-box input:focus {{
            border-color: var(--accent);
            width: 220px;
            box-shadow: 0 0 0 2px rgba(99,102,241,0.25);
        }}
        .search-icon {{
            position: absolute;
            left: 10px;
            color: var(--text-muted);
            pointer-events: none;
        }}
        .btn {{
            display: inline-flex;
            align-items: center;
            gap: 6px;
            padding: 8px 14px;
            border-radius: 10px;
            font-size: 12px;
            font-weight: 600;
            cursor: pointer;
            border: none;
            text-decoration: none;
            transition: all 0.15s;
        }}
        .btn-primary {{
            background: var(--accent);
            color: white;
        }}
        .btn-primary:hover {{ background: var(--accent-hover); }}
        .btn-outline {{
            background: transparent;
            border: 1px solid var(--border);
            color: var(--text-main);
        }}
        .btn-outline:hover {{
            background: #1e293b;
            border-color: #475569;
        }}
        .breadcrumbs-bar {{
            display: flex;
            align-items: center;
            gap: 6px;
            padding: 10px 16px;
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 12px;
            font-size: 13px;
            overflow-x: auto;
            white-space: nowrap;
        }}
        .crumb-link {{
            color: #818cf8;
            text-decoration: none;
            font-weight: 500;
        }}
        .crumb-link:hover {{ text-decoration: underline; }}
        .crumb-sep {{ color: var(--text-muted); opacity: 0.6; }}
        .upload-bar {{
            display: flex;
            align-items: center;
            gap: 12px;
            padding: 12px 18px;
            background: rgba(99,102,241,0.06);
            border: 1px dashed rgba(99,102,241,0.4);
            border-radius: 12px;
        }}
        .upload-tip {{ font-size: 12px; color: var(--text-muted); }}
        .table-card {{
            background: var(--bg-card);
            border: 1px solid var(--border);
            border-radius: 16px;
            overflow: hidden;
            box-shadow: 0 4px 20px rgba(0,0,0,0.2);
        }}
        table {{
            width: 100%;
            border-collapse: collapse;
            text-align: left;
        }}
        th {{
            padding: 12px 16px;
            font-size: 11px;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            color: var(--text-muted);
            border-bottom: 1px solid var(--border);
            font-weight: 600;
            background: #0d121f;
        }}
        td {{
            padding: 12px 16px;
            font-size: 13px;
            border-bottom: 1px solid rgba(255,255,255,0.04);
            vertical-align: middle;
        }}
        .entry-row:hover td {{
            background: var(--bg-row-hover);
        }}
        .col-icon {{ width: 36px; padding-right: 0; }}
        .icon-folder {{ color: var(--folder-color); }}
        .icon-image {{ color: #38bdf8; }}
        .icon-video {{ color: #f43f5e; }}
        .icon-audio {{ color: #a855f7; }}
        .icon-pdf {{ color: #ef4444; }}
        .icon-zip {{ color: #eab308; }}
        .icon-file {{ color: #94a3b8; }}
        .col-name {{ font-weight: 500; min-width: 180px; }}
        .name-link {{
            color: var(--text-main);
            text-decoration: none;
            word-break: break-all;
        }}
        .name-link:hover {{ color: #818cf8; text-decoration: underline; }}
        .col-size {{ width: 110px; font-family: monospace; font-size: 12px; color: var(--text-muted); }}
        .col-date {{ width: 150px; font-size: 12px; color: var(--text-muted); }}
        .col-actions {{ width: 100px; text-align: right; }}
        .action-btn {{
            display: inline-flex;
            align-items: center;
            gap: 4px;
            padding: 5px 10px;
            border-radius: 8px;
            background: #1e293b;
            border: 1px solid #334155;
            color: #cbd5e1;
            font-size: 11px;
            font-weight: 600;
            text-decoration: none;
            transition: all 0.15s;
        }}
        .action-btn:hover {{
            background: var(--accent);
            border-color: var(--accent);
            color: white;
        }}
        .footer {{
            text-align: center;
            font-size: 11px;
            color: var(--text-muted);
            margin-top: 10px;
            padding-bottom: 20px;
        }}
        @media (max-width: 640px) {{
            body {{ padding: 10px; }}
            .col-date {{ display: none; }}
            .col-size {{ width: 80px; }}
            .search-box input {{ width: 140px; }}
            .search-box input:focus {{ width: 170px; }}
        }}
    </style>
</head>
<body>
    <div class="container">
        <!-- Header -->
        <header class="header">
            <div class="brand">
                <div class="brand-logo">
                    <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><path d="M8.5 14.5A2.5 2.5 0 0 0 11 12c0-1.38-.5-2-1-3-1.072-2.143-.224-4.054 2-6 .5 2.5 2 4.9 4 6.5 2 1.6 3 3.5 3 5.5a7 7 0 1 1-14 0c0-1.153.433-2.294 1-3a2.5 2.5 0 0 0 2.5 2.5z"/></svg>
                </div>
                <div>
                    <span class="brand-title">{title_escaped}</span>
                    <span class="brand-badge">Dufs Mode</span>
                </div>
            </div>
            <div class="top-actions">
                <div class="search-box">
                    <svg class="search-icon" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/></svg>
                    <input type="text" id="searchInput" placeholder="Filter files..." oninput="filterFiles()" />
                </div>
                <a href="?zip" class="btn btn-outline" title="Download entire folder as ZIP">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
                    Download ZIP
                </a>
            </div>
        </header>

        <!-- Breadcrumbs -->
        <nav class="breadcrumbs-bar">
            {breadcrumbs_html}
        </nav>

        {upload_section_html}

        <!-- Table Card -->
        <main class="table-card">
            <table>
                <thead>
                    <tr>
                        <th class="col-icon"></th>
                        <th class="col-name" onclick="sortTable('name')" style="cursor:pointer;">Name &uarr;&darr;</th>
                        <th class="col-size" onclick="sortTable('size')" style="cursor:pointer;">Size &uarr;&darr;</th>
                        <th class="col-date" onclick="sortTable('date')" style="cursor:pointer;">Date Modified &uarr;&darr;</th>
                        <th class="col-actions">Actions</th>
                    </tr>
                </thead>
                <tbody id="entriesBody">
                    {parent_row_html}
                    {rows_html}
                </tbody>
            </table>
        </main>

        <footer class="footer">
            Powered by LanFlare &bull; Dufs-compatible High Speed Local LAN File Server
        </footer>
    </div>

    <script>
        function filterFiles() {{
            const val = document.getElementById('searchInput').value.toLowerCase().trim();
            const rows = document.querySelectorAll('.entry-row:not(.parent-row)');
            rows.forEach(r => {{
                const name = (r.getAttribute('data-name') || '').toLowerCase();
                r.style.display = name.includes(val) ? '' : 'none';
            }});
        }}

        let sortDirections = {{}};
        function sortTable(key) {{
            const tbody = document.getElementById('entriesBody');
            const parentRow = tbody.querySelector('.parent-row');
            const rows = Array.from(tbody.querySelectorAll('.entry-row:not(.parent-row)'));
            const asc = !sortDirections[key];
            sortDirections[key] = asc;

            rows.sort((a, b) => {{
                const isDirA = a.getAttribute('data-isdir') === 'true';
                const isDirB = b.getAttribute('data-isdir') === 'true';
                if (isDirA !== isDirB) return isDirA ? -1 : 1;

                if (key === 'name') {{
                    const nameA = a.getAttribute('data-name').toLowerCase();
                    const nameB = b.getAttribute('data-name').toLowerCase();
                    return asc ? nameA.localeCompare(nameB) : nameB.localeCompare(nameA);
                }} else if (key === 'size') {{
                    const sizeA = parseInt(a.querySelector('.col-size').getAttribute('data-bytes') || '0', 10);
                    const sizeB = parseInt(b.querySelector('.col-size').getAttribute('data-bytes') || '0', 10);
                    return asc ? sizeA - sizeB : sizeB - sizeA;
                }} else if (key === 'date') {{
                    const timeA = parseInt(a.querySelector('.col-date').getAttribute('data-time') || '0', 10);
                    const timeB = parseInt(b.querySelector('.col-date').getAttribute('data-time') || '0', 10);
                    return asc ? timeA - timeB : timeB - timeA;
                }}
                return 0;
            }});

            rows.forEach(r => tbody.appendChild(r));
        }}

        // Upload handler if enabled
        const fileInput = document.getElementById('fileInput');
        if (fileInput) {{
            fileInput.addEventListener('change', async () => {{
                const files = fileInput.files;
                if (!files || files.length === 0) return;
                for (const f of files) {{
                    const form = new FormData();
                    form.append('file', f);
                    try {{
                        const uploadUrl = window.location.pathname + (window.location.pathname.endsWith('/') ? '' : '/') + encodeURIComponent(f.name) + '?upload';
                        await fetch(uploadUrl, {{ method: 'PUT', body: f }});
                    }} catch (e) {{
                        console.error('Upload failed:', e);
                    }}
                }}
                window.location.reload();
            }});

            // Drag and drop support
            window.addEventListener('dragover', e => e.preventDefault());
            window.addEventListener('drop', async e => {{
                e.preventDefault();
                const files = e.dataTransfer.files;
                if (!files || files.length === 0) return;
                for (const f of files) {{
                    try {{
                        const uploadUrl = window.location.pathname + (window.location.pathname.endsWith('/') ? '' : '/') + encodeURIComponent(f.name) + '?upload';
                        await fetch(uploadUrl, {{ method: 'PUT', body: f }});
                    }} catch (err) {{
                        console.error('Drop upload failed:', err);
                    }}
                }}
                window.location.reload();
            }});
        }}
    </script>
</body>
</html>"#
    );

    Ok(html_response(StatusCode::OK, html))
}

/// Handles uploading a file into the target directory.
pub async fn handle_upload_file(
    file_path: &Path,
    mut req: Request<Incoming>,
) -> Result<Response<ServerResponseBody>, StatusCode> {
    use http_body_util::BodyExt;
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = file_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let mut out_file = match tokio::fs::File::create(file_path).await {
        Ok(f) => f,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let body = req.body_mut();
    while let Some(frame_res) = body.frame().await {
        let frame = match frame_res {
            Ok(f) => f,
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        };
        if let Ok(data) = frame.into_data() {
            if out_file.write_all(&data).await.is_err() {
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    let _ = out_file.flush().await;
    Ok(Response::builder()
        .status(StatusCode::CREATED)
        .header(CONTENT_TYPE, "text/plain")
        .body(full_box_body("File uploaded successfully\n"))
        .unwrap())
}

/// Handles deleting a file or directory.
pub async fn handle_delete_file(
    path: &Path,
) -> Result<Response<ServerResponseBody>, StatusCode> {
    if !path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    if path.is_dir() {
        if tokio::fs::remove_dir_all(path).await.is_ok() {
            Ok(empty_box_response(StatusCode::NO_CONTENT))
        } else {
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    } else {
        if tokio::fs::remove_file(path).await.is_ok() {
            Ok(empty_box_response(StatusCode::NO_CONTENT))
        } else {
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Dispatches an incoming HTTP request for the Dufs file server.
pub async fn handle_dufs_request(
    req: Request<Incoming>,
    _remote_ip: IpAddr,
    config: &DufsConfig,
) -> Result<Response<ServerResponseBody>, hyper::Error> {
    if !check_dufs_auth(&req, config) {
        return Ok(unauthorized_response(&config.title));
    }

    let method = req.method();
    let uri_path = req.uri().path();

    // Check if query is upload
    let is_upload = req
        .uri()
        .query()
        .map(|q| q.contains("upload"))
        .unwrap_or(false);

    // Resolve target path safely
    let target_path = match resolve_and_validate_path(
        &config.root_path,
        uri_path,
        &config.path_prefix,
    ) {
        Ok(p) => p,
        Err(status) => return Ok(empty_box_response(status)),
    };

    let response = match (method, is_upload) {
        // Upload file: PUT or POST with ?upload
        (&Method::PUT, _) | (&Method::POST, true) if config.allow_upload => {
            match handle_upload_file(&target_path, req).await {
                Ok(res) => res,
                Err(status) => empty_box_response(status),
            }
        }

        // Delete file or folder
        (&Method::DELETE, _) if config.allow_delete => {
            match handle_delete_file(&target_path).await {
                Ok(res) => res,
                Err(status) => empty_box_response(status),
            }
        }

        // GET or HEAD file or directory
        (&Method::GET, _) | (&Method::HEAD, _) => {
            if target_path.is_dir() {
                // Ensure directory URL ends with '/' for clean relative links
                if !uri_path.ends_with('/') {
                    let new_path = if let Some(q) = req.uri().query() {
                        format!("{uri_path}/?{q}")
                    } else {
                        format!("{uri_path}/")
                    };
                    return Ok(Response::builder()
                        .status(StatusCode::FOUND)
                        .header(hyper::header::LOCATION, new_path)
                        .body(full_box_body(Bytes::new()))
                        .unwrap());
                }

                match serve_directory(&target_path, &config.root_path, &req, config).await {
                    Ok(res) => res,
                    Err(status) => empty_box_response(status),
                }
            } else if target_path.is_file() {
                match serve_file(&target_path, &req).await {
                    Ok(res) => res,
                    Err(status) => empty_box_response(status),
                }
            } else {
                empty_box_response(StatusCode::NOT_FOUND)
            }
        }

        _ => empty_box_response(StatusCode::METHOD_NOT_ALLOWED),
    };

    Ok(response)
}

/// Starts an independent standalone HTTP server for dufs on the given port.
pub async fn start_standalone_dufs_server(
    config: DufsConfig,
    port: u16,
) -> anyhow::Result<DufsServerHandle> {
    let cancel_token = CancellationToken::new();
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;
    let bound_port = listener.local_addr()?.port();

    let config_arc = Arc::new(config);
    let cancel = cancel_token.clone();

    let join_handle = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                accept_res = listener.accept() => {
                    let (stream, remote_addr) = match accept_res {
                        Ok(res) => res,
                        Err(_) => continue,
                    };
                    let cfg = config_arc.clone();
                    let io = hyper_util::rt::TokioIo::new(stream);

                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| {
                            let cfg = cfg.clone();
                            async move {
                                handle_dufs_request(req, remote_addr.ip(), &cfg).await
                            }
                        });

                        let _ = hyper::server::conn::http1::Builder::new()
                            .max_buf_size(128 * 1024)
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }
        }
    });

    Ok(DufsServerHandle {
        port: bound_port,
        cancel_token,
        join_handle,
    })
}
