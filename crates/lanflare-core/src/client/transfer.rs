//! Client transfer orchestrator.
//!
//! Coordinates file upload sessions to remote LocalSend / LanFlare peers, including:
//! - Pre-flight calculation of file sizes, MIME types, and optional SHA-256 hashes.
//! - Negotiation with the receiver via `POST /api/localsend/v2/prepare-upload`.
//! - Streaming parallel uploads of accepted files with configurable concurrency bounds.
//! - Progress reporting over Tokio channels (bytes transferred, speed, ETA, status).
//! - Graceful cancellation via `POST /api/localsend/v2/cancel` and cancellation tokens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::Stream;
use reqwest::Body;
use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::io::ReaderStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use lanflare_proto::discovery::RegisterDtoV2;
use lanflare_proto::transfer::{FileDto, FileMetadataDto, PrepareUploadRequestDtoV2};

use crate::client::{ClientError, LsClient};

/// Default chunk size used when streaming files from disk to the network (128 KB).
pub const DEFAULT_STREAM_CHUNK_SIZE: usize = 128 * 1024;

/// Specification for a file to be sent to a remote peer.
#[derive(Clone, Debug)]
pub struct OutgoingFile {
    /// Unique client-side ID for this file in the session.
    pub id: String,
    /// Absolute path to the file on local disk.
    pub path: PathBuf,
    /// Name to advertise to the remote receiver (defaults to file basename).
    pub file_name: String,
    /// Size of the file in bytes.
    pub size: u64,
    /// MIME type string (e.g. `application/octet-stream`).
    pub file_type: String,
    /// Precomputed or lazily computed SHA-256 hash (hex string).
    pub sha256: Option<String>,
}

impl OutgoingFile {
    /// Creates an `OutgoingFile` from a local file path.
    ///
    /// Reads file metadata to obtain size. Basename is derived automatically.
    pub async fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ClientError> {
        let p = path.as_ref().to_path_buf();
        let meta = tokio::fs::metadata(&p).await?;

        let file_name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file.bin")
            .to_string();

        let file_type = mime_guess_simple(&file_name);

        Ok(Self {
            id: Uuid::new_v4().to_string(),
            path: p,
            file_name,
            size: meta.len(),
            file_type,
            sha256: None,
        })
    }

    /// Creates an `OutgoingFile` from a local file path with custom relative advertised file_name.
    pub async fn from_path_with_name<P: AsRef<Path>>(
        path: P,
        file_name: String,
    ) -> Result<Self, ClientError> {
        let p = path.as_ref().to_path_buf();
        let meta = tokio::fs::metadata(&p).await?;
        let file_type = mime_guess_simple(&file_name);

        Ok(Self {
            id: Uuid::new_v4().to_string(),
            path: p,
            file_name,
            size: meta.len(),
            file_type,
            sha256: None,
        })
    }

    /// Computes and populates the SHA-256 hash of the local file.
    pub async fn with_computed_sha256(mut self) -> Result<Self, ClientError> {
        let mut file = File::open(&self.path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; DEFAULT_STREAM_CHUNK_SIZE];

        loop {
            let n = file.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        let hash_bytes = hasher.finalize();
        self.sha256 = Some(format!("{:x}", hash_bytes));
        Ok(self)
    }

    /// Converts this file into a protocol [`FileDto`].
    pub fn to_dto(&self) -> FileDto {
        FileDto {
            id: self.id.clone(),
            file_name: self.file_name.clone(),
            size: self.size,
            file_type: self.file_type.clone(),
            sha256: self.sha256.clone(),
            preview: None,
            metadata: Some(FileMetadataDto {
                modified: None,
                accessed: None,
            }),
        }
    }
}

/// Simple MIME classifier without external dependencies.
pub fn mime_guess_simple(file_name: &str) -> String {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png".to_string()
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg".to_string()
    } else if lower.ends_with(".gif") {
        "image/gif".to_string()
    } else if lower.ends_with(".webp") {
        "image/webp".to_string()
    } else if lower.ends_with(".mp4") {
        "video/mp4".to_string()
    } else if lower.ends_with(".mkv") {
        "video/x-matroska".to_string()
    } else if lower.ends_with(".mp3") {
        "audio/mpeg".to_string()
    } else if lower.ends_with(".pdf") {
        "application/pdf".to_string()
    } else if lower.ends_with(".zip") {
        "application/zip".to_string()
    } else if lower.ends_with(".tar") || lower.ends_with(".gz") {
        "application/gzip".to_string()
    } else if lower.ends_with(".txt") {
        "text/plain".to_string()
    } else if lower.ends_with(".json") {
        "application/json".to_string()
    } else {
        "application/octet-stream".to_string()
    }
}

