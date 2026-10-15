//! High-throughput local loopback benchmark suite for LanFlare.
//!
//! Benchmarks:
//! 1. `StreamingFileWriter` buffer sizing profile (64KB, 128KB, 256KB, 512KB, 1MB) with SHA-256 calculation.
//! 2. End-to-end plain HTTP file transfer throughput over local loopback.
//! 3. End-to-end HTTPS (mTLS) with pinned SHA-256 fingerprint verification over local loopback.
//! 4. Concurrent parallel multi-file transfer throughput.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use uuid::Uuid;

use lanflare_core::client::transfer::{OutgoingFile, TransferOptions, TransferOrchestrator};
use lanflare_core::client::{LsClient, LsClientConfig};
use lanflare_core::config::AppConfig;
use lanflare_core::crypto::cert::generate_self_signed;
use lanflare_core::discovery::registry::PeerRegistry;
use lanflare_core::server::start_server;
use lanflare_core::storage::writer::StreamingFileWriter;
use lanflare_proto::discovery::{DeviceType, ProtocolType, RegisterDtoV2};

/// Helper to format speed in megabytes per second (MB/s) and gigabits per second (Gbps).
fn format_throughput(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return "N/A".to_string();
    }
    let mb_per_sec = (bytes as f64 / 1_048_576.0) / secs;
    let gbps = (bytes as f64 * 8.0 / 1_000_000_000.0) / secs;
    format!("{mb_per_sec:7.2} MB/s ({gbps:5.2} Gbps)")
}

/// Generates a test file of `total_bytes` on disk filled with non-trivial byte pattern.
async fn create_benchmark_file(path: &Path, total_bytes: usize) -> String {
    let mut file = File::create(path).await.expect("Failed to create bench file");
    let chunk_size = 128 * 1024;
    let mut chunk = vec![0u8; chunk_size];
    let mut hasher = Sha256::new();

    // Fill chunk with reproducible pseudo-random pattern
    for (i, b) in chunk.iter_mut().enumerate() {
        *b = ((i * 31 + 17) % 256) as u8;
    }

    let mut written = 0;
    while written < total_bytes {
        let to_write = (total_bytes - written).min(chunk_size);
        file.write_all(&chunk[..to_write]).await.expect("Write error");
        hasher.update(&chunk[..to_write]);
        written += to_write;
    }
    file.flush().await.expect("Flush error");

    format!("{:x}", hasher.finalize())
}

/// Benchmark 1: Profile `StreamingFileWriter` buffer capacities (64KB, 128KB, 256KB, 512KB, 1MB).
async fn benchmark_disk_writer_buffer_sizing(temp_dir: &Path) {
    println!("\n================================================================================");
    println!(" BENCHMARK 1: Storage Pipeline Buffer Sizing Profile (64 MB payload)");
    println!("================================================================================");

    let test_bytes = 64 * 1024 * 1024; // 64 MB
    let chunk_size = 128 * 1024;
    let chunk = vec![0x5A; chunk_size];

    let capacities = [
        ("64 KB", 64 * 1024),
        ("128 KB (Default)", 128 * 1024),
        ("256 KB", 256 * 1024),
        ("512 KB", 512 * 1024),
        ("1024 KB", 1024 * 1024),
    ];

    for (label, cap) in capacities {
        let target_path = temp_dir.join(format!("buf_bench_{cap}.bin"));
        let start = Instant::now();

        let mut writer = StreamingFileWriter::create_with_capacity(
            &target_path,
            Some(test_bytes as u64),
            None,
            cap,
        )
        .await
        .expect("Writer creation failed");

        let mut written = 0;
        while written < test_bytes {
            let to_write = (test_bytes - written).min(chunk_size);
            writer.write_chunk(&chunk[..to_write]).await.expect("Chunk write failed");
            written += to_write;
        }

        let commit_res = writer.commit().await.expect("Commit failed");
        let elapsed = start.elapsed();

        assert_eq!(commit_res.bytes_written, test_bytes as u64);
        assert!(commit_res.target_path.exists());
        let _ = tokio::fs::remove_file(&commit_res.target_path).await;

        println!("  Buffer: {:<18} | Time: {:>7.2?} | Throughput: {}", label, elapsed, format_throughput(test_bytes as u64, elapsed));
    }
}

