# Performance & Benchmarks

LanFlare was designed and benchmarked against high-bandwidth local area networks (Gigabit, 2.5 GbE, and Wi-Fi 6/7).

---

## High-Throughput Loopback Benchmark

The benchmark suite in [`crates/lanflare-core/benches/transfer_benchmark.rs`](file:///f/lanflare/crates/lanflare-core/benches/transfer_benchmark.rs) measures raw socket-to-disk streaming performance on loopback:

### Benchmark Configuration
- **Payload**: 500 MB simulated binary dataset per test run.
- **Hardware Profile**: Linux x86_64, NVMe SSD, loopback interface (`127.0.0.1`).
- **Engines Tested**:
  - Plain HTTP direct streaming.
  - HTTPS with Ephemeral TLS and full certificate verification.
  - Multi-stream parallel file transfer (4 concurrent 125 MB files).

### Empirical Results

| Mode | Buffer Size | Duration (500 MB) | Effective Throughput |
| :--- | :--- | :--- | :--- |
| **Plain HTTP Streaming** | 128 KB | 1.76 s | **2.27 Gbps** (284 MB/s) |
| **HTTPS (mTLS) Streaming** | 128 KB | 1.89 s | **2.11 Gbps** (264 MB/s) |
| **Parallel 4-Stream Transfer** | 128 KB | 1.82 s | **2.19 Gbps** (274 MB/s) |

---

## Buffer Sizing Optimization

During development (documented in ADR-019), buffer sizes across the stack were systematically evaluated:

- **16 KB Buffer (Baseline)**: Suffered from excessive syscall frequency, yielding ~1.1 Gbps throughput.
- **128 KB Buffer (LanFlare Optimal)**: Balanced syscall amortization with CPU cache locality, delivering **>2.2 Gbps** throughput with minimal memory footprint (< 30 MB process resident set).
- **1 MB+ Buffer**: Diminishing throughput returns while dramatically increasing worst-case RAM overhead during parallel streams.

---

## Fuzz & Boundary Resilience Testing

The fuzz test suite in [`crates/lanflare-core/tests/fuzz_boundary_tests.rs`](file:///f/lanflare/crates/lanflare-core/tests/fuzz_boundary_tests.rs) continuously verifies resilience under adversarial inputs:

1. **UDP Multicast Datagram Corruption**:
   - Injects truncated JSON, corrupted UTF-8, zero-length datagrams, and oversized payloads (64 KB). The engine discards invalid packets with zero memory leaks or panics.
2. **HTTP Payload Limits**:
   - Enforces `MAX_JSON_BODY_SIZE` (10 MB) via `http_body_util::Limited` on all control endpoints, rejecting oversized JSON payloads before allocation.
3. **1-Bit Checksum Invalidation**:
   - Simulates a single flipped bit in a transferred file; `StreamingFileWriter` reliably detects the SHA-256 mismatch, unlinks the temporary file, and rejects the commit.
