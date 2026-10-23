//! Protocol DTOs for LanFlare Multi-Functional LAN Toolbox (`/api/lanflare/v1/toolbox/*`).
//!
//! Provides protocol types, wire serialization, and data transfer objects for
//! "My Devices" self-trust ring pairing, E2EE clipboard synchronization, and
//! loop-suppression content hashing.

use std::fmt::Write;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::mesh::E2eeEnvelope;

/// Clipboard payload content variants supported across LanFlare clipboard sync.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClipboardPayloadDto {
    /// Plain UTF-8 text message or code snippet.
    Text {
        text: String,
    },
    /// Screenshot or copied image payload with MIME type and base64 encoded data.
    Image {
        mime_type: String,
        data_base64: String,
    },
    /// File list or URI references copied to clipboard.
    Files {
        files: Vec<String>,
    },
}

impl ClipboardPayloadDto {
    /// Constructs a plain text clipboard payload.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Constructs an image clipboard payload.
    pub fn image(mime_type: impl Into<String>, data_base64: impl Into<String>) -> Self {
        Self::Image {
            mime_type: mime_type.into(),
            data_base64: data_base64.into(),
        }
    }

    /// Constructs a files clipboard payload.
    pub fn files(files: Vec<String>) -> Self {
        Self::Files { files }
    }

    /// Returns a reference to the text if this payload is `Text`.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }

    /// Computes a canonical uppercase 64-character SHA-256 hex string of this payload.
    pub fn compute_content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        match self {
            Self::Text { text } => {
                hasher.update(b"text:");
                hasher.update(text.as_bytes());
            }
            Self::Image {
                mime_type,
                data_base64,
            } => {
                hasher.update(b"image:");
                hasher.update(mime_type.as_bytes());
                hasher.update(b":");
                hasher.update(data_base64.as_bytes());
            }
            Self::Files { files } => {
                hasher.update(b"files:");
                for (i, f) in files.iter().enumerate() {
                    if i > 0 {
                        hasher.update(b"|");
                    }
                    hasher.update(f.as_bytes());
                }
            }
        }
        let digest = hasher.finalize();
        let mut hex = String::with_capacity(64);
        for b in digest {
            let _ = write!(hex, "{:02X}", b);
        }
        hex
    }
}

/// Domain event representing a clipboard item authored by a device in the ring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardEventDto {
    /// Unique identifier for this clipboard event (e.g. UUID).
    pub event_id: String,

    /// Identifier of the personal DeviceRing this event belongs to.
    pub ring_id: String,

    /// Canonical uppercase SHA-256 fingerprint of the author device.
    pub origin_device_fp: String,

    /// Monotonic sequence number authored by `origin_device_fp`.
    pub seq_no: u64,

    /// UNIX timestamp in milliseconds when this event was created.
    pub timestamp_ms: u64,

    /// Canonical uppercase SHA-256 hash of the payload content.
    pub content_hash: String,

    /// Payload content (text, image, or files).
    pub payload: ClipboardPayloadDto,

    /// Optional digital signature from origin device over canonical signing bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl ClipboardEventDto {
    /// Creates a new `ClipboardEventDto` calculating the content hash automatically.
    pub fn new(
        event_id: impl Into<String>,
        ring_id: impl Into<String>,
        origin_device_fp: impl Into<String>,
        seq_no: u64,
        timestamp_ms: u64,
        payload: ClipboardPayloadDto,
    ) -> Self {
        let content_hash = payload.compute_content_hash();
        Self {
            event_id: event_id.into(),
            ring_id: ring_id.into(),
            origin_device_fp: origin_device_fp.into().to_ascii_uppercase(),
            seq_no,
            timestamp_ms,
            content_hash,
            payload,
            signature: None,
        }
    }

    /// Constructs canonical signing bytes for digital signature generation and verification.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.event_id,
            self.ring_id,
            self.origin_device_fp.to_ascii_uppercase(),
            self.seq_no,
            self.timestamp_ms,
            self.content_hash,
        )
        .into_bytes()
    }
}

