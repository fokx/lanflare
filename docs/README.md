# LanFlare Implementation & Developer Guide

Welcome to the internal implementation documentation for **LanFlare**. This directory contains in-depth technical documentation designed for advanced users, system architects, and developers who wish to understand the internal mechanics, contribute to the codebase, or integrate LanFlare crates into their own systems.

---

## 📑 Documentation Index

1. **[System Architecture](architecture.md)**
   - Workspace crate topology (`lanflare-proto`, `lanflare-core`, `lanflare`, `lanflare-desktop`)
   - Async threading model and Tokio runtime integration
   - Ephemeral TLS cryptographic model and certificate fingerprinting
   - Security boundaries and attack surface mitigations

2. **[Protocol & Wire Specification](protocol.md)**
   - LocalSend v2.2 wire protocol implementation
   - REST control endpoints (`/api/localsend/v2/...`)
   - DTO serialization models and camelCase wire compliance
   - PIN negotiation state machine and security flows
   - Reverse file transfer (browser download) protocol

3. **[Hybrid Discovery Engine](discovery.md)**
   - UDP multicast discovery (`224.0.0.167:53317`)
   - Modern mDNS (DNS-SD) service registration and browsing (`_lanflare._tcp.local.`)
   - Concurrent `/24` IPv4 subnet scanner fallback for restrictive Wi-Fi / Android AP isolation
   - Dynamic interface filtering, loopback suppression, and route scoring
   - Unified `PeerRegistry` deduplication, address aggregation, and eviction

4. **[Streaming Storage & Security Engine](storage.md)**
   - Direct socket-to-disk streaming pipeline (zero complete buffering in RAM)
   - Real-time SHA-256 calculation during stream consumption
   - Atomic rename pattern (`.{filename}.{uuid}.lanflare-part`)
   - Automatic `Drop` cleanup and async cancellation recovery
   - Directory traversal, absolute path attacks, and disk-bombing defense

5. **[Performance & Benchmarks](benchmarks.md)**
   - Loopback transfer benchmarks (>2.2 Gbps streaming throughput)
   - Buffer tuning analysis (128 KB chunk optimization)
   - Memory consumption under concurrent high-speed streams
   - Fuzz boundary test suite and malformed packet resilience

6. **[Desktop & Tauri v2 Integration](desktop-gui.md)**
   - Tauri v2 IPC bridge architecture
   - Non-blocking Tokio async runtime bridge
   - Frontend: SvelteKit 2 and Svelte 5 Runes reactivity
   - Mock API layer for browser development
   - Native OS integration (system tray, desktop notifications, dialogs)

---

## 🏗️ Repository Layout

```text
lanflare/
├── Cargo.toml                  # Workspace manifest and centralized dependencies
├── crates/
│   ├── lanflare-proto/         # LocalSend v2.2 protocol DTOs and serialization
│   ├── lanflare-core/          # Core discovery, crypto, HTTP server/client, storage
│   ├── lanflare/               # Terminal application, TUI renderer, and CLI binary
│   └── lanflare-desktop/       # Tauri v2 native desktop shell & IPC handlers
├── web/                        # SvelteKit 2 + Svelte 5 desktop frontend application
├── docs/                       # Developer & implementation technical documentation
└── tests/                      # End-to-end integration, fuzz, and compatibility suites
```

---

## 🛠️ Building & Testing from Source

### Prerequisites
- **Rust**: 1.85+ (Edition 2024 support)
- **Node.js**: 20+ and `pnpm` (only for building the desktop GUI)
- **C Compiler**: `clang` or `gcc`
- **Linux Packages** (for desktop GUI): `libwebkit2gtk-4.1-dev`, `libayatana-appindicator3-dev`, `librsvg2-dev`

### Running Tests
```bash
# Run all workspace unit and integration tests
cargo test --workspace

# Run protocol specification compliance tests
cargo test -p lanflare-proto

# Run high-throughput transfer benchmarks
cargo test -p lanflare-core --bench transfer_benchmark -- --nocapture

# Run fuzz and boundary security tests
cargo test -p lanflare-core --test fuzz_boundary_tests
```

### Building Release Binaries
```bash
# Build CLI binary (optimized with LTO and strip)
cargo build --release -p lanflare

# Run CLI directly
./target/release/lanflare --help
```