/// Fine-grained transfer progress updates sent during session execution.
#[derive(Clone, Debug)]
pub enum TransferProgress {
    /// Negotiation succeeded and transfer started.
    Started {
        session_id: String,
        accepted_files: Vec<String>,
        total_bytes: u64,
    },
    /// Progress update during upload.
    FileProgress {
        session_id: String,
        file_id: String,
        file_name: String,
        file_bytes_sent: u64,
        file_total_bytes: u64,
        session_bytes_sent: u64,
        session_total_bytes: u64,
        /// Transfer speed in bytes per second.
        bytes_per_second: u64,
        /// Estimated remaining duration until completion.
        eta: Option<Duration>,
    },
    /// A single file finished uploading and was accepted by the receiver.
    FileFinished {
        session_id: String,
        file_id: String,
        file_name: String,
    },
    /// A single file upload encountered an error.
    FileFailed {
        session_id: String,
        file_id: String,
        file_name: String,
        error: String,
    },
    /// All accepted files transferred successfully.
    SessionCompleted {
        session_id: String,
        total_files: usize,
        total_bytes: u64,
        duration: Duration,
    },
    /// The transfer session was cancelled or aborted.
    SessionCancelled {
        session_id: String,
        reason: String,
    },
}

/// Options configuring the transfer orchestrator.
#[derive(Clone, Debug)]
pub struct TransferOptions {
    /// Maximum number of files to upload in parallel (default: 4).
    pub max_concurrency: usize,
    /// Chunk size in bytes when streaming from disk to network (default: 128 KB).
    pub chunk_size: usize,
    /// Optional PIN code for recipient authentication.
    pub pin: Option<String>,
    /// Optional cancellation token.
    pub cancel_token: Option<CancellationToken>,
    /// Interval between progress emissions per file (default: 100ms).
    pub progress_interval: Duration,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            max_concurrency: 4,
            chunk_size: DEFAULT_STREAM_CHUNK_SIZE,
            pin: None,
            cancel_token: None,
            progress_interval: Duration::from_millis(100),
        }
    }
}

/// Orchestrates an end-to-end file transfer session to a remote peer.
pub struct TransferOrchestrator {
    client: LsClient,
    sender_info: RegisterDtoV2,
}

impl TransferOrchestrator {
    /// Creates a new transfer orchestrator.
    pub fn new(client: LsClient, sender_info: RegisterDtoV2) -> Self {
        Self {
            client,
            sender_info,
        }
    }

