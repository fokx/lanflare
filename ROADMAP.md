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


---

## Phase 7: Multi-Hop Overlay Mesh Protocol & Virtual Network Simulator

### Sub-phase 7.1: Mesh Protocol DTOs, Signed Node Descriptors & E2EE Envelopes (`lanflare-proto::mesh` & `lanflare-core::crypto::e2ee`)
- **Scope**:
  - Implement mesh wire DTOs in `lanflare-proto::mesh`: `MeshNodeAnnouncement`, `MeshLinkMetric`, `MeshGossipPacket`, `MeshRouteAdvertisement`, `RoutedEnvelope`, and `MeshStreamHeader`.
  - Implement cryptographic signing and verification for `MeshNodeAnnouncement` bound to `TlsIdentity` DER certificate fingerprints so relays cannot spoof identities.
  - Implement End-to-End Encryption (E2EE) envelopes (`E2eeEnvelope`, `seal_envelope`, `open_envelope`) in `lanflare-core::crypto::e2ee` using `ring` AEAD (`CHACHA20_POLY1305`) and public-key authenticated key derivation so intermediate relays cannot inspect payloads.
- **Acceptance Criteria**:
  - `MeshNodeAnnouncement` signature verification succeeds for valid self-signed RSA-2048 and Ed25519 identities and fails immediately if `alias`, `seq_no`, or `cert_pem` is tampered with.
  - E2EE envelope encryption/decryption round-trips arbitrary payloads and rejects modified ciphertext or mismatched recipient fingerprints.
- **Required Unit/Integration Tests**:
  - `mesh::tests::test_mesh_dto_serde_roundtrip`
  - `crypto::e2ee::tests::test_signed_node_announcement_verification_and_tamper_rejection`
  - `crypto::e2ee::tests::test_e2ee_envelope_seal_open_and_wrong_recipient_rejection`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-proto --lib mesh && cargo test -p lanflare-core --lib crypto::e2ee
  ```

### Sub-phase 7.2: Link-State / Path-Vector Topology Graph & Route Resolver (`lanflare-core::mesh::graph`)
- **Scope**:
  - Implement `MeshTopologyGraph` managing known 1-hop and multi-hop `MeshNode`s and directed `MeshLink`s (`src_fp`, `dst_fp`, `rtt_ms`, `subnet_tag`, `last_seen_ms`).
  - Implement BGP-style path-vector loop detection (`route_path` inspection), TTL enforcement (`MAX_MESH_TTL = 5`), sequence number freshness checks, and stale link eviction.
  - Implement Dijkstra shortest-path route calculation minimizing composite path cost ($\sum (\text{HOP\_PENALTY\_MS} + \text{rtt\_ms})$) and returning `(next_hop_fp, full_route_path, total_cost_ms)`.
- **Acceptance Criteria**:
  - Route advertisements containing the local node's own fingerprint in `route_path` are discarded with zero graph mutation.
  - Given a diamond 4-node topology with varying link RTTs, `resolve_route` deterministically selects the lowest-cost multi-hop path and fails over within 1 tick when the primary relay link expires.
- **Required Unit/Integration Tests**:
  - `mesh::graph::tests::test_path_vector_loop_rejection`
  - `mesh::graph::tests::test_dijkstra_lowest_cost_multi_hop_selection`
  - `mesh::graph::tests::test_relay_link_failure_automatic_reroute`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib mesh::graph
  ```

### Sub-phase 7.3: Multi-Hop Gossip, Packet Forwarding & Cut-Through Stream Relay (`lanflare-core::mesh::router`)
- **Scope**:
  - Implement `MeshRouter` and mount Hyper server endpoints:
    - `POST /api/lanflare/v1/mesh/gossip`
    - `GET  /api/lanflare/v1/mesh/topology`
    - `POST /api/lanflare/v1/mesh/forward`
    - `POST /api/lanflare/v1/mesh/stream`
  - Implement packet deduplication ring buffer (`seen_packet_ids`) and TTL decrementing on forwarded envelopes.
  - Implement cut-through streaming file relay in `/api/lanflare/v1/mesh/stream` piping 128 KB chunks from the inbound HTTP body directly to the next-hop outbound HTTP stream with bounded memory (< 2 MB per stream) and configurable `relay_enabled` policy.
