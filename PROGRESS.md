# Operational Progress & Architecture Decision Record (ADR)

## Current Phase & Milestone
- **Current Milestone**: Phase 1: Protocol Foundations & Core Cryptography
- **Overall Status**: Phase 1.1 complete. Ready for Phase 1.2 implementation.

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
  - *Decision*: Generate self-signed certificates using `rcgen`. Authentication relies on pinning and verifying the SHA-256 fingerprint of the remote certificate against the fingerprint advertised during discovery or handshake.

- **ADR-004: Direct Streaming File Transfers with Incremental SHA-256**
  - *Context*: File sharing involves arbitrary sizes (from small photos to multi-gigabyte video or backup archives).
  - *Decision*: Files must stream directly from the HTTP socket to temporary disk storage (`.lanflare-part`) via buffered async streams. Compute SHA-256 concurrently during streaming and atomically rename to destination path only upon verification match. Never buffer complete files in RAM.

- **ADR-005: Strict Path Traversal Defense**
  - *Context*: Remote peers specify `fileName` in `prepare-upload`.
  - *Decision*: Sanitize all received filenames by extracting strictly the basename, stripping illegal path characters and path separators (`/`, `\`, null bytes, control codes), and confining outputs strictly within the configured download directory.

- **ADR-006: LocalSend v2.2 Wire Schema & Unknown Enum Fallback**
  - *Context*: Heterogeneous device ecosystems running varying versions of LocalSend or third-party clients may send unknown device types or optional fields.
  - *Decision*: Implement custom deserialization for `Option<DeviceType>` in `lanflare-proto::discovery::device_type_serde` that maps unknown strings to `DeviceType::Desktop` as required by Section 7.1 of the LocalSend protocol specification. All wire fields use camelCase naming, with lowercase protocol and device types.

---

## Known Debt & Blockers
- *None at present.*

---

## Next Session Target
- **Target**: **Phase 1.2: Crypto Primitives & Ephemeral TLS Identities (`lanflare-core::crypto`)**
- **Action Items**:
  1. Implement self-signed X.509 certificate and private key generation using `rcgen` in `lanflare-core/src/crypto/cert.rs`.
  2. Implement SHA-256 certificate fingerprint computation (uppercase hex formatted) matching LocalSend conventions.
  3. Implement custom `rustls` server and client certificate verifiers (`lanflare-core/src/crypto/verifier.rs`) that authenticate peers against pinned fingerprints without CA trust roots.
  4. Write unit tests in `lanflare-core` verifying certificate generation, parsing via `x509-parser`, and fingerprint verification.
  5. Verify with `cargo test -p lanflare-core --lib crypto`.
