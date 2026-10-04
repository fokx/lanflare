//! Streaming disk writer with real-time SHA-256 calculation.

use std::path::{Path, PathBuf};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, BufWriter};

use super::path::PathSanitizeError;

/// Temporary file extension for ongoing downloads.
pub const TEMP_FILE_EXTENSION: &str = "lanflare-part";

/// Buffer capacity for the async disk writer (64 KB for high-throughput LAN transfers).
const BUFFER_CAPACITY: usize = 64 * 1024;

/// Errors that can occur during streaming storage operations.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Path sanitization error: {0}")]
    Path(#[from] PathSanitizeError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Size mismatch: expected {expected} bytes, received {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },

    #[error("Size limit exceeded: received {actual} bytes, exceeding maximum expected {expected} bytes")]
    SizeExceeded { expected: u64, actual: u64 },

    #[error("Checksum mismatch: expected {expected}, computed {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("Destination file already exists: {0}")]
    AlreadyExists(PathBuf),

    #[error("Transfer was aborted or already completed")]
    Aborted,

    #[error("Stream error: {0}")]
    StreamError(String),
}

/// Metadata result returned after a file is successfully verified and committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageCommitResult {
    /// Final destination path of the file.
    pub target_path: PathBuf,
    /// Total number of bytes written.
    pub bytes_written: u64,
    /// Computed SHA-256 hash in lowercase hex format.
    pub sha256: String,
}

/// Async buffered disk writer that streams data directly to a temporary `.lanflare-part` file
/// while concurrently calculating the SHA-256 hash incrementally in memory.
#[derive(Debug)]
pub struct StreamingFileWriter {
    target_path: PathBuf,
    temp_path: PathBuf,
    writer: Option<BufWriter<tokio::fs::File>>,
    hasher: Sha256,
    bytes_written: u64,
    expected_size: Option<u64>,
    expected_sha256: Option<String>,
    committed: bool,
}

impl StreamingFileWriter {
    /// Creates a new streaming file writer. Overwrites target file if it already exists.
    pub async fn create(
        target_path: impl Into<PathBuf>,
        expected_size: Option<u64>,
        expected_sha256: Option<String>,
    ) -> Result<Self, StorageError> {
        Self::create_with_overwrite(target_path, expected_size, expected_sha256, true).await
    }

    /// Creates a new streaming file writer with optional overwrite protection.
    pub async fn create_with_overwrite(
        target_path: impl Into<PathBuf>,
        expected_size: Option<u64>,
        expected_sha256: Option<String>,
        allow_overwrite: bool,
    ) -> Result<Self, StorageError> {
        let target_path = target_path.into();

        if !allow_overwrite && target_path.exists() {
            return Err(StorageError::AlreadyExists(target_path));
        }

        let parent = target_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let file_name = target_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("transfer");
        let random_suffix = uuid::Uuid::new_v4().simple().to_string();
        let temp_name = format!(".{file_name}.{random_suffix}.{TEMP_FILE_EXTENSION}");
        let temp_path = parent.join(temp_name);

        let file = tokio::fs::File::create(&temp_path).await?;
        let writer = BufWriter::with_capacity(BUFFER_CAPACITY, file);

        Ok(Self {
            target_path,
            temp_path,
            writer: Some(writer),
            hasher: Sha256::new(),
            bytes_written: 0,
            expected_size,
            expected_sha256,
            committed: false,
        })
    }

    /// Returns the target destination path.
    pub fn target_path(&self) -> &Path {
        &self.target_path
    }

    /// Returns the temporary file path currently being written to.
    pub fn temp_path(&self) -> &Path {
        &self.temp_path
    }

    /// Returns the total number of bytes written so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns the expected size in bytes, if specified.
    pub fn expected_size(&self) -> Option<u64> {
        self.expected_size
    }

    /// Returns the expected SHA-256 hash, if specified.
    pub fn expected_sha256(&self) -> Option<&str> {
        self.expected_sha256.as_deref()
    }

    /// Writes a single chunk to disk and updates the running SHA-256 digest.
    pub async fn write_chunk(&mut self, chunk: &[u8]) -> Result<(), StorageError> {
        if self.committed {
            return Err(StorageError::Aborted);
        }

        if let Some(expected) = self.expected_size {
            let next_total = self.bytes_written + chunk.len() as u64;
            if next_total > expected {
                // Abort early to prevent disk space exhaustion attacks
                self.cleanup_temp_file().await;
                return Err(StorageError::SizeExceeded {
                    expected,
                    actual: next_total,
                });
            }
        }

        self.hasher.update(chunk);

        if let Some(writer) = &mut self.writer {
            writer.write_all(chunk).await?;
            self.bytes_written += chunk.len() as u64;
            Ok(())
        } else {
            Err(StorageError::Aborted)
        }
    }