- **Acceptance Criteria**:
  - Duplicate `packet_id` envelopes or envelopes with `ttl == 0` are dropped immediately with HTTP 400 / 422.
  - Nodes with `relay_enabled == false` reject non-local destination forwarding with HTTP 403 Forbidden.
  - Cut-through streaming relays multi-megabyte E2EE streams across an intermediate node with bit-for-bit SHA-256 integrity.
- **Required Unit/Integration Tests**:
  - `mesh::router::tests::test_packet_dedup_and_ttl_exhaustion`
  - `mesh::router::tests::test_relay_policy_opt_out_rejection`
  - `mesh::router::tests::test_cut_through_stream_forwarding`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib mesh::router
  ```

### Sub-phase 7.4: In-Process Virtual Network Simulator & Multi-Subnet E2E Mesh Verification (`lanflare-core::mesh::sim`)
- **Scope**:
  - Implement `VirtualNetworkMatrix` and `SimulatedMeshCluster` in `lanflare-core::mesh::sim` capable of spawning $N$ isolated LanFlare nodes on localhost with programmable virtual subnet partitions (`Subnet A`, `Subnet B`, `Subnet C`), artificial link latency/jitter, packet drop injection, and dynamic link severing.
  - Integrate `PeerRegistry` with `MeshTopologyGraph` so multi-hop peers discovered via relays appear with hop count and route metadata.
- **Acceptance Criteria**:
  - In a 4-node simulated topology (`Node A [Sub1] <-> Relay B [Sub1, Sub2] <-> Relay C [Sub2, Sub3] <-> Node D [Sub3]`), Node A and Node D cannot reach each other directly, yet automatically discover each other via 3-hop gossip and complete an E2EE multi-hop file transfer.
  - Severing a primary relay in a dual-relay topology triggers automatic route reconvergence and successful transfer delivery.
- **Required Unit/Integration Tests**:
  - `test_two_hop_cross_subnet_discovery_and_transfer`
  - `test_three_hop_chain_e2ee_file_transfer`
  - `test_dual_relay_failover_under_simulated_link_cut`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --test mesh_sim_e2e
  ```

---

## Phase 8: Anti-Harassment Trust Engine, Persistent Chat Store & Resumable Transfers

### Sub-phase 8.1: Chat, Receipt & Trust Protocol DTOs + Hashcash Proof-of-Work Engine (`lanflare-proto::chat` & `lanflare-core::chat::trust`)
- **Scope**:
  - Define protocol DTOs in `lanflare-proto::chat`: `ChatMessageDto`, `ChatContentDto` (Text, Image, File, Voice, Video), `DeliveryReceiptDto`, `ReadReceiptDto`, `TrustRequestDto`, `HashcashStamp`, and `ResumeTransferDto`.
  - Implement the 5-Layer Anti-Harassment & Trust Gate in `lanflare-core::chat::trust`:
    - 3-tier contact state (`Stranger`, `PendingApproval`, `TrustedContact`, `Blocked`).
    - Hashcash Proof-of-Work generator and $O(1)$ verifier (`SHA256(sender_fp || target_fp || time_bucket || nonce)` with configurable leading zero bits).
    - Single 280-char plain-text handshake request limit for `Stranger`s, per-IP/per-fingerprint token-bucket rate limiter, and "Contacts Only" mode.
- **Acceptance Criteria**:
  - `Stranger` requests without a valid Hashcash PoW stamp or exceeding 280 chars or containing media attachments are rejected immediately before any disk I/O.
  - A second message from an unapproved `Stranger` is rejected with `TrustError::PendingApprovalLocked`.
  - `Blocked` fingerprints and rate-limited IPs are dropped silently in $O(1)$ time.
