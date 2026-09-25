# LanFlare Development Roadmap & Verification Plan

This roadmap partitions the project into atomic, independently verifiable sub-phases. Each sub-phase defines explicit deliverables, acceptance criteria, and exact verification commands.

---

## Phase 1: Protocol Foundations & Core Cryptography

### Sub-phase 1.1: Protocol DTOs & Serialization (`lanflare-proto`)
- **Scope**:
  - Implement LocalSend v2.2 compatible protocol models: `MulticastMessageV2`, `RegisterDtoV2`, `PrepareUploadRequestDtoV2`, `PrepareUploadResponseDtoV2`, `FileDto`, `InfoResponseDtoV2`.
  - Serde schemas with exact casing, optional fields, and type aliases.
  - JSON round-trip compatibility unit tests using fixture samples from LocalSend specifications.
- **Acceptance Criteria**:
  - Zero serialization discrepancies compared to `/g/localsend.protocol/README.md`.
  - Serde deserialization handles optional and extra fields gracefully.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-proto
  ```

### Sub-phase 1.2: Crypto Primitives & Ephemeral TLS Identities (`lanflare-core::crypto`)
- **Scope**:
  - RSA-2048 and Ed25519 self-signed X.509 certificate generation using `rcgen`.
  - SHA-256 fingerprint extraction from DER certificates formatted as uppercase hex strings.
  - Custom `rustls` server and client certificate verifiers that validate against known peer fingerprints instead of CA trust roots.
- **Acceptance Criteria**:
  - Generated certificates parse cleanly with `x509-parser`.
  - Verifier permits connections matching announced fingerprint and rejects rogue certificates with mismatch errors.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib crypto
  ```

---

## Phase 2: Local Discovery Engines & Peer Registry

### Sub-phase 2.1: LocalSend UDP Multicast Discovery (`lanflare-core::discovery::multicast`)
- **Scope**:
  - Dual-stack UDP multicast listener on IPv4 `224.0.0.167:53317` and IPv6 `ff12::fd3a:e420`.
  - Multicast announcement burst with jitter and backoff (`100ms`, `500ms`, `2000ms`).
  - Loopback suppression filtering out self-fingerprint packets.
- **Acceptance Criteria**:
  - Two simulated nodes in tests discover each other via virtual socket exchange within 1 second.
  - Socket binding handles port re-use safely via `SO_REUSEADDR` / `SO_REUSEPORT`.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib discovery::multicast
  ```

### Sub-phase 2.2: Modern mDNS (DNS-SD) Discovery Engine (`lanflare-core::discovery::mdns`)
- **Scope**:
  - Zero-configuration mDNS responder & browser advertising `_lanflare._tcp.local.`.
  - TXT record encoding containing device alias, protocol (`http`/`https`), port, and TLS fingerprint.
  - Periodic browsing and service expiration handling.
- **Acceptance Criteria**:
  - Can register and query local services across loopback and physical interfaces without crashing.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib discovery::mdns
  ```

### Sub-phase 2.3: Peer Aggregator & Registry (`lanflare-core::discovery::registry`)
- **Scope**:
  - Unified thread-safe `PeerRegistry` that deduplicates devices discovered across both Multicast and mDNS via device fingerprint.
  - Heartbeat tracking and stale peer timeout eviction.
- **Acceptance Criteria**:
  - Devices found on both channels merge into a single verified entry.
  - Inactive devices are marked stale after timeout.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib discovery::registry
  ```

---

## Phase 3: Transfer Engine & HTTP Server/Client

### Sub-phase 3.1: Secure Storage & Streaming I/O (`lanflare-core::storage`)
- **Scope**:
  - Path sanitization engine to block directory traversal attacks (`../`, absolute paths, Windows alternate streams).
  - Stream writer writing to temporary `.part` files while computing SHA-256 in real-time.
  - Atomic rename upon verification; cleanup on failure or cancellation.
- **Acceptance Criteria**:
  - Malicious paths (`../../etc/passwd`, `C:\Windows\System32`) are completely rejected or neutralized.
  - Checksum verification detects 1-bit corruption and rejects file with error code.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib storage
  ```

