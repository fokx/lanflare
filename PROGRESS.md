# Operational Progress & Architecture Decision Record (ADR)

## Current Phase & Milestone
- **Current Milestone**: Phase 4: Standalone CLI Application (`lanflare-cli`)
- **Overall Status**: Phase 3 (3.1, 3.2, 3.3) complete. Ready for Phase 4.1 implementation.

---

## Completed Tasks
- [x] Analyze LocalSend protocol v2.2 specification (`/g/localsend.protocol`).
- [x] Analyze LocalSend core Rust implementation (`/g/localsend/packages/core`).
- [x] Evaluate Multicast UDP vs. mDNS (DNS-SD) tradeoffs and formulate dual-engine discovery design.
- [x] Create comprehensive system architecture document ([`ARCHITECTURE.md`](file:///f/lanflare/ARCHITECTURE.md)).
- [x] Create granular phased development roadmap with verification commands ([`ROADMAP.md`](file:///f/lanflare/ROADMAP.md)).
- [x] Initialize operational progress tracker ([`PROGRESS.md`](file:///f/lanflare/PROGRESS.md)).
- [x] Initialize Cargo workspace and crate scaffolding passing `cargo check`.
- [x] Implement LocalSend v2.2 protocol DTOs in `lanflare-proto`:
  - Discovery models: `MulticastMessageV2`, `RegisterDtoV2`, `RegisterResponseDtoV2`, `InfoResponseDtoV2`, `ProtocolType`, `DeviceType`.
  - Transfer models: `FileDto`, `FileMetadataDto`, `PrepareUploadRequestDtoV2`, `PrepareUploadResponseDtoV2`, `PrepareDownloadResponseDtoV2`.
  - Resilient serde deserialization with Section 7.1 `DeviceType` fallback to `Desktop` on unknown types and case-insensitive matching.
  - Unit tests and integration test suite (`crates/lanflare-proto/tests/spec_vectors.rs`) validating exact round-tripping against official LocalSend v2.2 specification test vectors.
- [x] Implement Crypto Primitives & Ephemeral TLS Identities in `lanflare-core::crypto`:
  - Self-signed X.509 certificate and private key generation via `rcgen` (`generate_self_signed`, `generate_self_signed_rsa`, `generate_self_signed_ed25519`) in [`crates/lanflare-core/src/crypto/cert.rs`](file:///f/lanflare/crates/lanflare-core/src/crypto/cert.rs).
  - SHA-256 fingerprint extraction from DER/PEM certificates as uppercase 64-character hex strings matching LocalSend v2.2 standard.
  - Custom `rustls` server certificate verifier ([`PinnedServerCertVerifier`](file:///f/lanflare/crates/lanflare-core/src/crypto/verifier.rs#L47-L121)) and client certificate verifier ([`PinnedClientCertVerifier`](file:///f/lanflare/crates/lanflare-core/src/crypto/verifier.rs#L123-L208)) authenticating against pinned SHA-256 fingerprints without external CA trust roots.
  - Comprehensive unit test suite (15 unit tests in `crypto::cert` and `crypto::verifier`) testing round-trip generation, X.509 signature verification, public key SPKI extraction, fingerprint pinning, case-insensitivity, and rejection of rogue/mismatched certificates.
- [x] Implement LocalSend UDP Multicast Discovery in `lanflare-core::discovery::multicast`:
  - Interface discovery and filtering ([`InterfaceFilter`](file:///f/lanflare/crates/lanflare-core/src/discovery/interface.rs#L11-L60), `local_interfaces`) supporting wildcard subnet matching (`192.168.1.*`) across IPv4 and IPv6 interfaces.
  - Dual-stack UDP multicast listener and sender on IPv4 `224.0.0.167:53317` and link-local IPv6 `ff12::fd3a:e420:53317` with `socket2` `SO_REUSEADDR` / `SO_REUSEPORT`.
  - Multicast announcement engine with randomized jitter (+50ms) and backoff burst (`100ms`, `500ms`, `2000ms`), plus loopback suppression by self-fingerprint.
  - Graceful cancellation and port release with Tokio cancellation tokens and JoinSets.
  - Unit and integration tests covering configuration defaults, DTO wire generation, multi-node announcement exchange, loopback suppression, and port re-binding.
- [x] Implement Modern mDNS (DNS-SD) Discovery Engine in `lanflare-core::discovery::mdns`:
  - Zero-configuration mDNS responder and browser advertising and browsing `_lanflare._tcp.local.` via `mdns-sd`.
  - RFC 6763 TXT record encoding containing `alias`, `version`, `model`, `devtype`, `fingerprint`, `protocol`, `port`, and `download`.
  - Full support for auto address binding, TTL expiration, graceful unregistration (`unregister`), and daemon shutdown (`stop`).
  - Multicast loopback suppression filtering out self TLS certificate fingerprints.
  - Unit and integration tests (6 tests in [`crates/lanflare-core/src/discovery/mdns.rs`](file:///f/lanflare/crates/lanflare-core/src/discovery/mdns.rs)) verifying TXT property serialization, `ServiceInfo` / `ResolvedService` conversion, single-node registration/shutdown, loopback suppression, and multi-node discovery/peer removal.
- [x] Implement Peer Aggregator & Registry in `lanflare-core::discovery::registry`:
  - Unified [`DiscoveredPeer`](file:///f/lanflare/crates/lanflare-core/src/discovery/registry.rs) representation unifying UDP Multicast, mDNS (DNS-SD), and direct HTTP registration.
  - Canonical uppercase SHA-256 fingerprint deduplication with case-insensitive lookup.
  - Multi-endpoint IP consolidation sorting addresses deterministically (IPv4 prioritized before IPv6) and tracking observation sources per endpoint.
  - Reactive `broadcast` event stream (`RegistryEvent::PeerAdded`, `RegistryEvent::PeerUpdated`, `RegistryEvent::PeerExpired`, `RegistryEvent::PeerRemoved`) with specific update reasons (`NewEndpoint`, `Heartbeat`, `MetadataChanged`, `NewDiscoverySource`).
  - Configurable inactivity timeout eviction and automated background sweep worker with capacity bounds.
  - Unit test suite (9 tests in [`crates/lanflare-core/src/discovery/registry.rs`](file:///f/lanflare/crates/lanflare-core/src/discovery/registry.rs)) verifying discovery ingestion, deduplication, address merging, case-insensitivity, timeout eviction, max capacity eviction, and background sweep.
- [x] Implement Secure Storage & Streaming I/O in `lanflare-core::storage`:
  - Path sanitization and directory traversal defense in [`crates/lanflare-core/src/storage/path.rs`](file:///f/lanflare/crates/lanflare-core/src/storage/path.rs) (`sanitize_filename`, `validate_filename`, `is_reserved_windows_name`, `resolve_destination_path`, `resolve_unique_destination_path`) neutralizing directory traversal (`../`), stripping control characters, rejecting null bytes, Windows Alternate Data Streams (`:`), forbidden characters (`<`, `>`, `"`, `|`, `?`, `*`), reserved OS device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`), trailing spaces/dots, and enforcing the 255-byte component limit.
  - Streaming disk writer in [`crates/lanflare-core/src/storage/writer.rs`](file:///f/lanflare/crates/lanflare-core/src/storage/writer.rs) (`StreamingFileWriter`) with 64KB buffering and concurrent in-memory SHA-256 hash calculation, writing to isolated temporary `.{filename}.{uuid}.lanflare-part` files on the same filesystem.
  - Atomic rename (`tokio::fs::rename`) on checksum and size match upon `commit()`.
  - Early size-exceeded bombing defense, explicit `abort()`, and automatic synchronous cleanup in `Drop` deleting uncommitted `.lanflare-part` temporary files on task failure, panic, or cancellation.
  - Comprehensive unit test suite (15 unit tests in `storage::path` and `storage::writer`) covering valid inputs, traversal attack neutralizations and rejections, collision resolution, 1-bit checksum corruption detection, size verification, early abort, drop cleanup, reader piping, and stream chunk piping.
- [x] Implement Hyper v1 HTTP/HTTPS Server in `lanflare-core::server`:
  - Session state machine and PIN defense in [`crates/lanflare-core/src/server/session.rs`](file:///f/lanflare/crates/lanflare-core/src/server/session.rs) (`SessionManager`, `PinTracker`, `ActiveUploadSession`, `SessionFile`, `FileTransferStatus`) enforcing single-session concurrency bounds (409 Conflict), per-file upload tokens, sender IP pinning, and 3-attempt brute-force rate-limiting lockout (429 Too Many Requests).
  - LocalSend v2.2 endpoint router in [`crates/lanflare-core/src/server/router.rs`](file:///f/lanflare/crates/lanflare-core/src/server/router.rs) (`route_request`, `ClientConnectionInfo`) dispatching:
    - `GET  /api/localsend/v2/info` (and `/api/localsend/v1/info`)
    - `POST /api/localsend/v2/register` (two-way discovery registration)
    - `POST /api/localsend/v2/prepare-upload` (session negotiation, PIN validation, file token generation)
    - `POST /api/localsend/v2/upload` (direct binary chunk streaming with `StreamingFileWriter` SHA-256 verification and atomic commit)
    - `POST /api/localsend/v2/cancel` (sender or receiver session abort with cleanup)
  - Hyper v1 HTTP/1.1 server engine in [`crates/lanflare-core/src/server/mod.rs`](file:///f/lanflare/crates/lanflare-core/src/server/mod.rs) (`start_server`, `ServerHandle`, `ServerState`, `ServerTransferEvent`) supporting optional TLS termination via `tokio-rustls` and client certificate verification (`PinnedClientCertVerifier`).
  - Unit and integration test suite (11 tests in `server::session` and `server::tests`) validating plain HTTP lifecycle, discovery registration events, prepare-upload token generation, streaming upload integrity, PIN lockout protection, session cancellation, and HTTPS mTLS client authentication.
- [x] Implement HTTP/HTTPS Client & Transfer Orchestrator in `lanflare-core::client`:
  - Asynchronous client ([`LsClient`](file:///f/lanflare/crates/lanflare-core/src/client/mod.rs), `LsClientConfig`, `ClientError`) using `reqwest` implementing LocalSend v2.2 endpoints (`get_info`, `register`, `prepare_upload`, `upload_file`, `cancel_session`).
  - Pre-configured `rustls::ClientConfig` integration with [`PinnedServerCertVerifier`](file:///f/lanflare/crates/lanflare-core/src/crypto/verifier.rs) enabling strict receiver SHA-256 fingerprint pinning and client mTLS identity presentation (`with_client_auth_cert`).
  - File preparation abstraction ([`OutgoingFile`](file:///f/lanflare/crates/lanflare-core/src/client/transfer.rs#L34-L100)) with asynchronous size probing, MIME type guessing, lazy SHA-256 pre-calculation, and conversion into `FileDto`.
  - Parallel transfer orchestrator ([`TransferOrchestrator`](file:///f/lanflare/crates/lanflare-core/src/client/transfer.rs#L218-L435), `TransferOptions`, `TransferProgress`) streaming file chunks directly from disk with rate-limited progress monitoring (`ProgressStream`), instantaneous speed calculations, ETA estimations, session cancellation propagation, and semaphore-bounded concurrency.
  - End-to-end integration test suite ([`crates/lanflare-core/tests/transfer_e2e.rs`](file:///f/lanflare/crates/lanflare-core/tests/transfer_e2e.rs)) verifying multi-file transfer over plain HTTP and mTLS with PIN protection between `TransferOrchestrator` and `start_server`.

---

## Architecture Decisions & Invariants (ADRs)

- **ADR-001: Cargo Workspace Structure**
  - *Context*: We need a reusable core library for both CLI and GUI, plus shared protocol definitions.
  - *Decision*: Adopt a multi-crate workspace: `lanflare-proto` (DTOs), `lanflare-core` (network, crypto, storage), `lanflare-cli` (binary), and `lanflare-desktop` (Tauri).
  - *Consequences*: Clean separation of concerns; enables independent testing and lean CLI deployment without GUI bloat.

- **ADR-002: Dual-Engine Discovery (LocalSend Multicast + Modern mDNS)**
  - *Context*: LocalSend uses UDP multicast (`224.0.0.167:53317`), which is often dropped on enterprise and guest networks. mDNS (`224.0.0.251:5353`) is widely reflected and supported by OS standard services.
  - *Decision*: Provide both discovery engines concurrently. LocalSend Multicast provides backward compatibility with official LocalSend apps, while mDNS provides zero-configuration discovery for LanFlare-native peers. Discovered peers are deduplicated by TLS certificate SHA-256 fingerprint.

- **ADR-003: Mutual TLS with Fingerprint Pinning (mTLS)**
  - *Context*: Local network peers are ad-hoc without public certificate authority (CA) hierarchies.
  - *Decision*: Generate self-signed certificates using `rcgen` and `rsa` (RSA-2048 with CN `LocalSend User` for full backwards compatibility, with Ed25519 available for LanFlare native peers). Peer authentication relies on pinning and verifying the SHA-256 fingerprint of the remote DER certificate against the fingerprint advertised during discovery or handshake.
  - *Invariants*:
    - The verifiers bypass DNS/hostname SAN checks (since local peers connect by IP), validating cryptographic self-signatures and valid timeframes via `x509-parser`.
    - Fingerprints are case-insensitively matched but formatted as uppercase 64-char hex strings in wire outputs.
    - Handshakes fail early if the peer's certificate does not match the pinned fingerprint, ensuring zero request metadata or file payload is transmitted to rogue peers.

- **ADR-004: Direct Streaming File Transfers with Incremental SHA-256**
  - *Context*: File sharing involves arbitrary sizes (from small photos to multi-gigabyte video or backup archives).
  - *Decision*: Files must stream directly from the HTTP socket to temporary disk storage (`.lanflare-part`) via buffered async streams. Compute SHA-256 concurrently during streaming and atomically rename to destination path only upon verification match. Never buffer complete files in RAM.

- **ADR-005: Strict Path Traversal Defense**
  - *Context*: Remote peers specify `fileName` in `prepare-upload`.
  - *Decision*: Sanitize all received filenames by extracting strictly the basename, stripping illegal path characters and path separators (`/`, `\`, null bytes, control codes), and confining outputs strictly within the configured download directory.

- **ADR-006: LocalSend v2.2 Wire Schema & Unknown Enum Fallback**
  - *Context*: Heterogeneous device ecosystems running varying versions of LocalSend or third-party clients may send unknown device types or optional fields.
  - *Decision*: Implement custom deserialization for `Option<DeviceType>` in `lanflare-proto::discovery::device_type_serde` that maps unknown strings to `DeviceType::Desktop` as required by Section 7.1 of the LocalSend protocol specification. All wire fields use camelCase naming, with lowercase protocol and device types.

- **ADR-007: Multicast Interface Enumeration & Loopback Filtering**
  - *Context*: Multi-homed machines (e.g. WiFi + Ethernet + VPN adapters) require separate sockets per interface to ensure outgoing multicast announcements egress through each physical adapter, and loopback packets must be received by peers on the same machine without self-discovery.
  - *Decision*: Bind one IPv4 UDP socket per valid IPv4 interface address and one IPv6 socket per interface index. Enable `multicast_loop_v4/v6(true)` to permit multiple local testing or client processes to see each other, but filter out datagrams carrying the local node's fingerprint in the receive loop.

- **ADR-008: mDNS Service Type and TXT Attribute Standardization**
  - *Context*: LanFlare native zero-configuration discovery must co-exist with existing local DNS-SD networks without conflict, and communicate all necessary peer metadata prior to HTTP handshake.
  - *Decision*: Standardize service type on `_lanflare._tcp.local.`. Standardize TXT keys on lowercase RFC 6763 attributes: `alias`, `version`, `model`, `devtype`, `fingerprint`, `protocol`, `port`, and `download`. Instance names use unique device alias or configured instance identifier. The engine suppresses self-discovered instances matching local certificate fingerprint.

- **ADR-009: Peer Deduplication and Multi-Homing Consolidation**
  - *Context*: A single physical peer may be discovered across both Multicast and mDNS, and may possess multiple IP addresses (e.g. Ethernet and Wi-Fi, dual-stack IPv4/IPv6).
  - *Decision*: All discovery sources feed into `PeerRegistry`, keyed canonicalized uppercase SHA-256 fingerprint. Incoming announcements merge IP endpoints into a sorted list (IPv4 before IPv6), update heartbeat timestamps, and unify discovery source bitflags without losing historical endpoint metadata.
  - *Invariants*:
    - Primary IP selection prefers active IPv4 endpoints before IPv6 endpoints.
    - Capacity is bounded by `max_peers`, evicting the least recently seen peer if full.
    - Event channel broadcasts fine-grained lifecycle notifications (`PeerAdded`, `PeerUpdated`, `PeerExpired`, `PeerRemoved`).

- **ADR-010: Path Sanitization & Anti-Traversal Defense**
  - *Context*: Remote peers transmit untrusted file names via `PrepareUploadRequestDtoV2`. Unsanitized strings could overwrite critical operating system files, escape download directories, or exploit Windows-specific file system semantics (ADS, reserved DOS names).
  - *Decision*: Enforce dual layers: `validate_filename` for strict upfront rejection and `sanitize_filename` for safe normalization.
  - *Invariants*:
    - Null bytes (`\0`) and ASCII control characters are rejected immediately.
    - Directory separators (`/`, `\`) trigger basename extraction; pure traversal sequences (`.`, `..`) are rejected with `PathSanitizeError::PathTraversal`.
    - Windows Alternate Data Streams (`:`) are rejected with `PathSanitizeError::AlternateDataStream`.
    - Reserved DOS device stems (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`) regardless of extension or casing are rejected with `PathSanitizeError::ReservedDeviceName`.
    - Forbidden characters (`<`, `>`, `"`, `|`, `?`, `*`) are replaced with `_`.
    - Trailing spaces and dots are stripped to prevent Windows filesystem quirks.
    - Resolved destination paths must have `base_dir` as their immediate parent.

- **ADR-011: Streaming Disk Writer Lifecycle & RAII Temporary Part Cleanup**
  - *Context*: High-throughput streaming writes must protect system memory, verify cryptographic hashes, and prevent partial/corrupted files from lingering on disk upon transfer failure, cancellation, or crash.
  - *Decision*: Implement `StreamingFileWriter` with 64KB asynchronous buffering.
  - *Invariants*:
    - Temporary files are created in the same parent directory as the final target under the naming convention `.{filename}.{uuid}.lanflare-part`, guaranteeing that `tokio::fs::rename` is an atomic, same-filesystem inode swap.
    - Incoming chunks incrementally update `Sha256` digest in RAM without re-reading bytes from disk.
    - If `expected_size` is exceeded during writes, writing aborts immediately with `StorageError::SizeExceeded` to prevent disk bombing.
    - If either size or SHA-256 hash mismatches on `commit()`, the temporary file is deleted and `commit()` errors out.
    - Uncommitted writer instances automatically delete their `.lanflare-part` file in `Drop`, ensuring clean recovery on async cancellation or error propagation.

- **ADR-012: LocalSend v2 Server Session State & PIN Throttling Architecture**
  - *Context*: The receiver must accept incoming transfers without allowing simultaneous conflicting transfer streams to corrupt memory or disk, while preventing PIN brute-forcing over LAN.
  - *Decision*: Implement a dedicated `SessionManager` and `PinTracker` integrated with Hyper v1.
  - *Invariants*:
    - Transfer concurrency follows LocalSend v2 spec Section 4.1: exactly one transfer session can be pending or active in the slot at any time. Any overlapping `prepare-upload` yields HTTP 409 Conflict.
    - Upload security strictly binds the `sessionId`, `fileId`, `token`, and client remote IP address. Any mismatch yields HTTP 403 Forbidden.
    - PIN validation limits consecutive failed attempts per IP to 3 (`MAX_PIN_ATTEMPTS`). After 3 failed attempts, requests from that IP receive HTTP 429 Too Many Requests until reset.
    - Streaming uploads write directly to disk with `StreamingFileWriter`, failing with HTTP 422 Unprocessable Entity on checksum mismatch, or HTTP 400 Bad Request on size mismatch.
    - Cancellation via `POST /api/localsend/v2/cancel` immediately revokes pending tokens, unarms active session guards, and drops any active streams.

- **ADR-013: Client Transfer Pipeline, Fingerprint Pinning & Parallel Stream Dispatch**
  - *Context*: File senders must reliably discover receiver capabilities, authenticate untrusted receiver self-signed certificates without external CA hierarchies, handle optional PIN codes, and stream multi-gigabyte payloads concurrently without buffering entire files in memory.
  - *Decision*: Implement `LsClient` and `TransferOrchestrator` using `reqwest` backed by `rustls::ClientConfig::use_preconfigured_tls` with `PinnedServerCertVerifier`.
  - *Invariants*:
    - In mTLS mode, client presents its `TlsIdentity` certificate and private key (`CertificateDer`, `PrivateKeyDer`) and verifies receiver's certificate SHA-256 fingerprint against the pinned target. Handshakes terminate immediately if the receiver's fingerprint does not match.
    - `TransferOrchestrator` negotiates `POST /api/localsend/v2/prepare-upload` first. If receiver returns 401/403 (invalid PIN), 429 (PIN rate limit), or 409 (busy slot), the client yields specific, actionable typed errors (`ClientError::UnauthorizedPin`, `ClientError::PinRateLimited`, `ClientError::SessionConflict`).
    - Accepted files are transferred concurrently with a semaphore bounding parallelism (`max_concurrency`).
    - File bodies are streamed directly from `tokio::fs::File` using `ReaderStream` wrapped with `ProgressStream`.
    - Progress reports emit bytes sent, instantaneous speed, and estimated time of arrival (ETA) throttled by `progress_interval` to prevent UI thread flooding.
    - Cancellation via cancellation token or failure of any upload stream triggers `POST /api/localsend/v2/cancel?sessionId=...` to ensure receiver immediately releases pending locks and cleans up incomplete parts.

---

## Known Debt & Blockers
- *None at present.*

---

## Next Session Target
- **Target**: **Phase 4.1: Command-Line Interface & Scanner (`lanflare-cli`)**
- **Action Items**:
  1. Inspect [`crates/lanflare-cli`](file:///f/lanflare/crates/lanflare-cli).
  2. Implement CLI argument structure using `clap` (commands: `status`, `scan`, `send`, `receive`).
  3. Wire `scan` to active discovery engines (UDP multicast and mDNS) displaying pretty-printed peer table with OS icons, IP, protocol mode, and fingerprint.
  4. Verify with `cargo run -p lanflare-cli -- --help` and `cargo test -p lanflare-cli`.