/// Wire DTO for an end-to-end encrypted clipboard item broadcast across the ring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedClipboardDto {
    /// Identifier of the personal DeviceRing.
    pub ring_id: String,

    /// Canonical uppercase SHA-256 fingerprint of the origin device.
    pub origin_device_fp: String,

    /// Monotonic per-device sequence number.
    pub seq_no: u64,

    /// UNIX timestamp in milliseconds when authored.
    pub timestamp_ms: u64,

    /// Canonical uppercase SHA-256 hash of the plaintext payload.
    pub content_hash: String,

    /// Base64 encoded 12-byte random nonce for ChaCha20-Poly1305.
    pub nonce: String,

    /// Base64 encoded ciphertext + 16-byte Poly1305 authentication tag.
    pub ciphertext: String,

    /// Optional digital signature from origin device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl EncryptedClipboardDto {
    /// Constructs canonical signing bytes.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.ring_id,
            self.origin_device_fp.to_ascii_uppercase(),
            self.seq_no,
            self.timestamp_ms,
            self.content_hash,
            self.nonce,
            self.ciphertext,
        )
        .into_bytes()
    }
}

/// A device member participating in a personal "My Devices" self-trust ring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RingDeviceDto {
    /// Canonical uppercase SHA-256 certificate fingerprint.
    pub fingerprint: String,

    /// Human-readable device alias.
    pub alias: String,

    /// Optional PEM-encoded X.509 certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pem: Option<String>,

    /// UNIX timestamp in milliseconds when paired into the ring.
    pub paired_at_ms: u64,
}

impl RingDeviceDto {
    /// Creates a new `RingDeviceDto`.
    pub fn new(
        fingerprint: impl Into<String>,
        alias: impl Into<String>,
        cert_pem: Option<String>,
        paired_at_ms: u64,
    ) -> Self {
        Self {
            fingerprint: fingerprint.into().to_ascii_uppercase(),
            alias: alias.into(),
            cert_pem,
            paired_at_ms,
        }
    }
}

/// Membership roster of a personal `DeviceRing`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRingRosterDto {
    /// Unique identifier for this device ring.
    pub ring_id: String,

    /// User-friendly name for this ring (e.g. "My Devices").
    pub ring_name: String,

    /// Monotonic roster epoch / version.
    pub epoch: u64,

    /// Member devices in this ring.
    pub devices: Vec<RingDeviceDto>,

    /// UNIX timestamp in milliseconds of last roster modification.
    pub updated_at_ms: u64,

    /// Optional digital signature over canonical roster bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl DeviceRingRosterDto {
    /// Creates a new empty `DeviceRingRosterDto`.
    pub fn new(ring_id: impl Into<String>, ring_name: impl Into<String>) -> Self {
        Self {
            ring_id: ring_id.into(),
            ring_name: ring_name.into(),
            epoch: 1,
            devices: Vec::new(),
            updated_at_ms: 0,
            signature: None,
        }
    }

    /// Checks if a device fingerprint is a member of this ring (case-insensitive).
    pub fn is_member(&self, fp: &str) -> bool {
        self.devices
            .iter()
            .any(|d| d.fingerprint.eq_ignore_ascii_case(fp))
    }

    /// Looks up a member device by fingerprint (case-insensitive).
    pub fn get_device(&self, fp: &str) -> Option<&RingDeviceDto> {
        self.devices
            .iter()
            .find(|d| d.fingerprint.eq_ignore_ascii_case(fp))
    }

    /// Adds or updates a device in this roster, incrementing the epoch.
    pub fn add_device(&mut self, device: RingDeviceDto) {
        let fp = device.fingerprint.to_ascii_uppercase();
        if let Some(existing) = self
            .devices
            .iter_mut()
            .find(|d| d.fingerprint.eq_ignore_ascii_case(&fp))
        {
            *existing = device;
        } else {
            self.devices.push(device);
        }
        self.epoch = self.epoch.saturating_add(1);
    }

    /// Removes a device from this roster, returning whether it was present.
    pub fn remove_device(&mut self, fp: &str) -> bool {
        let initial_len = self.devices.len();
        self.devices
            .retain(|d| !d.fingerprint.eq_ignore_ascii_case(fp));
        if self.devices.len() < initial_len {
            self.epoch = self.epoch.saturating_add(1);
            true
        } else {
            false
        }
    }

    /// Constructs canonical signing bytes for this roster.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        let mut fps: Vec<String> = self
            .devices
            .iter()
            .map(|d| d.fingerprint.to_ascii_uppercase())
            .collect();
        fps.sort();
        format!(
            "{}:{}:{}:{}",
            self.ring_id,
            self.ring_name,
            self.epoch,
            fps.join(",")
        )
        .into_bytes()
    }
}

