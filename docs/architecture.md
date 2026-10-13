# System Architecture

LanFlare is designed as a modular, high-throughput, cross-platform file transfer system built in Rust. It prioritizes zero-copy streaming, low memory footprint, robust security, and seamless interoperability with the official LocalSend ecosystem.

---

## Workspace Crate Topology

The codebase is organized into a clean multi-crate Rust workspace:

```
                    ┌─────────────────────────┐
                    │        lanflare         │
                    │  (Terminal UI & Binary) │
                    └────────────┬────────────┘
                                 │
┌──────────────────────────┐     │     ┌──────────────────────────┐
│     lanflare-desktop     │     │     │     Future Extensions    │
│ (Tauri v2 + Svelte 5 GUI)│     │     │  (e.g., Daemon, Mobile)  │
└────────────┬─────────────┘     │     └────────────┬─────────────┘
             │                   │                  │
             └───────────────────┼──────────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │      lanflare-core      │
                    │  (Engine, Crypto, Net)  │
                    └────────────┬────────────┘
                                 │
                    ┌────────────▼────────────┐
                    │      lanflare-proto     │
                    │   (LocalSend v2.2 DTOs) │
                    └─────────────────────────┘
```

### 1. `lanflare-proto`
- **Responsibility**: Pure data representations and serde models for LocalSend v2.2 wire messages.
- **Dependencies**: Minimal (`serde`, `serde_json`, `thiserror`). No networking, no async runtime, no disk I/O.
- **Guarantees**: Fast compilation, deterministic serialization, and isolated protocol unit tests.

### 2. `lanflare-core`
- **Responsibility**: All operational mechanics of file sharing.
- **Key Modules**:
  - `crypto`: Ephemeral X.509 certificate generation, public key extraction, and SHA-256 fingerprint validation.
  - `discovery`: Dual multicast + mDNS engines, active subnet scanner, and thread-safe `PeerRegistry`.
  - `server`: Hyper v1 HTTP/HTTPS server with routing, session manager, and PIN verification.
  - `client`: High-throughput HTTP/HTTPS client and transfer orchestrator.
  - `storage`: Path sanitization, atomic streaming disk writer, and integrity verifier.

### 3. `lanflare`
- **Responsibility**: Interactive and scriptable terminal application.
- **Key Components**:
  - `clap v4` declarative argument parsing with subcommands: `status`, `scan`, `send`, `receive`.
  - `indicatif` streaming progress bars with transfer speed, ETA, and byte counters.
  - Interactive terminal peer selector.

### 4. `lanflare-desktop`
- **Responsibility**: Native cross-platform desktop application shell.
- **Architecture**:
  - Tauri v2 core integration with custom async state management.
  - Spawns background discovery and server tasks on the Tokio runtime without blocking the OS main loop.
  - Native system tray, system notifications, and file dialogs.
  - Web UI: SvelteKit 2 + Svelte 5 frontend with modern Runes reactivity (`$state`, `$derived`).

---

## Cryptographic & Security Model

```
       Sender (A)                                    Receiver (B)
 ┌──────────────────────┐                      ┌──────────────────────┐
 │ Ephemeral TLS Cert A │                      │ Ephemeral TLS Cert B │
 │ Fingerprint: SHA-256 │                      │ Fingerprint: SHA-256 │
 └──────────┬───────────┘                      └──────────┬───────────┘
            │                                             │
            │  1. Multicast / mDNS Announce (Fingerprint) │
            │ ──────────────────────────────────────────> │
            │                                             │
            │  2. HTTPS Handshake with Mutual TLS         │
            │ <=========================================> │
            │     - Verify Cert B matches Fingerprint B   │
            │     - Verify Cert A matches Fingerprint A   │
            │                                             │
            │  3. Optional PIN Token Authentication       │
            │ ──────────────────────────────────────────> │
            │                                             │
            │  4. Stream Encrypted Data (Socket -> Disk)  │
            │ ==========================================> │
```

1. **Ephemeral Self-Signed Certificates**:
   - On startup, a fresh in-memory self-signed TLS certificate is generated using `rcgen`.
   - The SHA-256 digest of the certificate's DER encoding serves as its public `fingerprint`.
2. **Mutual Verification (mTLS & TOFU)**:
   - When communicating over HTTPS, both client and server inspect peer certificates.
   - If a peer's fingerprint was previously broadcast in discovery, the connection validates that the presented certificate matches the broadcast fingerprint.
   - If not yet known, Trust-On-First-Use (TOFU) records the certificate fingerprint for subsequent transfer requests.
3. **Optional PIN Verification**:
   - Receiver can configure a 4-digit or 6-digit PIN.
   - Senders must obtain an authorized session token via `POST /api/localsend/v2/prepare-upload?pin=...`. Transfers without valid session tokens are rejected with HTTP 401/403.

---

## Threading & Runtime Architecture

LanFlare uses Tokio multi-threaded runtime with dedicated tasks:

- **Multicast Listener Task**: Bound to `0.0.0.0:53317` receiving UDP datagrams.
- **mDNS Service Task**: Handles DNS-SD queries and answers asynchronously.
- **Subnet Scanner Task**: Spawns bounded pools of 128 concurrent probe workers.
- **Hyper HTTP Engine**: Drives asynchronous HTTP/HTTPS connection acceptance, streaming incoming body chunks directly to disk without intermediate buffer accumulation.
