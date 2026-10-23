//! Resumable byte-offset checkpointing for interrupted `.lanflare-part` transfers.
//!
//! Sub-phase 8.3 implementation:
//! - Tracks partial transfer states (`PartialTransferState`) in `.{filename}.{session_id}.lanflare-resume.json`.
//! - Streaming writer (`ResumableFileWriter`) that can resume from verified on-disk byte offsets,
//!   incrementally updating the running SHA-256 hasher.
//! - `ResumeManager` providing directory-level checkpoint discovery, resume negotiation
//!   (`handle_resume_request`), and cleanup.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lanflare_proto::chat::ResumeTransferDto;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufWriter, SeekFrom};

use crate::storage::path::{sanitize_filename, PathSanitizeError};
use crate::storage::writer::{StorageCommitResult, StorageError, DEFAULT_BUFFER_CAPACITY, TEMP_FILE_EXTENSION};

/// Extension for resume metadata checkpoint files.
pub const RESUME_CHECKPOINT_EXTENSION: &str = "lanflare-resume.json";

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Errors arising during resumable transfer checkpointing and streaming.
#[derive(Debug, Error)]
pub enum ResumeError {
    #[error("Path sanitization error: {0}")]
    Path(#[from] PathSanitizeError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("Size exceeded: received {actual} bytes, expected maximum {expected} bytes")]
    SizeExceeded { expected: u64, actual: u64 },

    #[error("Size mismatch: received {actual} bytes, expected {expected} bytes")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("Checksum mismatch: expected {expected}, computed {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Invalid offset: expected {expected}, provided {actual}")]
    InvalidOffset { expected: u64, actual: u64 },

    #[error("Checkpoint not found for session {0}, file {1}")]
    NotFound(String, String),

    #[error("Transfer already committed or aborted")]
    AlreadyCommitted,
}

/// Persistent state metadata recorded in checkpoint files for interrupted transfers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialTransferState {
    /// Transfer session identifier.
    pub session_id: String,

    /// Unique file identifier within session.
    pub file_id: String,

    /// Sanitized base target file name.
    pub file_name: String,

    /// Canonical uppercase SHA-256 fingerprint of the sender.
    pub sender_fp: String,

    /// Canonical uppercase SHA-256 fingerprint of the recipient.
    pub recipient_fp: String,

    /// Total promised byte length of the file.
    pub total_bytes: u64,

    /// Number of verified bytes already received on disk.
    pub received_bytes: u64,

    /// Expected SHA-256 checksum of the completed full file.
    pub sha256: String,

    /// Chunk size used for streaming (default: 128 KB).
    pub chunk_size: usize,

    /// UNIX timestamp in milliseconds of the latest write or checkpoint.
    pub last_updated_ms: u64,
}

impl PartialTransferState {
    /// Formats the checkpoint JSON file name: `.{filename}.{session_id}.lanflare-resume.json`.
    pub fn checkpoint_filename(file_name: &str, session_id: &str) -> String {
        format!(".{file_name}.{session_id}.{RESUME_CHECKPOINT_EXTENSION}")
    }

    /// Formats the partial data file name: `.{filename}.{session_id}.lanflare-part`.
    pub fn part_filename(file_name: &str, session_id: &str) -> String {
        format!(".{file_name}.{session_id}.{TEMP_FILE_EXTENSION}")
    }

    /// Converts this state into a `ResumeTransferDto` for wire protocol negotiation.
    pub fn to_dto(&self) -> ResumeTransferDto {
        ResumeTransferDto {
            session_id: self.session_id.clone(),
            file_id: self.file_id.clone(),
            sender_fp: self.sender_fp.clone(),
            recipient_fp: self.recipient_fp.clone(),
            total_bytes: self.total_bytes,
            received_bytes: self.received_bytes,
            sha256: self.sha256.clone(),
            chunk_size: self.chunk_size,
        }
    }
}

/// Asynchronous streaming writer capable of resuming an interrupted transfer from a disk checkpoint.
pub struct ResumableFileWriter {
    base_dir: PathBuf,
    state: PartialTransferState,
    part_path: PathBuf,
    checkpoint_path: PathBuf,
    target_path: PathBuf,
    writer: Option<BufWriter<tokio::fs::File>>,
    hasher: Sha256,
    committed: bool,
}