    /// Executes the transfer session to `base_url` for the provided files.
    ///
    /// Emits progress events over `progress_tx` if provided.
    pub async fn transfer(
        &self,
        base_url: &str,
        files: Vec<OutgoingFile>,
        options: TransferOptions,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<(), ClientError> {
        if files.is_empty() {
            return Ok(());
        }

        let cancel_token = options
            .cancel_token
            .unwrap_or_default();

        // 1. Prepare request DTO
        let mut dler_files = HashMap::new();
        let mut files_by_id = HashMap::new();

        for file in files {
            dler_files.insert(file.id.clone(), file.to_dto());
            files_by_id.insert(file.id.clone(), file);
        }

        let prep_req = PrepareUploadRequestDtoV2 {
            info: self.sender_info.clone(),
            files: dler_files,
        };

        // 2. Negotiate prepare-upload
        tokio::select! {
            _ = cancel_token.cancelled() => {
                Err(ClientError::SessionRejected)
            }
            res = self.client.prepare_upload(base_url, &prep_req, options.pin.as_deref()) => {
                let prep_resp = res?;
                self.run_upload_loop(
                    base_url,
                    prep_resp.session_id,
                    prep_resp.files,
                    files_by_id,
                    options.max_concurrency,
                    options.progress_interval,
                    options.chunk_size,
                    cancel_token,
                    progress_tx,
                ).await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_upload_loop(
        &self,
        base_url: &str,
        session_id: String,
        accepted_tokens: HashMap<String, String>,
        mut files_by_id: HashMap<String, OutgoingFile>,
        concurrency: usize,
        progress_interval: Duration,
        chunk_size: usize,
        cancel_token: CancellationToken,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<(), ClientError> {
        let start_time = Instant::now();

        // Calculate total accepted bytes
        let accepted_file_ids: Vec<String> = accepted_tokens.keys().cloned().collect();
        let mut session_total_bytes: u64 = 0;
        for file_id in &accepted_file_ids {
            if let Some(f) = files_by_id.get(file_id) {
                session_total_bytes += f.size;
            }
        }

        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(TransferProgress::Started {
                    session_id: session_id.clone(),
                    accepted_files: accepted_file_ids.clone(),
                    total_bytes: session_total_bytes,
                })
                .await;
        }

        let session_bytes_counter = Arc::new(AtomicU64::new(0));
        let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
        let mut join_set = tokio::task::JoinSet::new();

        for (file_id, token) in accepted_tokens {
            let Some(file) = files_by_id.remove(&file_id) else {
                continue;
            };

            let client = self.client.clone();
            let base_url = base_url.to_string();
            let session_id = session_id.clone();
            let sem = Arc::clone(&semaphore);
            let cancel = cancel_token.clone();
            let tx = progress_tx.clone();
            let session_bytes = Arc::clone(&session_bytes_counter);

            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                if cancel.is_cancelled() {
                    return Err(ClientError::SessionRejected);
                }

                upload_single_file(
                    client,
                    base_url,
                    session_id,
                    file,
                    token,
                    session_bytes,
                    session_total_bytes,
                    start_time,
                    progress_interval,
                    chunk_size,
                    cancel,
                    tx,
                )
                .await
            });
        }

        let mut first_error: Option<ClientError> = None;

        while let Some(res) = join_set.join_next().await {
            match res {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    if first_error.is_none() {
                        first_error = Some(err);
                        // Cancel other ongoing uploads on first error
                        cancel_token.cancel();
                    }
                }
                Err(join_err) => {
                    if first_error.is_none() {
                        first_error = Some(ClientError::Tls(format!("Task join error: {join_err}")));
                        cancel_token.cancel();
                    }
                }
            }
        }

        if let Some(err) = first_error {
            // Notify peer of cancellation
            let _ = self.client.cancel_session(base_url, &session_id).await;

            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(TransferProgress::SessionCancelled {
                        session_id: session_id.clone(),
                        reason: err.to_string(),
                    })
                    .await;
            }
            return Err(err);
        }

        if cancel_token.is_cancelled() {
            let _ = self.client.cancel_session(base_url, &session_id).await;
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(TransferProgress::SessionCancelled {
                        session_id: session_id.clone(),
                        reason: "Cancelled by user".to_string(),
                    })
                    .await;
            }
            return Err(ClientError::SessionRejected);
        }

        let duration = start_time.elapsed();
        if let Some(ref tx) = progress_tx {
            let _ = tx
                .send(TransferProgress::SessionCompleted {
                    session_id,
                    total_files: accepted_file_ids.len(),
                    total_bytes: session_total_bytes,
                    duration,
                })
                .await;
        }

        Ok(())
    }
}

/// Streams a single file to the receiver, calculating speed and dispatching progress events.
#[allow(clippy::too_many_arguments)]
async fn upload_single_file(
    client: LsClient,
    base_url: String,
    session_id: String,
    file: OutgoingFile,
    token: String,
    session_bytes_counter: Arc<AtomicU64>,
    session_total_bytes: u64,
    session_start_time: Instant,
    progress_interval: Duration,
    chunk_size: usize,
    cancel_token: CancellationToken,
    progress_tx: Option<mpsc::Sender<TransferProgress>>,
) -> Result<(), ClientError> {
    let file_id = file.id.clone();
    let file_name = file.file_name.clone();
    let file_size = file.size;

    let fs_file = File::open(&file.path).await.map_err(ClientError::Io)?;
    let reader_stream = ReaderStream::with_capacity(fs_file, chunk_size);

    let progress_stream = ProgressStream::new(
        reader_stream,
        session_id.clone(),
        file_id.clone(),
        file_name.clone(),
        file_size,
        session_bytes_counter,
        session_total_bytes,
        session_start_time,
        progress_interval,
        cancel_token.clone(),
        progress_tx.clone(),
    );

    let body = Body::wrap_stream(progress_stream);

    let upload_res = client
        .upload_file(
            &base_url,
            &session_id,
            &file_id,
            &token,
            body,
            file_size,
        )
        .await;

    match upload_res {
        Ok(()) => {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(TransferProgress::FileFinished {
                        session_id,
                        file_id,
                        file_name,
                    })
                    .await;
            }
            Ok(())
        }
        Err(err) => {
            if let Some(ref tx) = progress_tx {
                let _ = tx
                    .send(TransferProgress::FileFailed {
                        session_id,
                        file_id,
                        file_name,
                        error: err.to_string(),
                    })
                    .await;
            }
            Err(err)
        }
    }
}