- **Required Unit/Integration Tests**:
  - `chat::tests::test_chat_dto_serde_roundtrip`
  - `chat::trust::tests::test_hashcash_pow_generation_and_verification`
  - `chat::trust::tests::test_stranger_single_message_gate_and_media_block`
  - `chat::trust::tests::test_sybil_identity_rotation_ip_rate_limiter`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-proto --lib chat && cargo test -p lanflare-core --lib chat::trust
  ```

### Sub-phase 8.2: Persistent Contact Book, Conversation Store & Delay-Tolerant Outbox (`lanflare-core::chat::store` & `lanflare-core::chat::outbox`)
- **Scope**:
  - Implement persistent `ChatStore` in `lanflare-core::chat::store` storing contacts, conversation threads, message history, and message delivery states (`Queued`, `Sending`, `Relayed`, `Delivered`, `Read`, `Failed`) keyed by canonical `fingerprint`.
  - Implement reactive `OutboxDispatcher` in `lanflare-core::chat::outbox` subscribing to `PeerRegistry` and `MeshTopologyGraph` events to automatically flush queued messages and receipts whenever an offline target `fingerprint` comes online (1-hop or $N$-hop).
- **Acceptance Criteria**:
  - Messages queued while a peer is offline persist across store reloads and dispatch automatically in FIFO causal order within 200ms of the peer's `fingerprint` appearing in `PeerRegistry` or `MeshTopologyGraph`.
  - Cryptographic `DeliveryReceiptDto` and `ReadReceiptDto` transition message states monotonically (`Queued -> Delivered -> Read`) and are themselves queued in the DTN outbox if the sender is offline.
- **Required Unit/Integration Tests**:
  - `chat::store::tests::test_persistent_store_crud_and_reload_recovery`
  - `chat::outbox::tests::test_offline_queue_auto_flush_on_peer_discovery`
  - `chat::outbox::tests::test_delivery_and_read_receipt_monotonic_transitions`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib chat::store && cargo test -p lanflare-core --lib chat::outbox
  ```

### Sub-phase 8.3: Resumable Byte-Offset Media/File Streaming & 1:1 Chat Endpoints (`lanflare-core::chat::engine` & `lanflare-core::storage::resume`)
- **Scope**:
  - Implement resumable `.lanflare-part` checkpointing in `lanflare-core::storage::resume` (`ResumeManager`, `PartialTransferState`), allowing interrupted transfers from trusted contacts to retain verified chunks on disk and resume from `received_bytes` offset via `POST /api/lanflare/v1/transfer/resume`.
  - Mount 1:1 chat & media endpoints in Hyper router (`/api/lanflare/v1/chat/request`, `/api/lanflare/v1/chat/message`, `/api/lanflare/v1/chat/receipt`, `/api/lanflare/v1/transfer/resume`) and wire `ChatEngine` in `lanflare-core::chat::engine`.
- **Acceptance Criteria**:
  - Interrupting a 4 MB file/video transfer at 50% and reconnecting resumes streaming strictly the remaining 2 MB and verifies the full-file SHA-256 upon atomic commit.
  - End-to-end 1:1 chat exchange (trust handshake -> accept -> text, image, voice, video -> delivery receipt -> read receipt) succeeds across both direct 1-hop LAN and 2-hop simulated mesh relays.
- **Required Unit/Integration Tests**:
  - `storage::resume::tests::test_interrupted_stream_checkpoint_and_byte_offset_resume`
  - `test_dtn_offline_chat_and_resumable_media_e2e`
  - `test_multi_hop_mesh_chat_with_receipts_e2e`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --test chat_dtn_e2e
  ```

---

## Phase 9: Decentralized Mesh Group Chat & Custodial Relay Mailbox

### Sub-phase 9.1: Hybrid Logical Clock (HLC) & Causal Merkle-DAG Group State (`lanflare-core::chat::group`)
- **Scope**:
  - Implement `HybridLogicalClock` (`wall_time_ms`, `counter`, `node_fp`) and `GroupDagState` in `lanflare-core::chat::group`.
  - Implement group creation, member roster management, pairwise-encrypted `GroupEpochKey` distribution, and content-addressed group message nodes (`msg_id = SHA256(header || parent_ids || payload)`).
- **Acceptance Criteria**:
  - Concurrent messages authored offline by 3 partitioned group members merge deterministically into a conflict-free topological sort order across all nodes once combined.
  - Forged group messages from non-members or with invalid signatures are rejected.
- **Required Unit/Integration Tests**:
  - `chat::group::tests::test_hlc_monotonicity_and_clock_skew_bound`
  - `chat::group::tests::test_causal_dag_topological_merge_determinism`
  - `chat::group::tests::test_group_key_sealing_and_non_member_rejection`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib chat::group
  ```

### Sub-phase 9.2: Group Anti-Entropy Frontier Sync & Pairwise Group Key Distribution (`lanflare-core::chat::sync`)
- **Scope**:
  - Implement anti-entropy DAG frontier synchronization (`POST /api/lanflare/v1/chat/group-sync`) in `lanflare-core::chat::sync`.
  - When a previously offline group member reconnects to the LAN or mesh, it exchanges its DAG `HEAD` frontier hashes with any online group peer and pulls missing historical messages and attachments.