    /// Streams chunks from an asynchronous stream and writes them directly to disk.
    pub async fn write_all_from_stream<S, B, E>(&mut self, mut stream: S) -> Result<(), StorageError>
    where
        S: futures_util::Stream<Item = Result<B, E>> + Unpin,
        B: AsRef<[u8]>,
        E: std::fmt::Display,
    {
        use futures_util::StreamExt;
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => self.write_chunk(chunk.as_ref()).await?,
                Err(err) => {
                    self.cleanup_temp_file().await;
                    return Err(StorageError::StreamError(err.to_string()));
                }
            }
        }
        Ok(())
    }

    /// Reads all data from an `AsyncRead` source and writes it to disk.
    pub async fn write_all_from_reader<R>(&mut self, mut reader: R) -> Result<(), StorageError>
    where
        R: AsyncRead + Unpin,
    {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let n = reader.read(&mut buffer).await?;
            if n == 0 {
                break;
            }
            self.write_chunk(&buffer[..n]).await?;
        }
        Ok(())
    }

    /// Flushes data, verifies size and SHA-256 checksum, and atomically renames
    /// the temporary `.lanflare-part` file to the final destination path.
    pub async fn commit(mut self) -> Result<StorageCommitResult, StorageError> {
        if self.committed {
            return Err(StorageError::Aborted);
        }

        // Flush and close the writer to release file handles before renaming
        if let Some(mut writer) = self.writer.take() {
            writer.flush().await?;
            drop(writer);
        } else {
            return Err(StorageError::Aborted);
        }

        // Finalize SHA-256 hash
        let hasher = std::mem::take(&mut self.hasher);
        let computed_hash = format!("{:x}", hasher.finalize());

        // Validate size if an expectation was set
        if let Some(expected) = self.expected_size
            && self.bytes_written != expected
        {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
            self.committed = true;
            return Err(StorageError::SizeMismatch {
                expected,
                actual: self.bytes_written,
            });
        }

        // Validate checksum if an expectation was set
        if let Some(ref expected) = self.expected_sha256
            && !expected.eq_ignore_ascii_case(&computed_hash)
        {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
            self.committed = true;
            return Err(StorageError::ChecksumMismatch {
                expected: expected.clone(),
                actual: computed_hash,
            });
        }

        // Atomically rename temporary file to destination path
        tokio::fs::rename(&self.temp_path, &self.target_path).await?;
        self.committed = true;

        Ok(StorageCommitResult {
            target_path: self.target_path.clone(),
            bytes_written: self.bytes_written,
            sha256: computed_hash,
        })
    }

    /// Aborts writing and immediately removes the temporary file from disk.
    pub async fn abort(mut self) -> Result<(), StorageError> {
        self.cleanup_temp_file().await;
        Ok(())
    }

    /// Internal cleanup helper that drops the open file handle and deletes the temp file.
    async fn cleanup_temp_file(&mut self) {
        if let Some(writer) = self.writer.take() {
            drop(writer);
        }
        if self.temp_path.exists() {
            let _ = tokio::fs::remove_file(&self.temp_path).await;
        }
        self.committed = true;
    }
}

