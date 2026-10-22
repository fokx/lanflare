//! Protocol DTOs for LanFlare Multi-Hop Overlay Mesh (`/api/lanflare/v1/mesh/*`).

use serde::{Deserialize, Serialize};

/// Maximum Time-To-Live (hop count) for mesh routed envelopes and route advertisements.
pub const MAX_MESH_TTL: u8 = 5;

/// Default penalty added to route cost per hop in Dijkstra path selection (ms).
pub const DEFAULT_HOP_PENALTY_MS: u32 = 15;

/// Default buffer and streaming chunk size for cut-through stream relay (128 KB).
pub const DEFAULT_STREAM_CHUNK_SIZE: usize = 128 * 1024;

/// Cryptographically signed descriptor of a mesh node.
///
/// Any node N hops away can independently verify that `fingerprint == SHA256(DER(cert_pem))`
/// and verify `signature` over the canonical announcement bytes using the public key in `cert_pem`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshNodeAnnouncement {
    /// Canonical uppercase SHA-256 hex string (64 characters).
    pub fingerprint: String,

    /// Human-readable device alias.
    pub alias: String,

    /// PEM-encoded self-signed X.509 certificate.
    pub cert_pem: String,

    /// Advertised capabilities (e.g. `["relay", "chat", "stream", "mailbox"]`).
    pub capabilities: Vec<String>,

    /// Whether this node acts as a multi-hop packet/stream forwarding relay.
    pub relay_enabled: bool,

    /// Monotonically increasing sequence number for freshness and replay defense.
    pub seq_no: u64,

    /// Timestamp of announcement creation in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,

    /// Base64-encoded digital signature over canonical announcement bytes.
    pub signature: String,
}

impl MeshNodeAnnouncement {
    /// Computes the canonical byte sequence to sign or verify.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        let caps = self.capabilities.join(",");
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.fingerprint.to_ascii_uppercase(),
            self.alias,
            self.cert_pem.trim(),
            caps,
            self.relay_enabled,
            self.seq_no,
            self.timestamp_ms
        )
        .into_bytes()
    }
}

/// Link performance metric measured between two adjacent 1-hop mesh peers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshLinkMetric {
    /// SHA-256 fingerprint of the source node.
    pub src_fp: String,

    /// SHA-256 fingerprint of the destination node.
    pub dst_fp: String,

    /// Measured round-trip latency in milliseconds.
    pub rtt_ms: u32,

    /// Optional network subnet / interface identifier (e.g. "wlan0", "eth0", "192.168.1.0/24").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet_tag: Option<String>,

    /// Estimated link throughput in kilobits per second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bandwidth_kbps: Option<u32>,

    /// Timestamp of the last successful ping or handshake probe (ms since UNIX epoch).
    pub last_seen_ms: u64,
}

/// Path-vector route advertisement gossiped across the overlay mesh.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshRouteAdvertisement {
    /// SHA-256 fingerprint of the origin destination node.
    pub origin_fp: String,

    /// Origin node's sequence number for freshness verification.
    pub seq_no: u64,

    /// Accumulated composite path cost: sum of (HOP_PENALTY_MS + rtt_ms).
    pub cost: u32,

    /// Ordered list of traversed peer fingerprints (path-vector loop prevention).
    pub route_path: Vec<String>,

    /// Remaining hop count allowed before packet drop.
    pub ttl: u8,
}

/// Gossip packet exchanged between mesh peers via `POST /api/lanflare/v1/mesh/gossip`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshGossipPacket {
    /// SHA-256 fingerprint of the sender node transmitting this gossip batch.
    pub sender_fp: String,

    /// Signed node announcements learned or updated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub announcements: Vec<MeshNodeAnnouncement>,

    /// Measured 1-hop link metrics for topology graph edge weighting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link_metrics: Vec<MeshLinkMetric>,

    /// Active route advertisements.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route_advertisements: Vec<MeshRouteAdvertisement>,
}

/// End-to-End Encrypted (E2EE) envelope protecting inner payloads from untrusted relays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct E2eeEnvelope {
    /// SHA-256 fingerprint of the origin sender (uppercase 64-char hex).
    pub sender_fp: String,

    /// SHA-256 fingerprint of the intended recipient (uppercase 64-char hex).
    pub recipient_fp: String,

    /// Optional sender certificate PEM for standalone self-contained verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_cert_pem: Option<String>,

    /// Base64-encoded encrypted symmetric key (sealed with recipient's public key).
    pub encrypted_key: String,

    /// Base64-encoded 12-byte AEAD nonce.
    pub nonce: String,

    /// Base64-encoded ciphertext with appended 16-byte Poly1305 authentication tag.
    pub ciphertext: String,

    /// Timestamp when sealed in milliseconds since UNIX epoch.
    pub timestamp_ms: u64,

    /// Base64-encoded digital signature by sender over envelope metadata & ciphertext.
    pub signature: String,
}

impl E2eeEnvelope {
    /// Computes canonical byte sequence for signing and verifying envelope integrity.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.sender_fp.to_ascii_uppercase(),
            self.recipient_fp.to_ascii_uppercase(),
            self.encrypted_key,
            self.nonce,
            self.ciphertext,
            self.timestamp_ms
        )
        .into_bytes()
    }
}

/// Multi-hop routed envelope carrying an `E2eeEnvelope` across intermediate relays.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutedEnvelope {
    /// Unique identifier for deduplication (e.g. UUID v4).
    pub packet_id: String,

    /// Origin sender fingerprint.
    pub src_fp: String,

    /// Ultimate destination fingerprint.
    pub dst_fp: String,

    /// Remaining hop count limit.
    pub ttl: u8,

    /// Ordered path of traversed nodes (for loop detection and return path).
    pub route_path: Vec<String>,

    /// Encrypted E2EE payload invisible to intermediate relays.
    pub payload: E2eeEnvelope,
}