/// A wrapper stream that counts uploaded bytes and emits progress reports at fixed intervals.
struct ProgressStream<S> {
    inner: S,
    session_id: String,
    file_id: String,
    file_name: String,
    file_total: u64,
    file_sent: u64,
    session_bytes: Arc<AtomicU64>,
    session_total: u64,
    session_start: Instant,
    last_progress_emit: Instant,
    progress_interval: Duration,
    cancel: CancellationToken,
    progress_tx: Option<mpsc::Sender<TransferProgress>>,
}

impl<S> ProgressStream<S> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        inner: S,
        session_id: String,
        file_id: String,
        file_name: String,
        file_total: u64,
        session_bytes: Arc<AtomicU64>,
        session_total: u64,
        session_start: Instant,
        progress_interval: Duration,
        cancel: CancellationToken,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Self {
        Self {
            inner,
            session_id,
            file_id,
            file_name,
            file_total,
            file_sent: 0,
            session_bytes,
            session_total,
            session_start,
            last_progress_emit: Instant::now(),
            progress_interval,
            cancel,
            progress_tx,
        }
    }
}

impl<S, B, E> Stream for ProgressStream<S>
where
    S: Stream<Item = Result<B, E>> + Unpin,
    B: bytes::Buf,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<B, E>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.cancel.is_cancelled() {
            return std::task::Poll::Ready(None);
        }

        let this = &mut *self;
        match std::pin::Pin::new(&mut this.inner).poll_next(cx) {
            std::task::Poll::Ready(Some(Ok(chunk))) => {
                let chunk_len = chunk.remaining() as u64;
                this.file_sent += chunk_len;
                let cur_session_sent =
                    this.session_bytes.fetch_add(chunk_len, Ordering::Relaxed) + chunk_len;

                let now = Instant::now();
                if now.duration_since(this.last_progress_emit) >= this.progress_interval {
                    this.last_progress_emit = now;

                    let elapsed = now.duration_since(this.session_start).as_secs_f64();
                    let bytes_per_second = if elapsed > 0.0 {
                        (cur_session_sent as f64 / elapsed) as u64
                    } else {
                        0
                    };

                    let eta = if bytes_per_second > 0 && cur_session_sent < this.session_total {
                        let remaining_bytes = this.session_total - cur_session_sent;
                        Some(Duration::from_secs_f64(
                            remaining_bytes as f64 / bytes_per_second as f64,
                        ))
                    } else {
                        None
                    };

                    if let Some(ref tx) = this.progress_tx {
                        let _ = tx.try_send(TransferProgress::FileProgress {
                            session_id: this.session_id.clone(),
                            file_id: this.file_id.clone(),
                            file_name: this.file_name.clone(),
                            file_bytes_sent: this.file_sent,
                            file_total_bytes: this.file_total,
                            session_bytes_sent: cur_session_sent,
                            session_total_bytes: this.session_total,
                            bytes_per_second,
                            eta,
                        });
                    }
                }

                std::task::Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mime_guess_simple() {
        assert_eq!(mime_guess_simple("photo.jpg"), "image/jpeg");
        assert_eq!(mime_guess_simple("photo.JPEG"), "image/jpeg");
        assert_eq!(mime_guess_simple("graphic.PNG"), "image/png");
        assert_eq!(mime_guess_simple("doc.pdf"), "application/pdf");
        assert_eq!(mime_guess_simple("notes.txt"), "text/plain");
        assert_eq!(mime_guess_simple("binary.bin"), "application/octet-stream");
    }

    #[tokio::test]
    async fn test_outgoing_file_from_path_and_sha256() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_outgoing_test_{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let file_path = temp_dir.join("sample.txt");
        tokio::fs::write(&file_path, b"Hello LanFlare!").await.unwrap();

        let outgoing = OutgoingFile::from_path(&file_path)
            .await
            .expect("Should construct from path");
        assert_eq!(outgoing.file_name, "sample.txt");
        assert_eq!(outgoing.size, 15);
        assert_eq!(outgoing.file_type, "text/plain");
        assert!(outgoing.sha256.is_none());

        let outgoing_with_hash = outgoing
            .with_computed_sha256()
            .await
            .expect("Should compute hash");
        assert!(outgoing_with_hash.sha256.is_some());
        let dto = outgoing_with_hash.to_dto();
        assert_eq!(dto.file_name, "sample.txt");
        assert_eq!(dto.size, 15);
        assert_eq!(dto.sha256, outgoing_with_hash.sha256);

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}