- **Acceptance Criteria**:
  - In a 4-peer group where Peer D is offline while Peers A, B, and C exchange 20 messages, bringing Peer D online synchronizes all 20 missing messages in exact causal order within 1 sync round.
- **Required Unit/Integration Tests**:
  - `test_four_peer_group_partition_and_anti_entropy_catchup`
  - `test_group_sync_over_multi_hop_mesh_relay`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --test group_sync_e2e
  ```

### Sub-phase 9.3: Custodial Mesh Relay Mailbox for Asynchronous Offline Delivery (`lanflare-core::mesh::mailbox`)
- **Scope**:
  - Implement `RelayMailbox` (`POST /api/lanflare/v1/mesh/mailbox/deposit` and `POST /api/lanflare/v1/mesh/mailbox/fetch`) allowing a sender to deposit bounded E2EE message envelopes on a mutually reachable relay node when the recipient is currently offline.
  - Enforce strict per-recipient and total memory/disk ring-buffer quotas (`max_mailbox_bytes_per_peer`, `ttl_seconds`) and automatic delivery/eviction when the recipient joins the mesh.
- **Acceptance Criteria**:
  - Node A deposits an E2EE message on Relay B while Node C is offline; Node A then goes offline; Node C comes online and retrieves and decrypts the message from Relay B; Relay B never has access to the plaintext.
  - Deposits exceeding per-peer quota return HTTP 413 / 429 without starving other peers.
- **Required Unit/Integration Tests**:
  - `mesh::mailbox::tests::test_asynchronous_never_simultaneously_online_delivery`
  - `mesh::mailbox::tests::test_mailbox_quota_enforcement_and_ttl_eviction`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib mesh::mailbox
  ```

---

## Phase 10: Multi-Functional LAN Toolbox (Clipboard Ring, Federated Search, Scratchpad & Diagnostics)

### Sub-phase 10.1: "My Devices" Self-Trust Ring & Universal Mesh Clipboard Sync (`lanflare-core::toolbox::clipboard`)
- **Scope**:
  - Implement `DeviceRing` pairing and E2EE clipboard broadcast (`POST /api/lanflare/v1/toolbox/clipboard`) in `lanflare-core::toolbox::clipboard` and DTOs in `lanflare-proto::toolbox`.
  - Support text, image, and file-URI clipboard payloads with loop-suppression content hashing and sequence numbers across 1-hop LAN and multi-hop mesh.
- **Acceptance Criteria**:
  - Paired devices in the same `DeviceRing` synchronize clipboard updates with zero infinite echo loops, while unpaired or external contacts cannot read or inject clipboard state.
- **Required Unit/Integration Tests**:
  - `toolbox::clipboard::tests::test_device_ring_e2ee_clipboard_sync_and_echo_suppression`
  - `toolbox::clipboard::tests::test_unauthorized_peer_clipboard_rejection`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib toolbox::clipboard
  ```

### Sub-phase 10.2: Federated Mesh File Catalog & Distributed Dufs Search (`lanflare-core::toolbox::search`)
- **Scope**:
  - Implement `MeshFileCatalog` and `/api/lanflare/v1/toolbox/search` in `lanflare-core::toolbox::search`, indexing files shared via `DufsManager` and executing federated search queries across 1-hop and multi-hop mesh peers.
- **Acceptance Criteria**:
  - Searching for a filename substring returns ranked results across multiple online/mesh peers with direct download/stream URLs, respecting PIN-protected folder permissions.
- **Required Unit/Integration Tests**:
  - `toolbox::search::tests::test_local_index_and_federated_mesh_query_aggregation`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib toolbox::search
  ```

### Sub-phase 10.3: Collaborative CRDT Scratchpad & Multi-Hop Mesh Speedtest/Traceroute (`lanflare-core::toolbox::collab` & `lanflare-core::mesh::diagnostics`)
- **Scope**:
  - Implement conflict-free shared Markdown/code scratchpad (`ScratchpadDoc` using LWW/sequence CRDT) in `lanflare-core::toolbox::collab`.
  - Implement multi-hop traceroute and memory-to-memory streaming speedtest (`/api/lanflare/v1/mesh/benchmark`) in `lanflare-core::mesh::diagnostics` reporting per-hop RTT and aggregate throughput (Mbps).