/// Benchmark 2: End-to-end plain HTTP streaming transfer over loopback TCP (64 MB).
async fn benchmark_end_to_end_plain_http_transfer(temp_dir: &Path) {
    println!("\n================================================================================");
    println!(" BENCHMARK 2: End-to-End Plain HTTP Streaming Transfer (64 MB file)");
    println!("================================================================================");

    let server_download_dir = temp_dir.join("http_receiver");
    tokio::fs::create_dir_all(&server_download_dir).await.unwrap();

    let server_config = AppConfig {
        alias: "HTTP Bench Receiver".to_string(),
        port: 0,
        download_dir: server_download_dir.clone(),
        auto_accept: true,
        pin: None,
        ..Default::default()
    };
    let registry = PeerRegistry::new();
    let (event_tx, _event_rx) = mpsc::channel(100);

    let server_handle = start_server(server_config, None, registry, Some(event_tx))
        .await
        .expect("Server bind failed");
    let server_port = server_handle.bound_port();
    let base_url = format!("http://127.0.0.1:{server_port}");

    // Prepare 64 MB file
    let file_size = 64 * 1024 * 1024;
    let source_path = temp_dir.join("http_bench_64mb.bin");
    let expected_hash = create_benchmark_file(&source_path, file_size).await;

    let out_file = OutgoingFile::from_path(&source_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    let client = LsClient::new(LsClientConfig::default()).unwrap();
    let sender_info = RegisterDtoV2 {
        alias: "HTTP Bench Sender".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "BENCH_HTTP_SENDER".to_string(),
        port: 53317,
        protocol: ProtocolType::Http,
        download: false,
    };

    let orchestrator = TransferOrchestrator::new(client, sender_info);
    let options = TransferOptions {
        max_concurrency: 1,
        chunk_size: 128 * 1024,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let start = Instant::now();
    orchestrator
        .transfer(&base_url, vec![out_file], options, None)
        .await
        .expect("Transfer must succeed");
    let elapsed = start.elapsed();

    // Verify bit-for-bit integrity
    let dest_file = server_download_dir.join("http_bench_64mb.bin");
    assert!(dest_file.exists());
    let dest_bytes = tokio::fs::read(&dest_file).await.unwrap();
    assert_eq!(dest_bytes.len(), file_size);
    let mut check_hasher = Sha256::new();
    check_hasher.update(&dest_bytes);
    let check_hash = format!("{:x}", check_hasher.finalize());
    assert_eq!(check_hash, expected_hash);

    println!("  Payload: 64 MB  | Time: {:>7.2?} | Throughput: {}", elapsed, format_throughput(file_size as u64, elapsed));
    server_handle.stop();
}

/// Benchmark 3: End-to-end HTTPS mTLS encrypted streaming transfer over loopback TCP (64 MB).
async fn benchmark_end_to_end_https_mtls_transfer(temp_dir: &Path) {
    println!("\n================================================================================");
    println!(" BENCHMARK 3: End-to-End HTTPS (mTLS) Streaming Transfer (64 MB file)");
    println!("================================================================================");

    let server_download_dir = temp_dir.join("https_receiver");
    tokio::fs::create_dir_all(&server_download_dir).await.unwrap();

    // 1. Generate TLS identities
    let server_ident = generate_self_signed().expect("Server TLS generation failed");
    let client_ident = generate_self_signed().expect("Client TLS generation failed");

    let server_config = AppConfig {
        alias: "HTTPS Bench Receiver".to_string(),
        port: 0,
        download_dir: server_download_dir.clone(),
        auto_accept: true,
        pin: None,
        ..Default::default()
    };
    let registry = PeerRegistry::new();
    let (event_tx, _event_rx) = mpsc::channel(100);

    let server_handle = start_server(server_config, Some(server_ident.clone()), registry, Some(event_tx))
        .await
        .expect("Server bind failed");
    let server_port = server_handle.bound_port();
    let base_url = format!("https://127.0.0.1:{server_port}");

    // Prepare 64 MB file
    let file_size = 64 * 1024 * 1024;
    let source_path = temp_dir.join("https_bench_64mb.bin");
    let expected_hash = create_benchmark_file(&source_path, file_size).await;

    let out_file = OutgoingFile::from_path(&source_path)
        .await
        .unwrap()
        .with_computed_sha256()
        .await
        .unwrap();

    // Build client with pinned server fingerprint and client cert
    let client_config = LsClientConfig {
        timeout: Duration::from_secs(60),
        connect_timeout: Duration::from_secs(10),
        tls_identity: Some(client_ident.clone()),
        target_fingerprint: Some(server_ident.fingerprint.clone()),
    };
    let client = LsClient::new(client_config).expect("Client creation failed");

    let sender_info = RegisterDtoV2 {
        alias: "HTTPS Bench Sender".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "BENCH_HTTPS_SENDER".to_string(),
        port: 53317,
        protocol: ProtocolType::Https,
        download: false,
    };

    let orchestrator = TransferOrchestrator::new(client, sender_info);
    let options = TransferOptions {
        max_concurrency: 1,
        chunk_size: 128 * 1024,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let start = Instant::now();
    orchestrator
        .transfer(&base_url, vec![out_file], options, None)
        .await
        .expect("mTLS Transfer must succeed");
    let elapsed = start.elapsed();

    // Verify bit-for-bit integrity
    let dest_file = server_download_dir.join("https_bench_64mb.bin");
    assert!(dest_file.exists());
    let dest_bytes = tokio::fs::read(&dest_file).await.unwrap();
    assert_eq!(dest_bytes.len(), file_size);
    let mut check_hasher = Sha256::new();
    check_hasher.update(&dest_bytes);
    let check_hash = format!("{:x}", check_hasher.finalize());
    assert_eq!(check_hash, expected_hash);

    println!("  Payload: 64 MB  | Time: {:>7.2?} | Throughput: {}", elapsed, format_throughput(file_size as u64, elapsed));
    server_handle.stop();
}

/// Benchmark 4: Parallel multi-file streaming transfer (4 files x 16 MB = 64 MB total).
async fn benchmark_parallel_multi_file_transfer(temp_dir: &Path) {
    println!("\n================================================================================");
    println!(" BENCHMARK 4: Concurrent Parallel Multi-File Transfer (4 x 16 MB = 64 MB)");
    println!("================================================================================");

    let server_download_dir = temp_dir.join("parallel_receiver");
    tokio::fs::create_dir_all(&server_download_dir).await.unwrap();

    let server_config = AppConfig {
        alias: "Parallel Bench Receiver".to_string(),
        port: 0,
        download_dir: server_download_dir.clone(),
        auto_accept: true,
        pin: None,
        ..Default::default()
    };
    let registry = PeerRegistry::new();
    let (event_tx, _event_rx) = mpsc::channel(100);

    let server_handle = start_server(server_config, None, registry, Some(event_tx))
        .await
        .expect("Server bind failed");
    let server_port = server_handle.bound_port();
    let base_url = format!("http://127.0.0.1:{server_port}");

    // Create 4 files of 16 MB each
    let file_size = 16 * 1024 * 1024;
    let mut out_files = Vec::new();
    let mut expected_hashes = HashMap::new();

    for i in 1..=4 {
        let f_name = format!("parallel_file_{i}.bin");
        let f_path = temp_dir.join(&f_name);
        let hash = create_benchmark_file(&f_path, file_size).await;
        expected_hashes.insert(f_name.clone(), hash);

        let out = OutgoingFile::from_path(&f_path)
            .await
            .unwrap()
            .with_computed_sha256()
            .await
            .unwrap();
        out_files.push(out);
    }

    let total_bytes = file_size as u64 * 4;
    let client = LsClient::new(LsClientConfig::default()).unwrap();
    let sender_info = RegisterDtoV2 {
        alias: "Parallel Bench Sender".to_string(),
        version: "2.0".to_string(),
        device_model: Some("Linux".to_string()),
        device_type: Some(DeviceType::Desktop),
        fingerprint: "BENCH_PARALLEL_SENDER".to_string(),
        port: 53317,
        protocol: ProtocolType::Http,
        download: false,
    };

    let orchestrator = TransferOrchestrator::new(client, sender_info);
    let options = TransferOptions {
        max_concurrency: 4,
        chunk_size: 128 * 1024,
        progress_interval: Duration::from_millis(50),
        ..Default::default()
    };

    let start = Instant::now();
    orchestrator
        .transfer(&base_url, out_files, options, None)
        .await
        .expect("Parallel transfer must succeed");
    let elapsed = start.elapsed();

    // Verify all 4 files
    for (f_name, expected_hash) in expected_hashes {
        let dest_file = server_download_dir.join(&f_name);
        assert!(dest_file.exists());
        let dest_bytes = tokio::fs::read(&dest_file).await.unwrap();
        assert_eq!(dest_bytes.len(), file_size);
        let mut check_hasher = Sha256::new();
        check_hasher.update(&dest_bytes);
        let check_hash = format!("{:x}", check_hasher.finalize());
        assert_eq!(check_hash, expected_hash);
    }

    println!("  Payload: 64 MB  | Time: {:>7.2?} | Aggregate Throughput: {}", elapsed, format_throughput(total_bytes, elapsed));
    server_handle.stop();
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let is_test_run = args.iter().any(|a| a == "--test");

    println!("================================================================================");
    println!(" LanFlare High-Throughput Streaming & Optimization Benchmark Suite");
    println!(" Mode: {}", if is_test_run { "Automated Test Verification" } else { "Standalone Benchmark" });
    println!(" OS: {} ({}) | Logical Cores: {}", std::env::consts::OS, std::env::consts::ARCH, num_cpus::get());
    println!("================================================================================");

    let temp_root = std::env::temp_dir().join(format!("lanflare_benchmarks_{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp_root).await.unwrap();

    benchmark_disk_writer_buffer_sizing(&temp_root).await;
    benchmark_end_to_end_plain_http_transfer(&temp_root).await;
    benchmark_end_to_end_https_mtls_transfer(&temp_root).await;
    benchmark_parallel_multi_file_transfer(&temp_root).await;

    let _ = tokio::fs::remove_dir_all(&temp_root).await;

    println!("\n================================================================================");
    println!(" ALL HIGH-THROUGHPUT BENCHMARKS COMPLETED SUCCESSFULLY");
    println!("================================================================================");
}

// Minimal fallback for num_cpus if not available as dependency
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}