/// Chunk header for multi-hop cut-through file streaming via `POST /api/lanflare/v1/mesh/stream`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshStreamHeader {
    /// Unique stream session identifier.
    pub stream_id: String,

    /// Origin sender fingerprint.
    pub src_fp: String,

    /// Ultimate destination fingerprint.
    pub dst_fp: String,

    /// File identifier being streamed.
    pub file_id: String,

    /// Total promised byte size of the file.
    pub total_size: u64,

    /// Planned route path (fingerprints) through relays.
    pub route_path: Vec<String>,

    /// Monotonically increasing chunk sequence index (0, 1, 2, ...).
    pub chunk_index: u64,

    /// Flag indicating whether this is the terminal chunk of the stream.
    pub is_last_chunk: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_dto_serde_roundtrip() {
        let announcement = MeshNodeAnnouncement {
            fingerprint: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            alias: "Mesh Node Alpha".into(),
            cert_pem: "-----BEGIN CERTIFICATE-----\nMIIB...\n-----END CERTIFICATE-----\n".into(),
            capabilities: vec!["relay".into(), "chat".into(), "stream".into()],
            relay_enabled: true,
            seq_no: 42,
            timestamp_ms: 1770000000000,
            signature: "AQIDBAUGBwgJCgsMDQ4PEA==".into(),
        };

        let link_metric = MeshLinkMetric {
            src_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            dst_fp: "FFEEDDCCBBAA00998877665544332211FFEEDDCCBBAA00998877665544332211".into(),
            rtt_ms: 12,
            subnet_tag: Some("192.168.1.0/24".into()),
            bandwidth_kbps: Some(100_000),
            last_seen_ms: 1770000005000,
        };

        let route_ad = MeshRouteAdvertisement {
            origin_fp: "FFEEDDCCBBAA00998877665544332211FFEEDDCCBBAA00998877665544332211".into(),
            seq_no: 10,
            cost: 27,
            route_path: vec![
                "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
                "AABBCCDDEEFF11223344556677889900AABBCCDDEEFF11223344556677889900".into(),
            ],
            ttl: 4,
        };

        let gossip = MeshGossipPacket {
            sender_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            announcements: vec![announcement.clone()],
            link_metrics: vec![link_metric.clone()],
            route_advertisements: vec![route_ad.clone()],
        };

        let envelope = E2eeEnvelope {
            sender_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            recipient_fp: "FFEEDDCCBBAA00998877665544332211FFEEDDCCBBAA00998877665544332211".into(),
            sender_cert_pem: Some("-----BEGIN CERTIFICATE-----\n...\n-----END CERTIFICATE-----".into()),
            encrypted_key: "S0VZREVSSVZFRA==".into(),
            nonce: "Tk9OQ0UxMjM0NTY3".into(),
            ciphertext: "Q0lQSEVSVEVYVFRBRw==".into(),
            timestamp_ms: 1770000010000,
            signature: "U0lHTkFUVVJFCg==".into(),
        };

        let routed = RoutedEnvelope {
            packet_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            src_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            dst_fp: "FFEEDDCCBBAA00998877665544332211FFEEDDCCBBAA00998877665544332211".into(),
            ttl: 3,
            route_path: vec!["11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into()],
            payload: envelope.clone(),
        };

        let stream_header = MeshStreamHeader {
            stream_id: "stream-uuid-1234".into(),
            src_fp: "11223344556677889900AABBCCDDEEFF11223344556677889900AABBCCDDEEFF".into(),
            dst_fp: "FFEEDDCCBBAA00998877665544332211FFEEDDCCBBAA00998877665544332211".into(),
            file_id: "file-99".into(),
            total_size: 104857600,
            route_path: vec!["RELAY_B".into(), "RELAY_C".into()],
            chunk_index: 0,
            is_last_chunk: false,
        };

        // Validate JSON serialization & deserialization round-trip
        let json_announcement = serde_json::to_string(&announcement).unwrap();
        let de_announcement: MeshNodeAnnouncement = serde_json::from_str(&json_announcement).unwrap();
        assert_eq!(announcement, de_announcement);

        let json_link = serde_json::to_string(&link_metric).unwrap();
        let de_link: MeshLinkMetric = serde_json::from_str(&json_link).unwrap();
        assert_eq!(link_metric, de_link);

        let json_route = serde_json::to_string(&route_ad).unwrap();
        let de_route: MeshRouteAdvertisement = serde_json::from_str(&json_route).unwrap();
        assert_eq!(route_ad, de_route);

        let json_gossip = serde_json::to_string(&gossip).unwrap();
        let de_gossip: MeshGossipPacket = serde_json::from_str(&json_gossip).unwrap();
        assert_eq!(gossip, de_gossip);

        let json_env = serde_json::to_string(&envelope).unwrap();
        let de_env: E2eeEnvelope = serde_json::from_str(&json_env).unwrap();
        assert_eq!(envelope, de_env);

        let json_routed = serde_json::to_string(&routed).unwrap();
        let de_routed: RoutedEnvelope = serde_json::from_str(&json_routed).unwrap();
        assert_eq!(routed, de_routed);

        let json_stream = serde_json::to_string(&stream_header).unwrap();
        let de_stream: MeshStreamHeader = serde_json::from_str(&json_stream).unwrap();
        assert_eq!(stream_header, de_stream);
    }
}