/// Pairwise envelope distributing a shared `RingSecret` to a newly paired device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RingKeyDistributionDto {
    /// Identifier of the DeviceRing.
    pub ring_id: String,

    /// Epoch number of the distributed secret.
    pub epoch: u64,

    /// Intended recipient device's canonical uppercase fingerprint.
    pub recipient_fp: String,

    /// Author sender device's canonical uppercase fingerprint.
    pub sender_fp: String,

    /// Sealed E2EE envelope encapsulating the 32-byte symmetric ring key.
    pub envelope: E2eeEnvelope,

    /// UNIX timestamp in milliseconds when created.
    pub created_at_ms: u64,
}

/// Response returned by `POST /api/lanflare/v1/toolbox/clipboard`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardResponseDto {
    /// Status: "applied" or "suppressed".
    pub status: String,

    /// Optional reason if suppressed (e.g. "echo_self", "echo_hash", "echo_seq").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// Canonical uppercase SHA-256 hash of the content.
    pub content_hash: String,
}

/// Query request sent to search indexed files across LAN and mesh peers (`POST /api/lanflare/v1/toolbox/search`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchQueryDto {
    /// Search keyword, pattern, or substring.
    pub query: String,

    /// Maximum number of hits to return (defaults to 50 if None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<usize>,

    /// Canonical uppercase SHA-256 fingerprint of the querying peer.
    pub requester_fp: String,

    /// Optional authentication PIN if searching protected folders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_pin: Option<String>,

    /// UNIX timestamp in milliseconds when query was initiated.
    pub timestamp_ms: u64,

    /// Optional digital signature over canonical query bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl FileSearchQueryDto {
    /// Creates a new search query for the given keyword and requester fingerprint.
    pub fn new(query: impl Into<String>, requester_fp: impl Into<String>) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            query: query.into(),
            max_results: None,
            requester_fp: requester_fp.into().to_ascii_uppercase(),
            auth_pin: None,
            timestamp_ms: now_ms,
            signature: None,
        }
    }

    /// Sets the maximum results limit.
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = Some(max);
        self
    }

    /// Sets the authentication PIN.
    pub fn with_pin(mut self, pin: impl Into<String>) -> Self {
        self.auth_pin = Some(pin.into());
        self
    }

    /// Constructs canonical bytes for digital signature generation/verification.
    pub fn canonical_signing_bytes(&self) -> Vec<u8> {
        format!(
            "{}:{}:{}:{}",
            self.query,
            self.requester_fp.to_ascii_uppercase(),
            self.timestamp_ms,
            self.auth_pin.as_deref().unwrap_or("")
        )
        .into_bytes()
    }
}

/// A single file match returned in search results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitDto {
    /// File name (e.g. `document.pdf`).
    pub file_name: String,

    /// Path relative to the shared folder root (e.g. `/docs/document.pdf`).
    pub relative_path: String,

    /// File size in bytes.
    pub size_bytes: u64,

    /// Modification timestamp in milliseconds.
    pub modified_at_ms: u64,

    /// MIME type string (e.g. `application/pdf`).
    pub mime_type: String,

    /// Optional SHA-256 hash if indexed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Whether this file requires an authentication PIN to download or access.
    pub requires_pin: bool,

    /// Direct stream or download URL (relative e.g. `/dufs/...` or absolute `http://...`).
    pub download_url: String,

    /// Relevance score (0.0 to 1.0) for ranking matches.
    pub score: f32,
}

