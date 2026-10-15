# lanflare-core

[![Crates.io](https://img.shields.io/crates/v/lanflare-core.svg)](https://crates.io/crates/lanflare-core)
[![Documentation](https://docs.rs/lanflare-core/badge.svg)](https://docs.rs/lanflare-core)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

High-performance, async engine for local peer discovery, ephemeral mTLS identity generation, streaming I/O, and file transfer orchestration. Powering [LanFlare](https://github.com/fokx/lanflare) and fully interoperable with official **LocalSend v2.2** clients.

## Key Subsystems

- **Hybrid Discovery Engine**:
  - UDP Multicast (`224.0.0.167:53317`) announcement and listener.
  - Modern mDNS (DNS-SD) service advertiser and browser (`_lanflare._tcp.local.`).
  - Active `/24` subnet scanning fallback for restrictive networks and Android AP isolation.
  - Unified `PeerRegistry` with deduplication, address aggregation, and automatic stale eviction.
- **Security & Ephemeral Crypto**:
  - Fast on-the-fly Ed25519 / RSA self-signed X.509 certificate generation via `rcgen`.
  - Trust-On-First-Use (TOFU) and pinned SHA-256 certificate fingerprint verification.
  - Hyper v1 HTTP/HTTPS mTLS server and client orchestrators.
- **Streaming Storage Engine**:
  - Direct socket-to-disk buffered pipeline (128 KB buffer profile) with >2.2 Gbps throughput.
  - Concurrent on-the-fly SHA-256 computation during streaming.
  - Atomic rename (`.{filename}.{uuid}.lanflare-part`) preventing incomplete or corrupt files.
  - Robust security: path traversal sanitization, reserved name neutralization, and disk bombing limits.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
lanflare-core = "0.3.0"
```

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0)).