impl Drop for StreamingFileWriter {
    fn drop(&mut self) {
        if !self.committed {
            // Close file handle first so file locks are released
            self.writer.take();
            let temp_path = self.temp_path.clone();
            if temp_path.exists() {
                let _ = std::fs::remove_file(temp_path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_streaming_write_and_commit_success() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_test_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("sample.bin");

        let payload = b"Hello LocalSend and LanFlare transfer engine!";
        let expected_hash = format!("{:x}", Sha256::digest(payload));

        let mut writer = StreamingFileWriter::create(
            &dest_file,
            Some(payload.len() as u64),
            Some(expected_hash.clone()),
        )
        .await
        .unwrap();

        assert!(writer.temp_path().exists());
        assert!(!dest_file.exists());

        // Write in two chunks
        writer.write_chunk(&payload[..15]).await.unwrap();
        writer.write_chunk(&payload[15..]).await.unwrap();

        assert_eq!(writer.bytes_written(), payload.len() as u64);

        let result = writer.commit().await.unwrap();
        assert_eq!(result.target_path, dest_file);
        assert_eq!(result.bytes_written, payload.len() as u64);
        assert_eq!(result.sha256, expected_hash);

        // Target file exists with exact bytes
        let on_disk = tokio::fs::read(&dest_file).await.unwrap();
        assert_eq!(on_disk, payload);

        // Temp file no longer exists
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_checksum_mismatch_1bit_corruption() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_corrupt_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("corrupted.bin");

        let payload = b"Original uncorrupted bytes data";
        let valid_hash = format!("{:x}", Sha256::digest(payload));

        let mut writer = StreamingFileWriter::create(
            &dest_file,
            Some(payload.len() as u64),
            Some(valid_hash),
        )
        .await
        .unwrap();

        let temp_path = writer.temp_path().to_path_buf();
        assert!(temp_path.exists());

        // Introduce 1-bit corruption in payload
        let mut corrupted_payload = payload.to_vec();
        corrupted_payload[0] ^= 0x01; // flip 1 bit

        writer.write_chunk(&corrupted_payload).await.unwrap();

        // Commit must detect mismatch and reject
        let err = writer.commit().await.unwrap_err();
        match err {
            StorageError::ChecksumMismatch { expected, actual } => {
                assert_ne!(expected, actual);
            }
            other => panic!("Expected ChecksumMismatch, got: {other:?}"),
        }

        // Target file must not exist
        assert!(!dest_file.exists());
        // Temp file must have been cleaned up
        assert!(!temp_path.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_size_mismatch_on_commit() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_size_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("incomplete.bin");

        let mut writer = StreamingFileWriter::create(&dest_file, Some(100), None).await.unwrap();
        let temp_path = writer.temp_path().to_path_buf();

        writer.write_chunk(b"only 25 bytes here.......").await.unwrap();
        let err = writer.commit().await.unwrap_err();

        match err {
            StorageError::SizeMismatch { expected, actual } => {
                assert_eq!(expected, 100);
                assert_eq!(actual, 25);
            }
            other => panic!("Expected SizeMismatch, got: {other:?}"),
        }

        assert!(!dest_file.exists());
        assert!(!temp_path.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_size_exceeded_early_abort() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_exceed_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("bomb.bin");

        let mut writer = StreamingFileWriter::create(&dest_file, Some(20), None).await.unwrap();
        let temp_path = writer.temp_path().to_path_buf();

        let err = writer.write_chunk(&[0u8; 30]).await.unwrap_err();
        match err {
            StorageError::SizeExceeded { expected, actual } => {
                assert_eq!(expected, 20);
                assert_eq!(actual, 30);
            }
            other => panic!("Expected SizeExceeded, got: {other:?}"),
        }

        // Temp file is immediately cleaned up
        assert!(!temp_path.exists());
        assert!(!dest_file.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_manual_abort() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_abort_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("aborted.bin");

        let mut writer = StreamingFileWriter::create(&dest_file, None, None).await.unwrap();
        let temp_path = writer.temp_path().to_path_buf();
        writer.write_chunk(b"partial content").await.unwrap();

        assert!(temp_path.exists());
        writer.abort().await.unwrap();

        assert!(!temp_path.exists());
        assert!(!dest_file.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_implicit_drop_cleanup() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_drop_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("dropped.bin");

        let temp_path;
        {
            let mut writer = StreamingFileWriter::create(&dest_file, None, None).await.unwrap();
            temp_path = writer.temp_path().to_path_buf();
            writer.write_chunk(b"will be dropped").await.unwrap();
            assert!(temp_path.exists());
            // writer dropped here without commit
        }

        assert!(!temp_path.exists());
        assert!(!dest_file.exists());

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_write_from_reader() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_reader_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("streamed.txt");

        let text = "Streamed via AsyncRead reader correctly!";
        let cursor = std::io::Cursor::new(text.as_bytes());

        let mut writer = StreamingFileWriter::create(&dest_file, None, None).await.unwrap();
        writer.write_all_from_reader(cursor).await.unwrap();
        let res = writer.commit().await.unwrap();

        assert_eq!(res.bytes_written, text.len() as u64);
        let content = tokio::fs::read_to_string(&dest_file).await.unwrap();
        assert_eq!(content, text);

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_target_already_exists_rejection() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_exists_{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temp_dir).await.unwrap();
        let dest_file = temp_dir.join("existing.txt");
        tokio::fs::write(&dest_file, b"already here").await.unwrap();

        let err = StreamingFileWriter::create_with_overwrite(&dest_file, None, None, false)
            .await
            .unwrap_err();

        match err {
            StorageError::AlreadyExists(p) => assert_eq!(p, dest_file),
            other => panic!("Expected AlreadyExists, got: {other:?}"),
        }

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    async fn test_streaming_write_from_stream() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_writer_stream_{}", uuid::Uuid::new_v4()));
        let dest_file = temp_dir.join("streamed_chunks.txt");

        let chunks: Vec<Result<Vec<u8>, std::io::Error>> = vec![
            Ok(b"Chunk 1, ".to_vec()),
            Ok(b"Chunk 2, ".to_vec()),
            Ok(b"Chunk 3.".to_vec()),
        ];
        let stream = futures_util::stream::iter(chunks);

        let mut writer = StreamingFileWriter::create(&dest_file, None, None).await.unwrap();
        writer.write_all_from_stream(stream).await.unwrap();
        let res = writer.commit().await.unwrap();

        assert_eq!(res.bytes_written, 26);
        let content = tokio::fs::read_to_string(&dest_file).await.unwrap();
        assert_eq!(content, "Chunk 1, Chunk 2, Chunk 3.");

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
