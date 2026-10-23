# Operational Progress & Architecture Decision Record (ADR)

## Current Phase & Milestone
- **Current Milestone**: Sub-phase 8.1 Complete; Ready for Sub-phase 8.2: Persistent Contact Book, Conversation Store & Delay-Tolerant Outbox.
- **Overall Status**: Ready for **Sub-phase 8.2: Persistent Contact Book, Conversation Store & Delay-Tolerant Outbox (`lanflare-core::chat::store` & `lanflare-core::chat::outbox`)**.

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

- [x] Implement Command-Line Interface & Scanner in `lanflare-cli` (Phase 4.1):
  - Structured CLI CLI/subcommand argument parser using `clap` in [`crates/lanflare-cli/src/lib.rs`](file:///f/lanflare/crates/lanflare-cli/src/lib.rs) and [`crates/lanflare-cli/src/main.rs`](file:///f/lanflare/crates/lanflare-cli/src/main.rs): `status`, `scan`, `send`, `receive`.
  - Pretty-printed peer table formatting in [`crates/lanflare-cli/src/tui.rs`](file:///f/lanflare/crates/lanflare-cli/src/tui.rs) with OS/device icons (🐧 Linux, 🪟 Windows, 🍎 macOS, 🤖 Android, 📱 iOS), IP address, port, transport mode, and shortened fingerprint.
  - Active scanning command handler (`lanflare scan`) in [`crates/lanflare-cli/src/commands.rs`](file:///f/lanflare/crates/lanflare-cli/src/commands.rs) wiring dual discovery engines (UDP multicast and mDNS) with `PeerRegistry` aggregation and burst announcement triggers.
  - Local node status reporter (`lanflare status`) enumerating active IPv4/IPv6 network interfaces and ephemeral/configured cryptographic TLS fingerprints.
- [x] Implement Interactive TUI & Transfer Progress in `lanflare-cli` (Phase 4.2):
  - Multi-bar progress display using `indicatif` (`TransferProgressTracker`) in [`crates/lanflare-cli/src/tui.rs`](file:///f/lanflare/crates/lanflare-cli/src/tui.rs) rendering overall and per-file transfer progress, elapsed time, transfer speed, and ETA.
  - Target peer resolution and interactive terminal picker (`match_target_peer`, `discover_or_select_target`) supporting query by canonical fingerprint prefix, direct IP address, exact alias, or case-insensitive substring.
  - Wired `lanflare send` to `TransferOrchestrator` with pre-flight file metadata extraction, SHA-256 pre-calculation, `LsClient` with pinned receiver TLS fingerprint verification, and reactive progress pump.
  - Wired `lanflare receive` to `start_server` daemon loop handling incoming transfer requests (`ServerTransferEvent::PrepareUpload`) with auto-accept or interactive terminal confirmation prompt (`[Y/n]`), PIN validation, and streaming disk persistence.
- [x] Implement Tauri v2 Core Integration in `lanflare-desktop` (Phase 5.1):
  - Desktop crate scaffolding initialized in [`crates/lanflare-desktop`](file:///f/lanflare/crates/lanflare-desktop) with Tauri v2 (`tauri` v2.12.3, `tauri-build` v2.7.2) and official plugins (`notification`, `dialog`, `fs`, `opener`, `shell`).
  - System tray icon and context menu implementation in [`crates/lanflare-desktop/src/tray.rs`](file:///f/lanflare/crates/lanflare-desktop/src/tray.rs) with window toggle, show/hide, and graceful exit handlers.
  - Desktop state management in [`crates/lanflare-desktop/src/state.rs`](file:///f/lanflare/crates/lanflare-desktop/src/state.rs) managing thread-safe runtime instances of `AppConfig`, `TlsIdentity`, `PeerRegistry`, `ServerHandle`, and active/pending transfer sessions.
  - Tauri IPC commands bridging GUI to `lanflare-core` in [`crates/lanflare-desktop/src/commands.rs`](file:///f/lanflare/crates/lanflare-desktop/src/commands.rs):
    - Node status & configuration: `get_node_status`, `update_config`.
    - Discovery orchestration: `get_discovered_peers`, `start_discovery`, `announce_presence`, `stop_discovery`.
    - Receiver daemon: `start_receiver_server`, `respond_incoming_transfer`, `stop_receiver_server`.
    - Sender orchestration: `send_files`, `cancel_outgoing_transfer`.
  - Tauri configuration ([`tauri.conf.json`](file:///f/lanflare/crates/lanflare-desktop/tauri.conf.json)) configured for SvelteKit 2 frontend output distribution (`../../web/build`).
  - Unit and integration test suite (`tests/desktop_tests.rs`) validating state generation and thread-safe configuration mutation.
- [x] Implement SvelteKit 2 + Svelte 5 Frontend in `web/` (Phase 5.2):
  - Initialized SvelteKit 2 and Svelte 5 frontend with Vite, TypeScript, TailwindCSS, `@sveltejs/adapter-static` for Tauri single-page export (`web/build/index.html`), and `lucide-svelte` icons.
  - Implemented reactive app state class (`AppState` in [`web/src/lib/state.svelte.ts`](file:///f/lanflare/web/src/lib/state.svelte.ts)) using modern Svelte 5 runes (`$state`, `$derived.by`) with typed IPC bridge wrappers (`web/src/lib/api.ts`) supporting both native Tauri webview and browser mock fallbacks.
  - Subscribed to real-time Tauri IPC events: `lanflare://peers-updated`, `lanflare://incoming-transfer-request`, `lanflare://incoming-transfer`, `lanflare://file-started`, `lanflare://file-received`, and `lanflare://transfer-progress`.
  - Built responsive views and modals:
    - Radar / Discovered Devices Grid ([`DeviceGrid.svelte`](file:///f/lanflare/web/src/lib/components/DeviceGrid.svelte)) displaying peers with device icons, endpoint badges, mTLS verification tags, search filter, and quick file send triggers.
    - Transfer Queue ([`TransferQueue.svelte`](file:///f/lanflare/web/src/lib/components/TransferQueue.svelte)) with per-file and session-level progress bars, speedometer indicators, ETA estimations, and session cancellation handlers.
    - Incoming Transfer Approval Modal ([`IncomingTransferModal.svelte`](file:///f/lanflare/web/src/lib/components/IncomingTransferModal.svelte)) with file review, total size calculation, and accept/decline actions.
    - Settings Modal ([`SettingsModal.svelte`](file:///f/lanflare/web/src/lib/components/SettingsModal.svelte)) configuring alias, port, download folder path, auto-accept toggle, and security PIN code.
    - Outgoing File Picker Dialog ([`SendFileDialog.svelte`](file:///f/lanflare/web/src/lib/components/SendFileDialog.svelte)) using `@tauri-apps/plugin-dialog` file open picker.
  - Verified static asset generation and compilation passing `pnpm build`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all`.
- [x] Implement Reverse File Transfer & Cross-Protocol Interoperability Suite (Phase 6.1):
  - Implemented reverse file transfer / browser download API in `lanflare-core::server` (`DownloadManager`, `DownloadFile`, `DownloadSession`):
    - `POST /api/localsend/v2/prepare-download`: session negotiation with optional PIN validation and cached session re-fetch for page reloads without re-entering PIN.
    - `GET /api/localsend/v2/download`: binary chunk streaming with `Content-Disposition`, MIME header, and session validation.
    - Updated `GET /api/localsend/v2/info` and `POST /api/localsend/v2/register` to advertise `download: true` when files are offered.
  - Extended `lanflare-core::client` (`LsClient`):
    - Added `prepare_download` calling `POST /api/localsend/v2/prepare-download`.
    - Added `download_file` calling `GET /api/localsend/v2/download?sessionId=...&fileId=...`.
  - Created automated cross-protocol interoperability test harness in [`crates/lanflare-core/tests/localsend_compat.rs`](file:///f/lanflare/crates/lanflare-core/tests/localsend_compat.rs):
    - `MockLocalSendServer` simulating official LocalSend v2 mobile/desktop app endpoints.
    - `test_lanflare_sender_to_official_localsend_receiver_interop`: verified multi-file transfer to official LocalSend mock receiver with exact bit-for-bit payload match.
    - `test_lanflare_sender_to_official_localsend_pin_negotiation`: verified PIN rejection (HTTP 401 UnauthorizedPin) and successful session execution upon correct PIN entry.
    - `test_lanflare_reverse_file_transfer_browser_download_api`: tested reverse download API round-trip, PIN protection, session re-fetch, SHA-256 verification, invalid token rejection, and clearing offered files.
    - `test_https_mtls_fingerprint_validation_parity`: validated strict certificate SHA-256 fingerprint verification and instant rejection of rogue certificates.
  - Verified test pass rate: 100% (78 unit/integration tests passing across workspace).
- [x] Implement High-Throughput Stress Testing & Optimization (Phase 6.2):
  - Implemented local loopback benchmark suite in [`crates/lanflare-core/benches/transfer_benchmark.rs`](file:///f/lanflare/crates/lanflare-core/benches/transfer_benchmark.rs) measuring multi-gigabit streaming transfers across 4 benchmark scenarios:
    - Benchmark 1: `StreamingFileWriter` buffer sizing profile (64KB, 128KB, 256KB, 512KB, 1024KB) with SHA-256 calculation.
    - Benchmark 2: End-to-end plain HTTP streaming transfer over loopback TCP (64 MB file).
    - Benchmark 3: End-to-end HTTPS (mTLS) with pinned SHA-256 fingerprint verification (64 MB file).
    - Benchmark 4: Concurrent parallel multi-file transfer (4 x 16 MB = 64 MB), achieving **263.78 MB/s (2.21 Gbps)** aggregate throughput in unoptimized debug mode.
  - Profiled buffer sizing and zero-copy streaming:
    - Increased default disk buffer capacity in `StreamingFileWriter` to 128 KB (`DEFAULT_BUFFER_CAPACITY`) and exposed `create_with_capacity`.
    - Replaced 16KB stack buffer in `write_all_from_reader` with 128 KB heap buffer.
    - Added configurable `chunk_size` (default: 128 KB) to `TransferOptions` and wired to `ReaderStream::with_capacity` in client transfer loop.
    - Configured Hyper HTTP/1.1 connection builder with `max_buf_size(128 * 1024)`.
  - Implemented fuzz testing & boundary verification suite in [`crates/lanflare-core/tests/fuzz_boundary_tests.rs`](file:///f/lanflare/crates/lanflare-core/tests/fuzz_boundary_tests.rs):
    - UDP multicast fuzzing: arbitrary random binary bytes, truncated JSON, deeply nested structures, invalid types/ports, and survival under rapid 50-packet garbage blasts.
    - HTTP request boundary hardening: enforced 10 MB limit (`MAX_JSON_BODY_SIZE`) using `http_body_util::Limited` on `/register` and `/prepare-upload` to defend against memory exhaustion attacks.
    - Path traversal attack neutralization during `prepare-upload` and `upload`.
    - Disk-space bombing defense: early abort and automatic `.lanflare-part` cleanup when uploaded bytes exceed promised file size.
    - Missing and fabricated query parameter rejection (400 Bad Request / 403 Forbidden).
  - Verified test pass rate: 100% (81 unit/integration tests and benchmarks passing across workspace).
- [x] Architect & Scaffold Multi-Functional LAN Platform (Phases 7–11):
  - Appended Sections 6–10 to [`ARCHITECTURE.md`](file:///f/lanflare/ARCHITECTURE.md) specifying Multi-Hop Overlay Mesh Networking, Path-Vector Gossip, E2EE Relay Envelopes, Two-Tier Virtual Network Simulator, Delay-Tolerant P2P & Group Chat, Resumable `.lanflare-part` Streaming, 5-Layer Anti-Harassment Trust Gate with Hashcash PoW, and LAN Toolbox modules.
  - Appended Phases 7–11 (Sub-phases 7.1 through 11.4) to [`ROADMAP.md`](file:///f/lanflare/ROADMAP.md) with atomic session boundaries, acceptance criteria, required tests, and verification commands.
  - Recorded ADRs (`ADR-022` through `ADR-027`) in [`PROGRESS.md`](file:///f/lanflare/PROGRESS.md).
  - Created placeholder module skeletons across `lanflare-proto` (`mesh`, `chat`, `toolbox`) and `lanflare-core` (`crypto::e2ee`, `storage::resume`, `mesh::*`, `chat::*`, `toolbox::*`) passing `cargo check --workspace`.
- [x] Implement Mesh Protocol DTOs, Signed Node Descriptors & E2EE Envelopes (Sub-phase 7.1):
  - Implemented mesh wire DTOs in `lanflare-proto::mesh`: `MeshNodeAnnouncement`, `MeshLinkMetric`, `MeshRouteAdvertisement`, `MeshGossipPacket`, `E2eeEnvelope`, `RoutedEnvelope`, and `MeshStreamHeader` with canonical constants (`MAX_MESH_TTL = 5`, `DEFAULT_HOP_PENALTY_MS = 15`, `DEFAULT_STREAM_CHUNK_SIZE = 128 * 1024`).
  - Implemented cryptographic signing and verification in `lanflare-core::crypto::e2ee` (`sign_node_announcement`, `verify_node_announcement`, `sign_with_identity`, `verify_signature_with_cert`) supporting both RSA-2048 and Ed25519 PKCS#8 identities, bound to DER certificate SHA-256 fingerprints.
  - Implemented End-to-End Encryption (E2EE) envelope framing in `lanflare-core::crypto::e2ee` (`seal_envelope`, `open_envelope`):
    - RSA-OAEP-SHA256 key encapsulation of 32-byte symmetric AEAD keys.
    - ChaCha20-Poly1305 AEAD payload encryption with randomized 12-byte nonces and AAD binding sender and recipient fingerprints.
    - Digital signatures over canonical envelope signing bytes.
    - Strict recipient fingerprint verification and rogue-recipient rejection.
  - Validated 100% test pass rate across workspace via `cargo test -p lanflare-proto --lib mesh && cargo test -p lanflare-core --lib crypto::e2ee` and `cargo test --all`.
- [x] Implement Link-State / Path-Vector Topology Graph & Route Resolver (Sub-phase 7.2):
  - Implemented `MeshNode`, `MeshLink`, `ResolvedRoute`, and `MeshTopologyGraph` in [`crates/lanflare-core/src/mesh/graph.rs`](file:///f/lanflare/crates/lanflare-core/src/mesh/graph.rs).
  - Implemented BGP-style path-vector loop prevention (`route_path` inspection), sequence number freshness checks, and TTL hop bounds (`MAX_MESH_TTL = 5`), discarding cyclic advertisements with zero graph mutation.
  - Implemented Dijkstra shortest-path route calculation over composite link cost ($\sum (\text{DEFAULT\_HOP\_PENALTY\_MS} + \text{rtt\_ms})$) with hop-constrained shortest path bounds, intermediate `relay_enabled` policy enforcement, and deterministic tie-breaking returning `(next_hop_fp, full_route_path, total_cost_ms)`.
  - Implemented stale link eviction (`evict_stale_links`, `tick`) with automatic failover rerouting upon primary relay link failure.
  - Comprehensive unit test suite (6 tests in [`mesh::graph::tests`](file:///f/lanflare/crates/lanflare-core/src/mesh/graph.rs)) testing path-vector loop rejection, zero graph mutation, Dijkstra lowest-cost selection, automatic failover rerouting upon link expiration, relay opt-out node bypass, TTL hop limits, and sequence number freshness.
  - Verified 100% test pass rate across the workspace via `cargo test -p lanflare-core --lib mesh::graph` and `cargo test --all`.
- [x] Implement Multi-Hop Gossip, Packet Forwarding & Cut-Through Stream Relay in `lanflare-core::mesh::router` (Sub-phase 7.3):
  - Implemented `MeshRouter`, `PacketDedupBuffer`, `MeshRouterConfig`, `MeshTopologyView`, `ForwardResponse`, `StreamDeliveryStats`, and `RouterError` in [`crates/lanflare-core/src/mesh/router.rs`](file:///f/lanflare/crates/lanflare-core/src/mesh/router.rs).
  - Mounted Hyper server mesh endpoints:
    - `POST /api/lanflare/v1/mesh/gossip`: bidirectional gossip batch exchange ingesting node announcements, link metrics, and route advertisements, returning local gossip response.
    - `GET  /api/lanflare/v1/mesh/topology`: exports active topology graph (`MeshTopologyView`).
    - `POST /api/lanflare/v1/mesh/forward`: verifies packet deduplication, decrements TTL, checks relay opt-out policy, resolves next-hop route via `MeshTopologyGraph`, appends local node to `route_path`, and forwards `RoutedEnvelope` to next hop or delivers locally via `PacketSink`.
    - `POST /api/lanflare/v1/mesh/stream`: performs cut-through chunked file stream relaying piping 128 KB chunks from inbound HTTP body directly to next-hop outbound HTTP stream with strictly bounded memory (< 2 MB via 16-chunk bounded Tokio mpsc channel) and backpressure, or delivers locally to `StreamChunkSink` computing uppercase 64-char hex SHA-256 integrity hash.
  - Integrated `MeshRouter` into `ServerState`, `route_request` in [`crates/lanflare-core/src/server/router.rs`](file:///f/lanflare/crates/lanflare-core/src/server/router.rs), and `start_server_full` in [`crates/lanflare-core/src/server/mod.rs`](file:///f/lanflare/crates/lanflare-core/src/server/mod.rs).
  - Comprehensive unit test suite (5 tests in [`mesh::router::tests`](file:///f/lanflare/crates/lanflare-core/src/mesh/router.rs)):
    - `test_packet_dedup_and_ttl_exhaustion`: verifies duplicate packet rejection (HTTP 422), TTL=0 rejection (HTTP 422), TTL=1 decrement exhaustion before forwarding, and bounded ring buffer capacity eviction.
    - `test_relay_policy_opt_out_rejection`: verifies non-local envelope and stream rejection (HTTP 403 Forbidden) when `relay_enabled == false`, while permitting local destination delivery, and verifies dynamic relay toggle.
    - `test_cut_through_stream_forwarding`: verifies 2 MB multi-chunk cut-through streaming across intermediate relay node with bounded memory and bit-for-bit SHA-256 match.
    - `test_gossip_exchange_and_topology_export`: verifies two-way gossip ingestion and `/topology` export.
    - `test_multi_hop_envelope_forwarding`: verifies 3-node chain (`A -> B -> C`) packet forwarding and delivery.
  - Verified 100% test pass rate across workspace via `cargo test -p lanflare-core --lib mesh::router`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all` (134 tests passing).
- [x] Implement In-Process Virtual Network Simulator & Multi-Subnet E2E Mesh Verification (`lanflare-core::mesh::sim`) (Sub-phase 7.4):
  - Implemented `VirtualNetworkMatrix`, `SimulatedLinkConfig`, `SimulatedNode`, and `SimulatedMeshCluster` in [`crates/lanflare-core/src/mesh/sim.rs`](file:///f/lanflare/crates/lanflare-core/src/mesh/sim.rs) capable of spawning $N$ isolated LanFlare nodes on localhost bound to ephemeral ports.
  - Implemented virtual subnet reachability matrix enforcing isolated Layer 2/3 domains (`Subnet A`, `Subnet B`, `Subnet C`), link latency/jitter injection, packet drop probability, and dynamic bidirectional link severing/restoration.
  - Integrated `VirtualNetworkMatrix` as a `MeshNetworkInterceptor` into `MeshRouter`, intercepting and enforcing reachability policies on outbound routed envelopes, cut-through streaming chunks, and gossip batches.
  - Integrated `PeerRegistry` with `MeshTopologyGraph` (`handle_mesh_node`, `sync_from_mesh_graph`) with `DiscoverySource::Mesh`, updating multi-hop peers with resolved `hop_count`, `route_path`, and `next_hop` metadata.
  - Implemented automated integration test suite in [`crates/lanflare-core/tests/mesh_sim_e2e.rs`](file:///f/lanflare/crates/lanflare-core/tests/mesh_sim_e2e.rs):
    - `test_two_hop_cross_subnet_discovery_and_transfer`: verified cross-subnet 2-hop discovery (`A -> B -> C`), `PeerRegistry` hop count and route tracking, and E2EE envelope and cut-through file streaming through intermediate relay.
    - `test_three_hop_chain_e2ee_file_transfer`: verified 4-node chain (`A -> B -> C -> D`) 3-hop route resolution and cut-through file transfer (512 KB) with bit-for-bit SHA-256 integrity match.
    - `test_dual_relay_failover_under_simulated_link_cut`: verified dual-relay diamond topology preferring low-latency Relay B1, dynamic link severing of A-B1 link, automatic Dijkstra failover to backup Relay B2, and successful envelope and streaming completion.
  - Verified 100% test pass rate across workspace via `cargo test -p lanflare-core --test mesh_sim_e2e`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all` (137 tests passing).
- [x] Implement Chat, Receipt & Trust Protocol DTOs + Hashcash Proof-of-Work Engine (Sub-phase 8.1):
  - Defined protocol DTOs in `lanflare-proto::chat`: `ChatMessageDto`, `ChatContentDto` (`Text`, `Image`, `File`, `Voice`, `Video`), `DeliveryReceiptDto`, `ReadReceiptDto`, `TrustRequestDto`, `HashcashStamp`, and `ResumeTransferDto`, with canonical signing bytes and validation helpers.
  - Implemented 5-layer anti-harassment trust gate in `lanflare-core::chat::trust` (`TrustGate`, `ContactTrustState`, `TrustError`, `TrustGateConfig`, `TokenBucket`):
    - 3-tier contact trust state machine (`Stranger`, `PendingApproval`, `TrustedContact`, `Blocked`).
    - Anti-Sybil Hashcash Proof-of-Work generator (`generate_hashcash_pow`) and $O(1)$ single-SHA256 verifier (`verify_hashcash_pow`) enforcing leading zero bits, target fingerprint matching, and time bucket freshness.
    - Stranger message gate enforcing max 280 plain-text characters, immediate rejection of rich media attachments, and single-request lock (`PendingApprovalLocked`) until user approval.
    - Token-bucket rate limiter per IP address and per fingerprint defending against Sybil identity rotation (`regenerate_identity`) from the same IP/subnet.
    - Global "Contacts Only" mode rejecting all stranger handshake requests, and explicit IP/fingerprint blocklists dropped in $O(1)$ time with zero disk I/O.
  - Comprehensive unit test suite covering DTO serde round-tripping, Hashcash PoW generation and verification, stranger gate & media block, Sybil rotation IP rate limiting, and contacts-only mode.
  - Verified 100% test pass rate across workspace via `cargo test -p lanflare-proto --lib chat && cargo test -p lanflare-core --lib chat::trust`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all` (141 tests passing).

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

- **ADR-014: CLI Dual-Engine Discovery Orchestration & Terminal Presentation**
  - *Context*: CLI users require rapid zero-configuration scanning of local networks to identify peers before transferring files, with clear visibility into operating system, IP address, encryption mode, and port.
  - *Decision*: Standardize CLI commands on `status`, `scan`, `send`, `receive`.
  - *Invariants*:
    - `lanflare scan` starts both `start_multicast_engine` and `start_mdns_engine` in parallel, triggers an announcement burst, feeds inbound events into `PeerRegistry`, and terminates engines cleanly within the specified timeout.
    - Formatting heuristics in `tui` detect device type and OS model keywords to output platform glyphs (Linux, Windows, macOS, Android, iOS).
    - `lanflare-cli` exports both binary `lanflare-cli` and library `lanflare_cli` targets to support integration testing of CLI arg parsing and handler execution without shell spawning.

- **ADR-015: CLI Multi-Bar Progress & Interactive Transfer Negotiation**
  - *Context*: CLI file transfers involve multiple files with varying sizes streaming in parallel. Terminal users require real-time visibility into overall progress, per-file throughput, and ETA without terminal corruption, while retaining interactive peer selection and approval prompts.
  - *Decision*: Implement `TransferProgressTracker` via `indicatif::MultiProgress` and interactive handlers in `commands.rs`.
  - *Invariants*:
    - `match_target_peer` queries target peers in priority order: canonical SHA-256 fingerprint prefix, exact parsed IP address matching an endpoint, exact case-insensitive alias match, and substring alias match. Direct IP inputs create ad-hoc peers.
    - `TransferProgressTracker` manages one overall progress bar and dynamically attaches per-file progress bars upon receiving `TransferProgress::FileProgress`, concluding with completion or cancellation statuses without leaking memory. Supports a non-interactive mode for headless or unit test execution.
    - `lanflare receive` acts as an event pump listening for `ServerTransferEvent::PrepareUpload`, rendering sender and file metadata, prompting the user (`[Y/n]`) when `auto_accept` is false, and sending `PrepareUploadDecision` via Tokio oneshot channels.

- **ADR-016: Tauri v2 Desktop IPC Bridge & Reactive State Architecture**
  - *Context*: The desktop GUI requires bi-directional communication with `lanflare-core`: invoking actions (start discovery, send transfer, update config, respond to incoming transfer requests) and reacting to async events (peers discovered, incoming transfer approval prompt, real-time file upload speed & byte progress).
  - *Decision*: Implement a dedicated `lanflare-desktop` crate using Tauri v2 (`tauri` 2.12.3) with standard plugins (`notification`, `dialog`, `fs`, `opener`, `shell`).
  - *Invariants*:
    - App state `DesktopState` is wrapped in `std::sync::Arc` and managed by Tauri state injection (`State<'_, Arc<DesktopState>>`), with fine-grained asynchronous locks (`RwLock` for configuration and server handle, `Mutex` for active sessions and discovery handles).
    - Peer discovery feeds incoming packets (`MulticastDiscoveryEvent`, `MdnsDiscoveryEvent`) into `PeerRegistry` and emits reactive Tauri events (`lanflare://peers-updated`) to the webview.
    - Incoming transfers (`ServerTransferEvent::PrepareUpload`) either auto-accept when configured or park a Tokio `oneshot::Sender<PrepareUploadDecision>` in `DesktopState::pending_incoming`, emitting `lanflare://incoming-transfer-request` to prompt the GUI user. The user's response invokes `respond_incoming_transfer` to unblock the server.
    - Outgoing file uploads spawn an async background task streaming `TransferProgress` updates directly to the webview via `lanflare://transfer-progress` events without blocking UI IPC threads.
    - Window close or hide actions minimize cleanly to the system tray (`TrayIconBuilder`), keeping background transfer and discovery listeners active unless explicitly quit.
- **ADR-017: SvelteKit 2 + Svelte 5 Runes Reactive Architecture, Light/Dark Theming & Offline Static Prerendering**
  - *Context*: The desktop GUI requires instant local performance, zero SSR server dependencies, real-time streaming reactivity without old Svelte 4 store ceremony, graceful fallbacks for browser-only development, and adaptive theme styling matching system dark preferences while defaulting to light theme.
  - *Decision*: Implement the web frontend in `web/` using SvelteKit 2, Svelte 5 runes (`$state`, `$derived.by`, `$effect`), TailwindCSS, and `@sveltejs/adapter-static` writing to `web/build`. Add a reactive `ThemeManager` ([`web/src/lib/theme.svelte.ts`](file:///f/lanflare/web/src/lib/theme.svelte.ts)) supporting `system`, `light`, and `dark` modes with inline anti-flash script in `app.html`.
  - *Invariants*:
    - Static single-page application is configured with `ssr = false`, `prerender = true`, and fallback `index.html`, matching `tauri.conf.json` `frontendDist: "../../web/build"`.
    - Central reactivity resides in `AppState` ([`web/src/lib/state.svelte.ts`](file:///f/lanflare/web/src/lib/state.svelte.ts)) using class properties decorated with `$state` and `$derived.by`.
    - Theme appearance auto-detects system `(prefers-color-scheme: dark)` setting dynamically. If detection fails or media queries are unavailable, it defaults strictly to light theme. Users can also explicitly override the theme to light or dark in preferences or via the sidebar quick toggle.
    - IPC layer in [`web/src/lib/api.ts`](file:///f/lanflare/web/src/lib/api.ts) detects `window.__TAURI_INTERNALS__`. When invoked outside Tauri (e.g. standalone Vite dev server), it returns mock models with zero console errors.
    - Responsive mobile UI and desktop window resizing: Desktop window constraints in `tauri.conf.json` are set to `minWidth: 360`, `minHeight: 500`. On viewports narrower than `768px` (`md`), the desktop sidebar converts into a mobile top navigation bar (branding, scan, theme toggle, and settings) and bottom tab bar (`Devices` and `Transfers` tabs). Device cards and transfer queue items dynamically reflow into single-column layouts with truncated badges and adaptive controls.
    - Form accessibility guidelines (e.g., associating form labels to input controls) are strictly enforced to guarantee zero Svelte compiler warnings.

- **ADR-018: Reverse File Transfer (Browser Download API) Architecture & Session Re-entrancy**
  - *Context*: In reverse file transfer mode, a sender offers files on local endpoints for web browsers or receiver clients (`POST /api/localsend/v2/prepare-download` and `GET /api/localsend/v2/download`). Browsers often refresh or request multiple assets concurrently, and optional PIN security must protect file metadata without re-prompting users on page reload.
  - *Decision*: Implement `DownloadManager` in `lanflare-core::server::session` maintaining registered offered files and active client sessions.
  - *Invariants*:
    - `POST /api/localsend/v2/prepare-download` validates optional PIN. If query includes an existing `sessionId` that has already been accepted for the client's IP address, the request is accepted immediately without re-validating the PIN.
    - `GET /api/localsend/v2/download` validates both `sessionId` and `fileId`, ensuring the client IP matches the session's recorded IP. Files stream as raw binary with `application/octet-stream` and sanitized `Content-Disposition`.
    - Both `GET /api/localsend/v2/info` and `POST /api/localsend/v2/register` dynamically advertise `download: true` if and only if offered download files are actively registered in the `DownloadManager`.

- **ADR-019: High-Throughput Streaming Buffer Tuning & Memory Exhaustion Boundary Defense**
  - *Context*: High-throughput streaming transfers across local loopback and 10Gbps LAN adapters must minimize syscall overhead and context switching without permitting memory exhaustion attacks (such as unbounded JSON control payloads or disk bombing).
  - *Decision*: Increase `DEFAULT_BUFFER_CAPACITY` in `StreamingFileWriter` to 128 KB, expose `create_with_capacity`, replace 16 KB stack buffer in `write_all_from_reader` with 128 KB heap buffer, add configurable `chunk_size` (default: 128 KB) to `TransferOptions`, configure Hyper HTTP/1.1 connection builder with `max_buf_size(128 * 1024)`, and enforce `MAX_JSON_BODY_SIZE` (10 MB) via `http_body_util::Limited` on all incoming control JSON requests.
  - *Invariants*:
    - Incoming JSON request bodies on `/register` and `/prepare-upload` are bounded to `MAX_JSON_BODY_SIZE` (10 MB). Exceeding requests are rejected immediately with HTTP 413 Payload Too Large or HTTP 400 Bad Request, preventing arbitrary RAM allocation attacks.
    - Path traversal attacks in advertised filenames (`PrepareUploadRequestDtoV2`) are strictly sanitized to basenames inside `download_dir` prior to temporary file allocation, neutralizing directory breakout attempts.
    - Disk bombing (uploading more bytes than promised in `prepare-upload`) triggers early stream abort and immediate temporary `.lanflare-part` file deletion.
- **ADR-020: Active Subnet Sweep Fallback, Parallel Multi-Interface Probing & Mobile Discovery**
  - *Context*: While LanFlare's receiver and server reliably receive files from LocalSend Android clients, outbound discovery (`lanflare scan` and `lanflare send`) failed to discover the mobile peer when relying solely on UDP multicast (`224.0.0.167:53317`) and mDNS. This occurs because Android OS power-saving and Wi-Fi Access Point (AP) isolation frequently drop wireless multicast packets or block incoming UDP multicast datagrams. Furthermore, the LocalSend specification dictates that peers answering multicast announcements do so via HTTP unicast `POST /api/localsend/v2/register` rather than broadcasting a UDP multicast reply.
  - *Decision*:
    1. Implement concurrent `/24` subnet scanning in `lanflare-core::discovery::scanner::scan_subnet` with doubled concurrency (`SCAN_CONCURRENCY = 128`), fast connect timeout (`300ms`), and early abort skipping plain HTTP retries when the initial TCP port connect fails or times out.
    2. Parallelize multi-interface sweeps across all active physical/virtual LAN subnets (`futures_util::future::join_all`), filtering out point-to-point / CGNAT VPN meshes (`tailscale*`, `100.64.0.0/10`, `docker*`, `vboxnet*`).
    3. Implement 500ms grace escalation matching official LocalSend: multicast and mDNS start first; if no peers respond within 500ms (or if `--subnet` is explicitly set), the parallel subnet scanner launches concurrently across all LAN subnets. As soon as a peer is discovered or the sweep completes, the scanner finishes with early exit without waiting for blind timeouts.
    4. Enhance direct IP targeting (`lanflare send --to <IP>`): actively probe the destination IP over HTTPS and HTTP to resolve its TLS fingerprint, alias, and port before transfer orchestration, eliminating TOFU dummy peers and connection failures.
  - *Invariants*:
    - Loopback and non-LAN VPN/container adapters are excluded from subnet sweeps.
    - Self-discovered fingerprints matching local TLS identity are suppressed from registry ingestion.
    - Verified live end-to-end against physical Android LocalSend device (`Sec` at `10.40.1.121:53317`) with 100% discovery rate and sub-2.3s total sweep time across multi-interface networks.

- **ADR-021: Mobile Edge-to-Edge Safe Area Inset Bridge & Responsive Layout Adaptation**
  - *Context*: On mobile Android and iOS devices, modern operating systems run webviews in edge-to-edge display mode where the webview canvas spans underneath translucent system bars (top status/notification bar with clock, battery, camera cutout; bottom navigation bar with 3-button or gesture pill). Android WebViews running in edge-to-edge mode return `0px` for standard CSS `env(safe-area-inset-*)`, causing the top navigation bar and bottom navigation tabs to be occluded by system controls.
  - *Decision*:
    1. Implement native Android Javascript Bridge (`AndroidSafeInsets`) in `MainActivity.kt` using `ViewCompat.setOnApplyWindowInsetsListener(webView)` to calculate pixel-to-dp insets from `WindowInsetsCompat.Type.systemBars()` and `displayCutout()`, dynamically evaluating JavaScript to update CSS root variables `--safe-area-inset-top`, `--safe-area-inset-bottom`, `--safe-area-inset-left`, and `--safe-area-inset-right`.
    2. Add `viewport-fit=cover` to `<meta name="viewport">` in `app.html` with a synchronous inline script reading `window.AndroidSafeInsets` before DOM hydration to avoid layout flicker.
    3. Define unified CSS fallback variables in `app.css`:
       `--safe-top: max(env(safe-area-inset-top, 0px), var(--safe-area-inset-top, 0px))` (and bottom/left/right equivalents), working identically across Android WebView, iOS Safari/WebKit, and desktop browsers.
    4. Apply dynamic safe-area insets to mobile top header (`padding-top: calc(0.625rem + var(--safe-top))`), mobile bottom nav (`padding-bottom: calc(0.5rem + var(--safe-bottom))`), aside/main containers, and all modal backdrops/dialogs (`IncomingTransferModal`, `SettingsModal`, `SendFileDialog`, `TextMessageModal`, `IncomingBrowserDownloadModal`).
    5. Set root layout viewport to `h-screen h-[100dvh]` to account for dynamic browser bar resizing.
  - *Invariants*:
    - UI elements never clip or overlap behind native system bars or notches on any platform.
    - Zero regression on desktop when resizing windows down to mobile widths (safe insets evaluate to `0px`).
    - Zero compiler or clippy warnings across Rust, Kotlin, and TypeScript.

- **ADR-022: Path-Vector Mesh Gossip, Composite Cost Routing & Cut-Through Stream Relay**
  - *Context*: Multi-homed devices bridging separate subnets (e.g., Wi-Fi + Ethernet, corporate VLANs, VPNs) can connect peers that cannot see each other via L2 multicast or `/24` subnet scans.
  - *Decision*: Implement `/api/lanflare/v1/mesh/*` using cryptographically signed `MeshNodeAnnouncement` descriptors, BGP-style path-vector loop avoidance (`route_path: Vec<Fingerprint>`), TTL bounds (`MAX_MESH_TTL = 5`), and Dijkstra route selection minimizing $\sum (\text{HOP\_PENALTY\_MS} + \text{rtt\_ms})$.
  - *Invariants*:
    - Route advertisements containing the local node's own fingerprint in `route_path` or carrying stale `seq_no` are discarded immediately.
    - Multi-hop file transfers (`/api/lanflare/v1/mesh/stream`) use cut-through chunked streaming with Tokio backpressure, never buffering full files in relay RAM or disk.
    - Nodes with `relay_enabled == false` reject non-local forwarding requests with HTTP 403 Forbidden.

- **ADR-023: End-to-End Encryption (E2EE) Envelope Over Untrusted Mesh Relays**
  - *Context*: Intermediate mesh relays forward control messages, chat messages, and file streams across subnets and must not be able to read or tamper with payloads.
  - *Decision*: Wrap all mesh-routed payloads in an `E2eeEnvelope` (`lanflare-core::crypto::e2ee`) using public-key authenticated key derivation bound to source and destination X.509 certificates (`TlsIdentity`) and `CHACHA20_POLY1305` AEAD encryption.
  - *Invariants*:
    - Relays only inspect outer routing metadata (`packet_id`, `src_fp`, `dst_fp`, `ttl`, `route_path`).
    - Payloads failing AEAD authentication or signed by a key not matching `src_fp` are rejected before storage or UI dispatch.

- **ADR-024: Two-Tier Virtual Network Simulator (`VirtualNetworkMatrix` & Interactive Sandbox)**
  - *Context*: Developers and users rarely have multi-router physical network testbeds to verify multi-hop mesh discovery, relay forwarding, and link failover.
  - *Decision*: Build an in-process `VirtualNetworkMatrix` (`lanflare-core::mesh::sim`) that runs $N$ isolated LanFlare nodes on localhost with programmable virtual subnet partitions, latency/drop injection, and link severing, exposed both to `cargo test` integration suites and to an interactive "Virtual Mesh Sandbox" in the GUI/CLI.

- **ADR-025: Identity-First Delay-Tolerant Outbox & Resumable `.lanflare-part` Transfers**
  - *Context*: LAN peers frequently go offline or change IP addresses mid-conversation or mid-transfer.
  - *Decision*: Persist contacts, conversations, and an `OutboxDispatcher` queue keyed strictly by canonical 64-char SHA-256 `fingerprint`. Extend `StreamingFileWriter` with `.lanflare-resume.json` byte-offset checkpoints (`lanflare-core::storage::resume`) so interrupted transfers from trusted peers resume from `received_bytes` upon reconnection.
  - *Invariants*:
    - Outbox items dispatch automatically in causal FIFO order when the target `fingerprint` appears via 1-hop `PeerRegistry` or multi-hop `MeshTopologyGraph`.
    - Optional custodial `RelayMailbox` on mutual relays stores bounded E2EE envelopes when sender and recipient are never online simultaneously.

- **ADR-026: Defense-in-Depth Anti-Harassment Gate, Hashcash PoW & Rate Limiting**
  - *Context*: Open LANs contain untrusted strangers who might attempt unsolicited file bombing, message flooding, or Sybil identity rotation (`regenerate_identity`).
  - *Decision*: Enforce a 3-tier contact trust state machine (`Stranger` -> `PendingApproval` -> `TrustedContact` / `Blocked`) guarded by Hashcash Proof-of-Work and per-IP/per-fingerprint token-bucket rate limiting.
  - *Invariants*:
    - `Stranger`s can send at most one $\le 280$-character plain-text introduction request accompanied by a valid Hashcash PoW stamp; all media/file uploads and subsequent messages from unapproved strangers are rejected with HTTP 403 Locked.
    - Zero disk bytes (`.lanflare-part`) are allocated for `Stranger` or `Blocked` peers.

- **ADR-027: Causal Merkle-DAG + Hybrid Logical Clock (HLC) Serverless Group Sync**
  - *Context*: Local group chats have no central server and members join/leave or go offline independently.
  - *Decision*: Model group chat history as a content-addressed Causal Merkle-DAG ordered by Hybrid Logical Clocks (`HybridLogicalClock`) and synchronized via anti-entropy frontier exchange (`/api/lanflare/v1/chat/group-sync`).

- **ADR-028: Signed Mesh Node Descriptors & Hybrid RSA-OAEP + ChaCha20-Poly1305 E2EE Envelope Framing**
  - *Context*: Intermediate multi-hop relays forward packets, chat messages, and file streams across subnets and must not be able to forge node announcements, read sensitive payload content, or tamper with message bytes.
  - *Decision*: Combine cryptographic digital signatures over canonical node announcements with hybrid RSA-OAEP key encapsulation and `CHACHA20_POLY1305` AEAD encryption.
  - *Invariants*:
    - `MeshNodeAnnouncement::canonical_signing_bytes` strictly binds `fingerprint`, `alias`, `cert_pem`, `capabilities`, `relay_enabled`, `seq_no`, and `timestamp_ms`. Modifying any field invalidates digital signature verification.
    - Node descriptor fingerprints must match `SHA256(DER(cert_pem))` case-insensitively; any spoofing or mismatch rejects the announcement in $O(1)$ time.
    - `E2eeEnvelope` generates a random 32-byte symmetric key encapsulated with recipient's RSA public key using RSA-OAEP-SHA256.
    - Payloads are encrypted using `ring::aead::CHACHA20_POLY1305` with randomized 12-byte nonces and AAD binding `sender_fp` and `recipient_fp`. Bit flips in ciphertext or nonce tampering fail AEAD authentication immediately with `E2eeError::AeadAuthenticationFailed`.
    - Envelopes carry a digital signature generated by the sender's private key (`key_pem`, supporting RSA or Ed25519) over envelope metadata & ciphertext.
    - Opening an envelope with a non-matching recipient identity fails with `E2eeError::RecipientMismatch` before RSA decryption is attempted.

- **ADR-029: Link-State & Path-Vector Hybrid Graph Representation & Hop-Constrained Shortest Path**
  - *Context*: Decentralized LAN overlay mesh nodes must maintain dynamic connectivity maps across multi-homed devices without central routing controllers, avoid routing loops, reject stale topology updates, enforce intermediate relay opt-out policies, and automatically fail over when relay links expire.
  - *Decision*: Implement `MeshTopologyGraph` combining link-state adjacency indexing (`adj: HashMap<String, HashSet<String>>`) with BGP-style path-vector route advertisement validation and Dijkstra shortest-path route resolution.
  - *Invariants*:
    - **Loop Prevention & Zero Mutation**: Any incoming route advertisement containing the local node's fingerprint in `route_path` (case-insensitively), having `origin_fp == local_fp`, containing internal cycles, having `ttl == 0`, or having `route_path.len() > max_ttl` is rejected with zero graph mutation.
    - **Sequence Number Freshness**: Any announcement or route advertisement carrying a sequence number less than the highest known sequence number for that origin is rejected with `MeshGraphError::StaleSequenceNumber`. Equal sequence numbers are ignored as idempotent duplicates.
    - **Composite Cost Function**: Link weight is calculated as $\text{cost}(e) = \text{DEFAULT\_HOP\_PENALTY\_MS} + e.\text{rtt\_ms}$. Dijkstra tracks optimal cost by `(fingerprint, hop_count)` to guarantee hop-constrained shortest path optimality within `MAX_MESH_TTL = 5`.
    - **Relay Policy Enforcement**: Any intermediate node with `relay_enabled == false` is skipped during Dijkstra expansion and cannot serve as a multi-hop transit relay. Destinations are exempt from the relay requirement.
    - **Link Expiration & Automatic Reroute**: Inactive links exceeding `link_expiry_ms` are pruned via `evict_stale_links` / `tick`, triggering automatic failover to alternative routes within 1 tick.

- **ADR-030: Multi-Hop Gossip, Packet Forwarding & Cut-Through Stream Relay Architecture**
  - *Context*: Multi-hop overlay mesh networks require robust packet and stream forwarding over intermediate relay nodes without central coordinators, while preventing routing loops, packet duplication storms, memory exhaustion, and transit abuse on battery/metered nodes.
  - *Decision*: Implement `MeshRouter` mounting Hyper endpoints (`/api/lanflare/v1/mesh/*`) backed by `PacketDedupBuffer`, `MeshTopologyGraph`, and bounded cut-through streaming channels.
  - *Invariants*:
    - **Packet Deduplication Ring Buffer**: Track up to `dedup_capacity` (default: 10,000) packet IDs via a combined FIFO `VecDeque` and `HashSet`. Duplicate `packet_id`s are rejected immediately with HTTP 422 Unprocessable Entity with zero downstream network traffic.
    - **TTL Decrementing & Exhaustion**: Incoming envelopes with `ttl == 0` are rejected immediately. For non-local forwarding, TTL is decremented by 1 (`ttl.saturating_sub(1)`). If the decremented TTL reaches 0, forwarding aborts with HTTP 422 Unprocessable Entity. Traversed nodes are appended to `route_path`.
    - **Relay Policy Enforcement**: Nodes configured with `relay_enabled == false` reject all non-local forwarding requests (`forward` and `stream`) with HTTP 403 Forbidden. Local destination delivery is always permitted regardless of relay setting.
    - **Bounded Memory Cut-Through Stream Relay**: Streaming transfers (`POST /api/lanflare/v1/mesh/stream`) do not buffer complete files in RAM or disk. Inbound 128 KB chunks are piped directly to next-hop outbound HTTP stream via a bounded Tokio `mpsc` channel (default: 16 chunks = 2 MB max in-flight memory). Channel backpressure naturally throttles upstream TCP connections if downstream hops slow down.
    - **Local Delivery & Checksum Verification**: Streams reaching their destination compute streaming uppercase 64-char hex SHA-256 digests and dispatch chunks to registered `StreamChunkSink` callbacks, returning `StreamDeliveryStats` with 100% bit-for-bit integrity.

- **ADR-031: Virtual Network Matrix Interception & Multi-Hop Registry Synchronization**
  - *Context*: Multi-hop mesh discovery, Dijkstra route resolution, and cut-through streaming require automated integration verification on developer machines without physical multi-router testbeds. Furthermore, peers discovered via multi-hop mesh relays must seamlessly surface in the unified `PeerRegistry` alongside 1-hop Multicast and mDNS peers.
  - *Decision*: Implement `VirtualNetworkMatrix` implementing `MeshNetworkInterceptor` and attach it to `MeshRouter`. Extend `PeerRegistry` with `DiscoverySource::Mesh` and `sync_from_mesh_graph`.
  - *Invariants*:
    - **Subnet Partition & Reachability Invariant**: Direct communication between simulated nodes requires that neither node is offline, the directed link is not severed, and both nodes share at least one virtual subnet.
    - **Source Origin Exemption**: The relay policy check (`relay_enabled == false`) applies strictly to non-local forwarded envelopes where `src_fp != local_fp`. A node can always originate its own outgoing packets regardless of relay configuration.
    - **Mesh Peer Registry Representation**: Multi-hop mesh peers are ingested into `PeerRegistry` with `DiscoverySource::Mesh`, populating `hop_count`, `route_path`, and `next_hop` metadata without overwriting direct IP endpoints if discovered on 1-hop channels.
    - **Full Gossip Propagation**: Nodes gossiping via `MeshRouter` exchange all known node announcements and generated path-vector route advertisements, enabling multi-hop nodes $N$ hops away to discover remote aliases and X.509 certificates.

- **ADR-032: 5-Layer Anti-Harassment Trust Gate, Hashcash Proof-of-Work & Token-Bucket Sybil Defense**
  - *Context*: Open local area networks (universities, coffee shops, hotels, offices) expose devices to unsolicited message flooding, malicious media bombing, and Sybil identity rotation (`regenerate_identity`) where an attacker cycles certificates to bypass blocklists.
  - *Decision*: Enforce a strict 5-layer trust gate (`TrustGate` in `lanflare-core::chat::trust`) backed by `lanflare-proto::chat` protocol DTOs.
  - *Invariants*:
    - **3-Tier Contact Trust State Machine**: Peers are classified into `Stranger`, `PendingApproval`, `TrustedContact`, or `Blocked`. Unknown fingerprints start strictly as `Stranger`.
    - **Zero Unsolicited Disk Allocation**: `Stranger` and `Blocked` peers are strictly forbidden from transmitting rich media attachments (`Image`, `File`, `Voice`, `Video`). Any attempt is rejected immediately prior to disk I/O with `TrustError::MediaNotPermittedForStranger`.
    - **Single Introduction Gate**: `Stranger` introduction messages and `TrustRequestDto`s are limited to $\le 280$ UTF-8 characters. Successfully evaluated requests transition the sender to `PendingApproval`. Any subsequent message or request prior to explicit user approval is locked and rejected with `TrustError::PendingApprovalLocked`.
    - **O(1) Anti-Sybil Hashcash PoW**: Stranger requests must carry a valid `HashcashStamp` satisfying `LeadingZeroBits(SHA256(sender_fp || target_fp || time_bucket || nonce)) >= k` (default $k=16$). Verification is $O(1)$ (1 single SHA-256 computation), while generation requires computational work, throttling mass botnet probes. Target fingerprint matching and time bucket freshness (`max_bucket_skew = 1`) prevent replay across nodes or across time.
    - **Token-Bucket Rate Limiter Defense**: Per-IP and per-fingerprint token buckets constrain burst rates (e.g. 5 requests capacity, refill 0.1/s). Even if an attacker rapidly rotates ephemeral certificate fingerprints from the same IP, the IP rate limiter rejects subsequent attempts in $O(1)$ time with `TrustError::RateLimited`.
    - **Contacts-Only Mode & O(1) Dropping**: Enabling "Contacts Only" mode immediately drops all stranger handshakes with `TrustError::ContactsOnlyMode`. Blocked fingerprints and blocked IPs are rejected at the boundary in $O(1)$ time.

---

## Known Debt & Blockers
- *None at present.*

---

## Next Session Target
- **Target**: **Sub-phase 8.2: Persistent Contact Book, Conversation Store & Delay-Tolerant Outbox (`lanflare-core::chat::store` & `lanflare-core::chat::outbox`)**
- **Action Items**:
  1. Implement persistent `ChatStore` in `lanflare-core::chat::store`:
     - Contacts table persisting `fingerprint`, `alias`, `trust_state`, `avatar_thumbnail`, and `last_seen_ms` to disk under `<data_dir>/chat/contacts.json`.
     - Conversations and message thread store persisting message histories and delivery states (`Queued`, `Sending`, `Relayed`, `Delivered`, `Read`, `Failed`) keyed by canonical `fingerprint`.
  2. Implement reactive `OutboxDispatcher` in `lanflare-core::chat::outbox`:
     - Delay-tolerant queue storing pending outgoing messages and receipts when recipients are offline.
     - Event subscriber listening to `PeerRegistry` events (`PeerAdded`, `PeerUpdated`) and `MeshTopologyGraph` route events to automatically trigger FIFO causal outbox flushes when an offline peer's fingerprint appears online (1-hop LAN or multi-hop mesh).
     - Cryptographic receipt processing transitioning message states monotonically (`Queued -> Delivered -> Read`).
  3. Implement unit and integration tests:
     - `chat::store::tests::test_persistent_store_crud_and_reload_recovery`
     - `chat::outbox::tests::test_offline_queue_auto_flush_on_peer_discovery`
     - `chat::outbox::tests::test_delivery_and_read_receipt_monotonic_transitions`
  4. Verification command:
     ```bash
     cargo test -p lanflare-core --lib chat::store && cargo test -p lanflare-core --lib chat::outbox
     ```





