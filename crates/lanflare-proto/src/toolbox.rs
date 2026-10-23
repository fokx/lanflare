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
}