impl ResumableFileWriter {
    /// Creates a new resumable writer or resumes an existing partial transfer from disk.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_or_resume(
        base_dir: impl Into<PathBuf>,
        session_id: &str,
        file_id: &str,
        file_name: &str,
        sender_fp: &str,
        recipient_fp: &str,
        total_bytes: u64,
        expected_sha256: &str,
        chunk_size: usize,
    ) -> Result<Self, ResumeError> {
        let base_dir = base_dir.into();
        tokio::fs::create_dir_all(&base_dir).await?;

        let clean_name = sanitize_filename(file_name)?;
        let part_name = PartialTransferState::part_filename(&clean_name, session_id);
        let checkpoint_name = PartialTransferState::checkpoint_filename(&clean_name, session_id);

        let part_path = base_dir.join(&part_name);
        let checkpoint_path = base_dir.join(&checkpoint_name);
        let target_path = base_dir.join(&clean_name);

        let chunk_size = if chunk_size == 0 {
            DEFAULT_BUFFER_CAPACITY
        } else {
            chunk_size
        };

        // Check if an existing checkpoint exists and can be resumed
        let mut resume_from_offset = 0u64;
        let mut hasher = Sha256::new();

        if checkpoint_path.exists()
            && part_path.exists()
            && let Ok(bytes) = tokio::fs::read(&checkpoint_path).await
            && let Ok(saved_state) = serde_json::from_slice::<PartialTransferState>(&bytes)
            && saved_state.session_id == session_id
            && saved_state.file_id == file_id
            && saved_state.sha256.eq_ignore_ascii_case(expected_sha256)
            && saved_state.total_bytes == total_bytes
            && let Ok(meta) = tokio::fs::metadata(&part_path).await
        {
            let disk_len = meta.len();
            let valid_bytes = saved_state.received_bytes.min(disk_len);

            if valid_bytes > 0 {
                // Seed hasher by reading verified bytes from disk
                let mut existing_file = tokio::fs::File::open(&part_path).await?;
                let mut buffer = vec![0u8; chunk_size];
                let mut remaining = valid_bytes;

                while remaining > 0 {
                    let to_read = (remaining as usize).min(buffer.len());
                    let n = existing_file.read_exact(&mut buffer[..to_read]).await?;
                    hasher.update(&buffer[..n]);
                    remaining -= n as u64;
                }

                resume_from_offset = valid_bytes;
            }
        }

        let state = PartialTransferState {
            session_id: session_id.to_string(),
            file_id: file_id.to_string(),
            file_name: clean_name,
            sender_fp: sender_fp.to_ascii_uppercase(),
            recipient_fp: recipient_fp.to_ascii_uppercase(),
            total_bytes,
            received_bytes: resume_from_offset,
            sha256: expected_sha256.to_string(),
            chunk_size,
            last_updated_ms: current_time_ms(),
        };

        // Open or create the part file
        let file = if resume_from_offset > 0 {
            let mut f = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&part_path)
                .await?;
            f.seek(SeekFrom::Start(resume_from_offset)).await?;
            f
        } else {
            tokio::fs::File::create(&part_path).await?
        };

        let writer = BufWriter::with_capacity(chunk_size, file);

        let mut instance = Self {
            base_dir,
            state,
            part_path,
            checkpoint_path,
            target_path,
            writer: Some(writer),
            hasher,
            committed: false,
        };

        // Persist initial checkpoint
        instance.save_checkpoint_internal().await?;

        Ok(instance)
    }

    /// Returns the current partial transfer state.
    pub fn state(&self) -> &PartialTransferState {
        &self.state
    }

    /// Returns the base directory for this transfer.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Returns the verified number of bytes written so far.
    pub fn received_bytes(&self) -> u64 {
        self.state.received_bytes
    }

    /// Returns the total promised file size.
    pub fn total_bytes(&self) -> u64 {
        self.state.total_bytes
    }

    /// Returns true if all bytes have been received.
    pub fn is_complete(&self) -> bool {
        self.state.received_bytes == self.state.total_bytes
    }

    /// Writes a single chunk to the partial file, updating the running SHA-256 hasher.
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), ResumeError> {
        if self.committed {
            return Err(ResumeError::AlreadyCommitted);
        }

        let next_total = self.state.received_bytes + chunk.len() as u64;
        if next_total > self.state.total_bytes {
            return Err(ResumeError::SizeExceeded {
                expected: self.state.total_bytes,
                actual: next_total,
            });
        }

        self.hasher.update(chunk);

        if let Some(writer) = &mut self.writer {
            writer.write_all(chunk).await?;
            self.state.received_bytes = next_total;
            self.state.last_updated_ms = current_time_ms();
            self.save_checkpoint_internal().await?;
            Ok(())
        } else {
            Err(ResumeError::AlreadyCommitted)
        }
    }

    /// Flushes writer buffer and persists checkpoint metadata to disk.
    pub async fn save_checkpoint(&mut self) -> Result<(), ResumeError> {
        if let Some(writer) = &mut self.writer {
            writer.flush().await?;
        }
        self.save_checkpoint_internal().await
    }

    async fn save_checkpoint_internal(&mut self) -> Result<(), ResumeError> {
        let json_bytes = serde_json::to_vec_pretty(&self.state)?;
        let tmp_path = self.checkpoint_path.with_extension(format!(
            "{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::write(&tmp_path, json_bytes).await?;
        tokio::fs::rename(&tmp_path, &self.checkpoint_path).await?;
        Ok(())
    }

    /// Pauses writing and closes file handles while safely preserving the `.lanflare-part` file
    /// and checkpoint JSON on disk for future resumption.
    pub async fn pause(mut self) -> Result<PartialTransferState, ResumeError> {
        if let Some(mut writer) = self.writer.take() {
            writer.flush().await?;
        }
        self.save_checkpoint_internal().await?;
        self.committed = true; // Prevents Drop from acting
        Ok(self.state.clone())
    }

    /// Aborts transfer and removes both partial data and checkpoint files.
    pub async fn abort(mut self) -> Result<(), ResumeError> {
        self.writer.take();
        if self.part_path.exists() {
            let _ = tokio::fs::remove_file(&self.part_path).await;
        }
        if self.checkpoint_path.exists() {
            let _ = tokio::fs::remove_file(&self.checkpoint_path).await;
        }
        self.committed = true;
        Ok(())
    }

    /// Flushes writer, validates total size and full-file SHA-256 checksum, atomically renames
    /// the `.lanflare-part` file to the target destination, and cleans up checkpoint metadata.
    pub async fn commit(mut self) -> Result<StorageCommitResult, ResumeError> {
        if self.committed {
            return Err(ResumeError::AlreadyCommitted);
        }

        if let Some(mut writer) = self.writer.take() {
            writer.flush().await?;
            drop(writer);
        }

        if self.state.received_bytes != self.state.total_bytes {
            return Err(ResumeError::SizeMismatch {
                expected: self.state.total_bytes,
                actual: self.state.received_bytes,
            });
        }

        let hasher = std::mem::take(&mut self.hasher);
        let computed = format!("{:x}", hasher.finalize());

        if !self.state.sha256.eq_ignore_ascii_case(&computed) {
            return Err(ResumeError::ChecksumMismatch {
                expected: self.state.sha256.clone(),
                actual: computed,
            });
        }

        // Atomically rename part file to destination path
        tokio::fs::rename(&self.part_path, &self.target_path).await?;

        // Remove resume checkpoint
        if self.checkpoint_path.exists() {
            let _ = tokio::fs::remove_file(&self.checkpoint_path).await;
        }

        self.committed = true;

        Ok(StorageCommitResult {
            target_path: self.target_path.clone(),
            bytes_written: self.state.received_bytes,
            sha256: computed,
        })
    }
}

impl Drop for ResumableFileWriter {
    fn drop(&mut self) {
        // When dropped without commit/pause, close file handle.
        // We DO NOT delete part or checkpoint files, ensuring interrupted transfers can be resumed.
        self.writer.take();
    }
}

/// Directory-level manager for discovering, validating, and resuming interrupted transfers.
#[derive(Clone, Debug)]
pub struct ResumeManager {
    base_dir: PathBuf,
}

impl ResumeManager {
    /// Creates a new `ResumeManager` operating within the given base directory.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Returns the configured base directory.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Retrieves an existing checkpoint for a specific file and session if present.
    pub async fn get_checkpoint(
        &self,
        file_name: &str,
        session_id: &str,
    ) -> Result<Option<PartialTransferState>, ResumeError> {
        let clean_name = sanitize_filename(file_name)?;
        let checkpoint_name = PartialTransferState::checkpoint_filename(&clean_name, session_id);
        let checkpoint_path = self.base_dir.join(checkpoint_name);

        if !checkpoint_path.exists() {
            return Ok(None);
        }

        let bytes = tokio::fs::read(&checkpoint_path).await?;
        let state: PartialTransferState = serde_json::from_slice(&bytes)?;
        Ok(Some(state))
    }

    /// Finds a checkpoint matching session ID and file ID.
    pub async fn find_checkpoint(
        &self,
        session_id: &str,
        file_id: &str,
    ) -> Result<Option<PartialTransferState>, ResumeError> {
        let checkpoints = self.list_checkpoints().await?;
        for cp in checkpoints {
            if cp.session_id == session_id && cp.file_id == file_id {
                return Ok(Some(cp));
            }
        }
        Ok(None)
    }

    /// Handles an incoming `ResumeTransferDto` negotiation request:
    /// returns the verified on-disk `received_bytes` if found, or 0 if fresh/invalid.
    pub async fn handle_resume_request(
        &self,
        req: &ResumeTransferDto,
    ) -> Result<ResumeTransferDto, ResumeError> {
        let mut response = req.clone();
        if let Some(state) = self.find_checkpoint(&req.session_id, &req.file_id).await? {
            let part_name = PartialTransferState::part_filename(&state.file_name, &state.session_id);
            let part_path = self.base_dir.join(part_name);

            if part_path.exists()
                && state.sha256.eq_ignore_ascii_case(&req.sha256)
                && state.total_bytes == req.total_bytes
                && let Ok(meta) = tokio::fs::metadata(&part_path).await
            {
                response.received_bytes = state.received_bytes.min(meta.len());
                return Ok(response);
            }
        }

        response.received_bytes = 0;
        Ok(response)
    }

    /// Creates or resumes a streaming file writer.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_writer(
        &self,
        session_id: &str,
        file_id: &str,
        file_name: &str,
        sender_fp: &str,
        recipient_fp: &str,
        total_bytes: u64,
        expected_sha256: &str,
        chunk_size: usize,
    ) -> Result<ResumableFileWriter, ResumeError> {
        ResumableFileWriter::create_or_resume(
            &self.base_dir,
            session_id,
            file_id,
            file_name,
            sender_fp,
            recipient_fp,
            total_bytes,
            expected_sha256,
            chunk_size,
        )
        .await
    }

    /// Deletes a checkpoint and its corresponding partial `.lanflare-part` file.
    pub async fn delete_checkpoint(&self, file_name: &str, session_id: &str) -> Result<bool, ResumeError> {
        let clean_name = sanitize_filename(file_name)?;
        let checkpoint_name = PartialTransferState::checkpoint_filename(&clean_name, session_id);
        let part_name = PartialTransferState::part_filename(&clean_name, session_id);

        let checkpoint_path = self.base_dir.join(checkpoint_name);
        let part_path = self.base_dir.join(part_name);

        let mut deleted = false;
        if checkpoint_path.exists() {
            let _ = tokio::fs::remove_file(checkpoint_path).await;
            deleted = true;
        }
        if part_path.exists() {
            let _ = tokio::fs::remove_file(part_path).await;
            deleted = true;
        }

        Ok(deleted)
    }

    /// Enumerates all active partial transfer checkpoints in the base directory.
    pub async fn list_checkpoints(&self) -> Result<Vec<PartialTransferState>, ResumeError> {
        let mut results = Vec::new();
        if !self.base_dir.exists() {
            return Ok(results);
        }

        let mut read_dir = tokio::fs::read_dir(&self.base_dir).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str())
                && file_name.ends_with(RESUME_CHECKPOINT_EXTENSION)
                && let Ok(bytes) = tokio::fs::read(&path).await
                && let Ok(state) = serde_json::from_slice::<PartialTransferState>(&bytes)
            {
                results.push(state);
            }
        }

        Ok(results)
    }

    /// Sweeps and removes stale checkpoints older than `max_age`.
    pub async fn sweep_stale_checkpoints(&self, max_age: Duration) -> Result<usize, ResumeError> {
        let now = current_time_ms();
        let max_age_ms = max_age.as_millis() as u64;
        let mut count = 0;

        for state in self.list_checkpoints().await? {
            if now.saturating_sub(state.last_updated_ms) > max_age_ms
                && self.delete_checkpoint(&state.file_name, &state.session_id).await?
            {
                count += 1;
            }
        }

        Ok(count)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[tokio::test]
    pub async fn test_interrupted_stream_checkpoint_and_byte_offset_resume() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_resume_test_{}", uuid::Uuid::new_v4()));
        let resume_mgr = ResumeManager::new(&temp_dir);

        // 1. Prepare 4 MB test payload (4,194,304 bytes)
        let total_bytes: usize = 4 * 1024 * 1024;
        let mut payload = Vec::with_capacity(total_bytes);
        for i in 0..total_bytes {
            payload.push((i % 251) as u8);
        }
        let expected_sha256 = format!("{:x}", Sha256::digest(&payload));

        let session_id = "test-session-4mb";
        let file_id = "video-part-1";
        let file_name = "sample_4mb_video.mp4";
        let sender_fp = "AAAA111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0001";
        let recipient_fp = "BBBB111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0002";
        let chunk_size = 128 * 1024; // 128 KB chunks

        // 2. Start initial transfer and stream strictly 50% (2 MB = 2,097,152 bytes)
        let half_bytes = total_bytes / 2;
        {
            let mut writer = resume_mgr
                .create_writer(
                    session_id,
                    file_id,
                    file_name,
                    sender_fp,
                    recipient_fp,
                    total_bytes as u64,
                    &expected_sha256,
                    chunk_size,
                )
                .await
                .expect("create writer");

            let mut offset = 0;
            while offset < half_bytes {
                let end = (offset + chunk_size).min(half_bytes);
                writer
                    .write_chunk(&payload[offset..end])
                    .await
                    .expect("write chunk");
                offset = end;
            }

            assert_eq!(writer.received_bytes(), half_bytes as u64);
            assert!(!writer.is_complete());

            // Simulate interruption: pause & drop writer
            let paused_state = writer.pause().await.expect("pause writer");
            assert_eq!(paused_state.received_bytes, half_bytes as u64);
        }

        // 3. Verify on-disk checkpoint and part file state
        let checkpoint = resume_mgr
            .find_checkpoint(session_id, file_id)
            .await
            .expect("find checkpoint")
            .expect("checkpoint must exist on disk");
        assert_eq!(checkpoint.received_bytes, half_bytes as u64);
        assert_eq!(checkpoint.total_bytes, total_bytes as u64);

        // 4. Test wire negotiation with handle_resume_request
        let req_dto = ResumeTransferDto {
            session_id: session_id.to_string(),
            file_id: file_id.to_string(),
            sender_fp: sender_fp.to_string(),
            recipient_fp: recipient_fp.to_string(),
            total_bytes: total_bytes as u64,
            received_bytes: 0, // Sender probing receiver's state
            sha256: expected_sha256.clone(),
            chunk_size,
        };

        let negotiated = resume_mgr
            .handle_resume_request(&req_dto)
            .await
            .expect("handle resume request");
        assert_eq!(
            negotiated.received_bytes, half_bytes as u64,
            "Receiver must acknowledge verified 2 MB byte offset"
        );

        // 5. Reconnect and resume streaming strictly the remaining 2 MB
        {
            let mut resumed_writer = resume_mgr
                .create_writer(
                    session_id,
                    file_id,
                    file_name,
                    sender_fp,
                    recipient_fp,
                    total_bytes as u64,
                    &expected_sha256,
                    chunk_size,
                )
                .await
                .expect("resume writer");

            assert_eq!(
                resumed_writer.received_bytes(),
                half_bytes as u64,
                "Resumed writer must start at offset 2 MB"
            );

            let mut offset = half_bytes;
            while offset < total_bytes {
                let end = (offset + chunk_size).min(total_bytes);
                resumed_writer
                    .write_chunk(&payload[offset..end])
                    .await
                    .expect("write remaining chunk");
                offset = end;
            }

            assert_eq!(resumed_writer.received_bytes(), total_bytes as u64);
            assert!(resumed_writer.is_complete());

            // 6. Commit and verify full-file SHA-256 upon atomic rename
            let commit_result = resumed_writer.commit().await.expect("commit resumed file");
            assert_eq!(commit_result.bytes_written, total_bytes as u64);
            assert_eq!(commit_result.sha256, expected_sha256);
            assert!(commit_result.target_path.exists());
        }

        // 7. Verify checkpoint was deleted upon successful commit
        let after_checkpoint = resume_mgr
            .find_checkpoint(session_id, file_id)
            .await
            .expect("find checkpoint after commit");
        assert!(
            after_checkpoint.is_none(),
            "Checkpoint must be removed after successful commit"
        );

        // 8. Verify the committed target file on disk matches the exact original payload
        let target_file_path = temp_dir.join(file_name);
        let disk_bytes = tokio::fs::read(&target_file_path)
            .await
            .expect("read target file");
        assert_eq!(disk_bytes, payload, "Committed file must match exact payload bit-for-bit");

        // Cleanup temp dir
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