### Sub-phase 3.2: Hyper v1 Server Implementation (`lanflare-core::server`)
- **Scope**:
  - Hyper v1 HTTP/HTTPS server with TLS termination via `rustls`.
  - Endpoints:
    - `POST /api/localsend/v2/register`
    - `POST /api/localsend/v2/prepare-upload`
    - `POST /api/localsend/v2/upload`
    - `POST /api/localsend/v2/cancel`
    - `GET  /api/localsend/v2/info`
  - PIN protection (`?pin=...`) and session state machine.
- **Acceptance Criteria**:
  - Successfully handles prepare-upload, assigns file tokens, and accepts binary uploads.
  - Cancelling a session cleanly aborts ongoing file streams and deletes incomplete files.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib server
  ```

### Sub-phase 3.3: HTTP/HTTPS Client & Transfer Orchestrator (`lanflare-core::client`)
- **Scope**:
  - Async client sending files to LocalSend or LanFlare peers.
  - Progress reporting channel emitting bytes transferred, instantaneous transfer speed, and completion status.
  - Parallel file uploads for multiple files within the same session.
- **Acceptance Criteria**:
  - Inter-node transfer test sends multi-megabyte payloads in parallel and verifies integrity.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --test transfer_e2e
  ```

---

## Phase 4: Standalone CLI Application (`lanflare-cli`)

### Sub-phase 4.1: Command-Line Interface & Scanner
- **Scope**:
  - Commands: `lanflare status`, `lanflare scan`, `lanflare send`, `lanflare receive`.
  - Pretty-printed peer table with OS icons, IP, protocol mode, and fingerprint.
- **Acceptance Criteria**:
  - `cargo run -p lanflare-cli -- --help` prints usage with zero warnings or errors.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-cli
  ```

### Sub-phase 4.2: Interactive TUI & Transfer Progress
- **Scope**:
  - Multi-bar progress display using `indicatif` for ongoing file transfers.
  - Interactive file selection and peer picker.
- **Acceptance Criteria**:
  - Can send files to an official LocalSend receiver and receive files from it via CLI.
- **Verification Command**:
  ```bash
  cargo run -p lanflare-cli -- receive --help
  ```

---

## Phase 5: Desktop GUI with Tauri v2 + SvelteKit 2

### Sub-phase 5.1: Tauri v2 Core Integration (`lanflare-desktop`)
- **Scope**:
  - Tauri v2 app wrapper initialized with system tray, window management, and native notifications.
  - Tauri IPC commands bridging GUI to `lanflare-core` (discovery, transfers, configuration).
- **Acceptance Criteria**:
  - `cargo check -p lanflare-desktop` passes.
- **Verification Command**:
  ```bash
  cargo check -p lanflare-desktop
  ```

### Sub-phase 5.2: SvelteKit 2 + Svelte 5 Frontend (`web/`)
- **Scope**:
  - Modern Svelte 5 runes (`$state`, `$derived`, `$effect`) reactive state management.
  - Responsive views: Radar/Device Grid, Transfer Queue with speedometers, Settings dialog.
  - Drag-and-drop file receiver and transfer approval prompt.
- **Acceptance Criteria**:
  - Web UI compiles cleanly with Vite (`pnpm build`).
- **Verification Command**:
  ```bash
  cd web && pnpm build
  ```

---

## Phase 6: E2E Compatibility, Performance & Hardening

### Sub-phase 6.1: Cross-Protocol Interoperability Suite
- **Scope**:
  - Automated integration test suite running LanFlare against real LocalSend binaries / protocol mock harnesses.
  - Reverse file transfer (browser download API: `/api/localsend/v2/prepare-download` & `/api/localsend/v2/download`).
- **Acceptance Criteria**:
  - 100% test pass rate across bidirectional transfers.
- **Verification Command**:
  ```bash
  cargo test --test localsend_compat
  ```

### Sub-phase 6.2: High-Throughput Stress Testing & Optimization
- **Scope**:
  - 10Gbps local throughput benchmarking, buffer sizing, zero-copy optimizations.
  - Memory leak verification and fuzz testing on malformed HTTP and UDP packets.
- **Acceptance Criteria**:
  - Sustained wire speeds matching or exceeding standard HTTP transfer benchmarks without runaway memory usage.
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --bench transfer_benchmark
  ```
