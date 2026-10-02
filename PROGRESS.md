# Operational Progress & Architecture Decision Record (ADR)

## Current Phase & Milestone
- **Current Milestone**: Phase 3: Transfer Engine & HTTP Server/Client
- **Overall Status**: Phase 2.3 complete. Ready for Phase 3.1 implementation.

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

---

## Known Debt & Blockers
- *None at present.*

---

## Next Session Target
- **Target**: **Phase 3.1: Secure Storage & Streaming I/O (`lanflare-core::storage`)**
- **Action Items**:
  1. Inspect [`crates/lanflare-core/src/storage`](file:///f/lanflare/crates/lanflare-core/src/storage) modules (`path.rs`, `writer.rs`, `mod.rs`).
  2. Implement path sanitization engine in `storage::path` preventing path traversal (`../`, absolute paths, Windows alternate data streams `::`, null bytes, reserved OS device names like `CON`, `PRN`, `NUL`).
  3. Implement streaming disk writer in `storage::writer` with concurrent SHA-256 hashing writing to `.lanflare-part` temporary files.
  4. Implement atomic rename on hash match and cleanup / deletion on abort or failure.
  5. Add unit and integration tests verifying traversal attack rejections, checksum verification, and cleanup.
  6. Verify with `cargo test -p lanflare-core --lib storage` and `cargo clippy --all-targets -- -D warnings`.