- **Acceptance Criteria**:
  - Concurrent scratchpad edits from multiple peers converge to identical document text regardless of message arrival order.
  - Multi-hop traceroute and benchmark accurately report per-hop RTT latencies and throughput across a 3-hop simulated mesh.
- **Required Unit/Integration Tests**:
  - `toolbox::collab::tests::test_crdt_scratchpad_concurrent_convergence`
  - `mesh::diagnostics::tests::test_multi_hop_traceroute_and_bandwidth_benchmark`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-core --lib toolbox::collab && cargo test -p lanflare-core --lib mesh::diagnostics
  ```

---

## Phase 11: Desktop GUI & CLI Integration for Mesh, Chat & LAN Toolbox

### Sub-phase 11.1: CLI Subcommands for Mesh Topology, Simulator, Chat & Diagnostics (`lanflare-cli`)
- **Scope**:
  - Extend `lanflare-cli` with subcommands:
    - `lanflare mesh status` / `lanflare mesh sim` / `lanflare mesh trace <target>`
    - `lanflare chat send <target> <msg>` / `lanflare chat trust <target>` / `lanflare chat groups`
    - `lanflare search <query>`
- **Acceptance Criteria**:
  - All new CLI subcommands parse cleanly and execute against simulated and local nodes with formatted terminal tables/trees.
- **Required Unit/Integration Tests**:
  - `test_cli_mesh_and_chat_arg_parsing_and_execution`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-cli
  ```

### Sub-phase 11.2: Tauri v2 IPC Commands & Event Bridge for Mesh, Chat & Toolbox (`lanflare-desktop`)
- **Scope**:
  - Wire `MeshRouter`, `VirtualNetworkMatrix`, `ChatEngine`, `OutboxDispatcher`, and `Toolbox` into `DesktopState` (`crates/lanflare-desktop/src/state.rs`) and expose Tauri IPC commands in `crates/lanflare-desktop/src/commands.rs`.
  - Emit real-time Tauri events: `lanflare://mesh-updated`, `lanflare://chat-message`, `lanflare://chat-receipt`, `lanflare://trust-request`, and `lanflare://clipboard-synced`.
- **Acceptance Criteria**:
  - `lanflare-desktop` compiles with zero warnings and passes state/command integration tests for mesh simulation toggling, chat dispatch, and trust approval.
- **Required Unit/Integration Tests**:
  - `test_desktop_mesh_and_chat_state_commands`
- **Verification Command**:
  ```bash
  cargo test -p lanflare-desktop
  ```

### Sub-phase 11.3: Svelte 5 Interactive Mesh Topology Inspector & Virtual Sandbox View (`web/`)
- **Scope**:
  - Build `MeshTopologyView.svelte` using Svelte 5 runes (`$state`, `$derived`) rendering an interactive SVG/Canvas network graph of local node, 1-hop peers, multi-subnet bridge relays, and multi-hop peers.
  - Add route inspector panel, multi-hop ping/speedtest trigger, and interactive **"Virtual Mesh Sandbox"** control bar allowing users to spawn virtual subnets/nodes and toggle links live in the GUI.
- **Acceptance Criteria**:
  - Frontend passes `pnpm check` and `pnpm build` with zero TypeScript or Svelte compiler warnings.
- **Verification Command**:
  ```bash
  cd web && pnpm build
  ```

### Sub-phase 11.4: Svelte 5 Rich P2P/Group Chat View, Voice Recorder & LAN Toolbox UI (`web/`)
- **Scope**:
  - Build `ChatView.svelte` (1:1 and Group conversations, Stranger Request approval banner, offline outbox queue badges, delivery/read checkmarks, inline image/video player, microphone voice-note recorder, and resumable attachment progress bars) and `ToolboxView.svelte` (Universal Clipboard Ring, Federated File Search, and Live Collaborative Scratchpad).
  - Integrate navigation tabs into `web/src/routes/+page.svelte` for both desktop sidebar and mobile bottom navigation bar.
- **Acceptance Criteria**:
  - Full workspace passes `pnpm build`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --workspace`.
- **Verification Command**:
  ```bash
  cd web && pnpm build && cd .. && cargo test --workspace
  ```
