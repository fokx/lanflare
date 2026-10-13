# Hybrid Discovery Engine

LanFlare implements a 3-layer hybrid discovery mechanism engineered to reliably detect peers across heterogeneous networks, Wi-Fi AP isolation environments, and mobile power-saving restrictions.

---

## The 3-Layer Discovery Architecture

```
 Layer 1: UDP Multicast (Fast LAN Broadcast)
   └── IP: 224.0.0.167:53317
   └── Periodic announce bursts & listener

 Layer 2: Modern mDNS / DNS-SD (Zero-Config Service Discovery)
   └── Service: _lanflare._tcp.local.
   └── Standard RFC 6763 TXT records

 Layer 3: Active Subnet Scanner Fallback (Direct Probe)
   └── Concurrency: 128 parallel workers
   └── Fast 300ms TCP connect probe on candidate hosts
```

---

## Layer 1: UDP Multicast

- **Group Address**: `224.0.0.167`
- **Port**: `53317`
- **Mechanism**:
  - Outbound: Binds to each active physical network interface and sends `MulticastMessageV2` datagrams.
  - Inbound: Binds a reusable multicast socket on `0.0.0.0:53317`, joins the multicast group across all eligible interfaces, and decodes incoming packets.
- **Loopback & Self-Suppression**:
  - The engine discards packets whose embedded TLS certificate fingerprint matches the local node's fingerprint.

---

## Layer 2: Modern mDNS (DNS-SD)

- **Service Type**: `_lanflare._tcp.local.`
- **TXT Records**:
  - `alias`: Human-readable device name.
  - `version`: Protocol version (`2.2`).
  - `model`: Device model (e.g. `Linux`, `Windows`, `macOS`).
  - `devtype`: Device classification (`desktop`, `mobile`, `headless`).
  - `fingerprint`: Hex-encoded SHA-256 certificate digest.
  - `protocol`: Transfer protocol (`https` or `http`).
  - `port`: HTTP/HTTPS listening port.
  - `download`: Boolean flag indicating browser download support.

---

## Layer 3: Active Subnet Scan Fallback

### Problem Statement: The Mobile Discovery Challenge
In many real-world environments—especially corporate Wi-Fi, hotel networks, and modern Android devices—multicast and mDNS packets are blocked:
1. **Wi-Fi Access Point (AP) Isolation**: Drops L2 broadcast and multicast traffic between wireless clients.
2. **Android Power Management**: Aggressively suspends UDP multicast listeners to conserve battery.
3. **LocalSend Multicast Reply Semantics**: Official LocalSend receivers reply to multicast announcements via HTTP unicast (`POST /api/localsend/v2/register`) rather than broadcasting a UDP reply. If outbound multicast from the desktop is blocked by the router, the mobile device never receives the trigger.

### LanFlare Solution: Parallel Subnet Probing
LanFlare's `lanflare-core::discovery::scanner`:
- Inspects all active physical `/24` subnets (filtering out virtual adapters such as Docker, Tailscale CGNAT `100.64.0.0/10`, and VirtualBox).
- Probes all candidate host addresses `[1..=254]` concurrently using a bounded pool of **128 parallel workers**.
- Utilizes an ultra-fast **300ms connection timeout**. If a host does not have port 53317 open, the probe aborts immediately without attempting HTTP parsing.
- Probes respond to `POST /api/localsend/v2/register` and `GET /api/localsend/v2/info`, resolving peer metadata and TLS fingerprints within **< 1.2 seconds**.

---

## Unified PeerRegistry

All events from Multicast, mDNS, and Subnet Probes converge into `PeerRegistry`:
- **Deduplication**: Keyed by `fingerprint` (case-insensitive SHA-256). Multiple IP addresses (e.g., dual-band Wi-Fi + Ethernet) are aggregated into a single logical peer.
- **Primary IP Resolution**: Ranks reachable IP addresses, preferring local subnet addresses matching active interfaces.
- **Stale Eviction**: Background eviction task evicts peers that haven't sent a heartbeat within the configured TTL (default: 60 seconds).