impl SearchHitDto {
    /// Creates a new `SearchHitDto`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        file_name: impl Into<String>,
        relative_path: impl Into<String>,
        size_bytes: u64,
        modified_at_ms: u64,
        mime_type: impl Into<String>,
        requires_pin: bool,
        download_url: impl Into<String>,
        score: f32,
    ) -> Self {
        Self {
            file_name: file_name.into(),
            relative_path: relative_path.into(),
            size_bytes,
            modified_at_ms,
            mime_type: mime_type.into(),
            sha256: None,
            requires_pin,
            download_url: download_url.into(),
            score,
        }
    }
}

/// Response returned by a peer responding to a `FileSearchQueryDto`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSearchResultDto {
    /// Canonical uppercase SHA-256 fingerprint of the responding node.
    pub responder_fp: String,

    /// Human-readable device alias of the responding node.
    pub responder_alias: String,

    /// List of matched search hits.
    pub hits: Vec<SearchHitDto>,

    /// Total number of indexed files on this peer.
    pub total_indexed: usize,

    /// Execution duration in milliseconds for this search.
    pub search_duration_ms: u64,
}

impl FileSearchResultDto {
    /// Creates a new empty `FileSearchResultDto`.
    pub fn new(responder_fp: impl Into<String>, responder_alias: impl Into<String>) -> Self {
        Self {
            responder_fp: responder_fp.into().to_ascii_uppercase(),
            responder_alias: responder_alias.into(),
            hits: Vec::new(),
            total_indexed: 0,
            search_duration_ms: 0,
        }
    }

    /// Sorts hits in descending order by relevance score.
    pub fn sort_by_score_descending(&mut self) {
        self.hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.file_name.cmp(&b.file_name))
        });
    }
}

/// Summary or snapshot of a node's shared files catalog across the mesh.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshFileCatalogDto {
    /// Canonical uppercase SHA-256 fingerprint of the catalog owner.
    pub owner_fp: String,

    /// Device alias of the owner.
    pub owner_alias: String,

    /// Catalog epoch / version number.
    pub catalog_epoch: u64,

    /// Names of shared folder roots.
    pub shared_roots: Vec<String>,

    /// List of indexed catalog items.
    pub items: Vec<SearchHitDto>,

    /// UNIX timestamp in milliseconds when catalog was updated.
    pub updated_at_ms: u64,
}

impl MeshFileCatalogDto {
    /// Creates a new `MeshFileCatalogDto`.
    pub fn new(
        owner_fp: impl Into<String>,
        owner_alias: impl Into<String>,
        catalog_epoch: u64,
    ) -> Self {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self {
            owner_fp: owner_fp.into().to_ascii_uppercase(),
            owner_alias: owner_alias.into(),
            catalog_epoch,
            shared_roots: Vec::new(),
            items: Vec::new(),
            updated_at_ms: now_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clipboard_payload_hashing_and_serde() {
        let text_payload = ClipboardPayloadDto::text("fn main() { println!(\"LanFlare\"); }");
        let hash1 = text_payload.compute_content_hash();
        assert_eq!(hash1.len(), 64);
        assert!(hash1.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic
        assert_eq!(hash1, text_payload.compute_content_hash());

        let json = serde_json::to_string(&text_payload).unwrap();
        let decoded: ClipboardPayloadDto = serde_json::from_str(&json).unwrap();
        assert_eq!(text_payload, decoded);
        assert_eq!(decoded.as_text(), Some("fn main() { println!(\"LanFlare\"); }"));

        let img_payload = ClipboardPayloadDto::image("image/png", "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=");
        let img_hash = img_payload.compute_content_hash();
        assert_ne!(hash1, img_hash);
        assert_eq!(img_payload.as_text(), None);

        let files_payload = ClipboardPayloadDto::files(vec![
            "/tmp/file1.txt".into(),
            "/tmp/file2.png".into(),
        ]);
        let files_hash = files_payload.compute_content_hash();
        assert_ne!(hash1, files_hash);
        assert_ne!(img_hash, files_hash);
    }

    #[test]
    fn test_clipboard_event_and_roster_serde() {
        let payload = ClipboardPayloadDto::text("Test sync");
        let event = ClipboardEventDto::new(
            "evt-123",
            "ring-abc",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            1,
            1700000000000,
            payload.clone(),
        );

        let json = serde_json::to_string(&event).unwrap();
        let decoded: ClipboardEventDto = serde_json::from_str(&json).unwrap();
        assert_eq!(event, decoded);
        assert_eq!(event.canonical_signing_bytes(), decoded.canonical_signing_bytes());

        let mut roster = DeviceRingRosterDto::new("ring-abc", "My Personal Ring");
        assert_eq!(roster.epoch, 1);
        assert!(!roster.is_member("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));

        roster.add_device(RingDeviceDto::new(
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "Workstation",
            None,
            1700000000000,
        ));
        assert_eq!(roster.epoch, 2);
        assert!(roster.is_member("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));

        let roster_json = serde_json::to_string(&roster).unwrap();
        let decoded_roster: DeviceRingRosterDto = serde_json::from_str(&roster_json).unwrap();
        assert_eq!(roster, decoded_roster);

        assert!(roster.remove_device("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
        assert_eq!(roster.epoch, 3);
        assert!(!roster.is_member("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"));
    }

    #[test]
    fn test_file_search_dtos_and_ranking_serde() {
        let query = FileSearchQueryDto::new("report", "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB")
            .with_max_results(20)
            .with_pin("1234");
        assert_eq!(query.max_results, Some(20));
        assert_eq!(query.auth_pin.as_deref(), Some("1234"));
        assert!(!query.canonical_signing_bytes().is_empty());

        let json = serde_json::to_string(&query).unwrap();
        let decoded: FileSearchQueryDto = serde_json::from_str(&json).unwrap();
        assert_eq!(query, decoded);

        let hit1 = SearchHitDto::new(
            "report_q1.pdf",
            "/finance/report_q1.pdf",
            1048576,
            1700000000000,
            "application/pdf",
            false,
            "/dufs/finance/report_q1.pdf",
            0.85,
        );
        let hit2 = SearchHitDto::new(
            "report.txt",
            "/notes/report.txt",
            2048,
            1700000001000,
            "text/plain",
            false,
            "/dufs/notes/report.txt",
            1.0,
        );
        let hit3 = SearchHitDto::new(
            "old_report.doc",
            "/archive/old_report.doc",
            4096,
            1600000000000,
            "application/msword",
            true,
            "/dufs/archive/old_report.doc",
            0.5,
        );

        let mut res = FileSearchResultDto::new(
            "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC",
            "FileServerNode",
        );
        res.hits.push(hit1.clone());
        res.hits.push(hit2.clone());
        res.hits.push(hit3.clone());
        res.total_indexed = 150;
        res.search_duration_ms = 4;

        res.sort_by_score_descending();
        assert_eq!(res.hits[0].file_name, "report.txt");
        assert_eq!(res.hits[1].file_name, "report_q1.pdf");
        assert_eq!(res.hits[2].file_name, "old_report.doc");

        let res_json = serde_json::to_string(&res).unwrap();
        let decoded_res: FileSearchResultDto = serde_json::from_str(&res_json).unwrap();
        assert_eq!(res, decoded_res);
    }

    #[test]
    fn test_mesh_file_catalog_dto_serde() {
        let mut catalog = MeshFileCatalogDto::new(
            "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD",
            "DesktopHost",
            1,
        );
        catalog.shared_roots.push("PublicDocs".into());
        catalog.items.push(SearchHitDto::new(
            "readme.md",
            "/readme.md",
            512,
            1700000000000,
            "text/markdown",
            false,
            "/dufs/readme.md",
            1.0,
        ));

        let json = serde_json::to_string(&catalog).unwrap();
        let decoded: MeshFileCatalogDto = serde_json::from_str(&json).unwrap();
        assert_eq!(catalog, decoded);
    }
}
