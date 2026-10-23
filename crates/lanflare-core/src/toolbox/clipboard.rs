//! "My Devices" self-trust ring and E2EE universal mesh clipboard synchronization.
//!
//! Sub-phase 10.1 implementation:
//! - Implements `DeviceRing` pairing and shared 32-byte ring secret distribution (`RingSecret`, `RingKeyDistributionDto`).
//! - E2EE clipboard broadcast via ChaCha20-Poly1305 symmetric encryption and X.509 digital signatures.
//! - Endpoint `POST /api/lanflare/v1/toolbox/clipboard` receiving encrypted clipboard updates.
//! - Defense against infinite echo loops using:
//!   1. Origin device filtering (`origin_device_fp == local_device_fp`).
//!   2. SHA-256 content hashing (`current_content_hash` and bounded `recent_hashes`).
//!   3. Monotonic per-device sequence numbers (`last_seen_seq`).
//! - Support for `Text`, `Image`, and `Files` clipboard payloads.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use lanflare_proto::toolbox::{
    ClipboardEventDto, ClipboardPayloadDto, ClipboardResponseDto, DeviceRingRosterDto,
    EncryptedClipboardDto, RingDeviceDto, RingKeyDistributionDto,
};
use ring::aead::{
    Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, UnboundKey, CHACHA20_POLY1305,
};
use ring::rand::SecureRandom;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::crypto::cert::TlsIdentity;
use crate::crypto::e2ee::{open_envelope, seal_envelope, sign_with_identity, verify_signature_with_cert, E2eeError};
use crate::discovery::registry::DiscoveredPeer;
use crate::mesh::router::MAX_CONTROL_BODY_SIZE;
use crate::server::dufs::{empty_box_response, full_box_body, json_response, ServerResponseBody};
use crate::server::router::ClientConnectionInfo;

/// Maximum number of recent content hashes tracked for echo suppression.
pub const MAX_RECENT_HASHES: usize = 128;

/// Errors arising during clipboard synchronization or device ring operations.
#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("Unknown or mismatched ring: expected {expected}, got {got}")]
    MismatchedRing { expected: String, got: String },

    #[error("Peer {0} is unauthorized; not in ring roster")]
    UnauthorizedPeer(String),

    #[error("AEAD decryption or integrity check failed: {0}")]
    DecryptionFailed(String),

    #[error("Key serialization or generation error: {0}")]
    KeyError(String),

    #[error("Invalid digital signature: {0}")]
    InvalidSignature(String),

    #[error("Content hash mismatch: expected {expected}, computed {computed}")]
    ContentHashMismatch { expected: String, computed: String },

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("E2EE envelope error: {0}")]
    E2ee(#[from] E2eeError),

    #[error("Network transfer error: {0}")]
    Network(String),
}

/// Helper struct providing single-nonce consumption for `ring::aead`.
struct SingleNonce(Option<[u8; 12]>);

impl NonceSequence for SingleNonce {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        self.0
            .take()
            .map(Nonce::assume_unique_for_key)
            .ok_or(ring::error::Unspecified)
    }
}

/// 32-byte symmetric secret key shared among paired devices in a `DeviceRing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RingSecret {
    key_bytes: [u8; 32],
}

impl RingSecret {
    /// Generates a fresh random 32-byte symmetric key using system CSPRNG.
    pub fn generate() -> Result<Self, ClipboardError> {
        let mut key_bytes = [0u8; 32];
        let rng = ring::rand::SystemRandom::new();
        rng.fill(&mut key_bytes)
            .map_err(|_| ClipboardError::KeyError("CSPRNG failure generating RingSecret".into()))?;
        Ok(Self { key_bytes })
    }

    /// Constructs a `RingSecret` from raw 32 bytes.
    pub fn from_raw(key_bytes: [u8; 32]) -> Self {
        Self { key_bytes }
    }

    /// Returns a reference to the raw 32-byte array.
    pub fn key_bytes(&self) -> &[u8; 32] {
        &self.key_bytes
    }

    /// Derives a 32-byte `RingSecret` deterministically from a ring ID and user PIN / passphrase.
    pub fn derive_from_pin(ring_id: &str, pin: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"lanflare-ring-v1:");
        hasher.update(ring_id.as_bytes());
        hasher.update(b":");
        hasher.update(pin.as_bytes());
        let digest = hasher.finalize();
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&digest);
        Self { key_bytes }
    }

    /// Encrypts plaintext bytes using ChaCha20-Poly1305 with AAD binding ring ID, origin, and sequence number.
    ///
    /// Returns `(nonce_base64, ciphertext_base64)`.
    pub fn encrypt(
        &self,
        ring_id: &str,
        origin_fp: &str,
        seq_no: u64,
        plaintext: &[u8],
    ) -> Result<(String, String), ClipboardError> {
        let mut nonce = [0u8; 12];
        let rng = ring::rand::SystemRandom::new();
        rng.fill(&mut nonce)
            .map_err(|_| ClipboardError::KeyError("CSPRNG failure generating nonce".into()))?;

        let aad_str = format!("{}:{}:{}", ring_id, origin_fp.to_ascii_uppercase(), seq_no);
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.key_bytes)
            .map_err(|_| ClipboardError::KeyError("Invalid ChaCha20 symmetric key".into()))?;
        let mut sealing_key = SealingKey::new(unbound, SingleNonce(Some(nonce)));

        let mut ciphertext = plaintext.to_vec();
        sealing_key
            .seal_in_place_append_tag(Aad::from(aad_str.as_bytes()), &mut ciphertext)
            .map_err(|_| ClipboardError::DecryptionFailed("ChaCha20 seal failed".into()))?;

        Ok((STANDARD.encode(nonce), STANDARD.encode(&ciphertext)))
    }

    /// Decrypts ChaCha20-Poly1305 ciphertext using the shared ring secret and AAD.
    pub fn decrypt(
        &self,
        ring_id: &str,
        origin_fp: &str,
        seq_no: u64,
        nonce_base64: &str,
        ciphertext_base64: &str,
    ) -> Result<Vec<u8>, ClipboardError> {
        let nonce_bytes = STANDARD
            .decode(nonce_base64)
            .map_err(|e| ClipboardError::DecryptionFailed(format!("Invalid base64 nonce: {e}")))?;
        if nonce_bytes.len() != 12 {
            return Err(ClipboardError::DecryptionFailed(
                "Nonce must be exactly 12 bytes".into(),
            ));
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&nonce_bytes);

        let mut ciphertext = STANDARD
            .decode(ciphertext_base64)
            .map_err(|e| ClipboardError::DecryptionFailed(format!("Invalid base64 ciphertext: {e}")))?;
        if ciphertext.len() < 16 {
            return Err(ClipboardError::DecryptionFailed(
                "Ciphertext shorter than 16-byte tag".into(),
            ));
        }

        let aad_str = format!("{}:{}:{}", ring_id, origin_fp.to_ascii_uppercase(), seq_no);
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.key_bytes)
            .map_err(|_| ClipboardError::KeyError("Invalid ChaCha20 key for decryption".into()))?;
        let mut opening_key = OpeningKey::new(unbound, SingleNonce(Some(nonce)));

        let decrypted = opening_key
            .open_in_place(Aad::from(aad_str.as_bytes()), &mut ciphertext)
            .map_err(|_| {
                ClipboardError::DecryptionFailed("ChaCha20-Poly1305 authentication failed".into())
            })?;

        Ok(decrypted.to_vec())
    }
}

/// Outcome of handling an incoming clipboard synchronization event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandleClipboardResult {
    /// Clipboard content was successfully decrypted, authenticated, and applied.
    Applied(ClipboardEventDto),
    /// Suppressed because the origin device is self (loopback suppression).
    SuppressedEchoSelf,
    /// Suppressed because content hash matches current clipboard or recent history.
    SuppressedEchoHash,
    /// Suppressed because sequence number is stale or already seen.
    SuppressedEchoSeq { last_seen: u64, received: u64 },
}

/// Internal mutable state of the personal `DeviceRing`.
struct DeviceRingInner {
    roster: DeviceRingRosterDto,
    secret: RingSecret,
    current_seq_no: u64,
    last_seen_seq: HashMap<String, u64>,
    current_content_hash: Option<String>,
    current_payload: Option<ClipboardPayloadDto>,
    recent_hashes: HashSet<String>,
    recent_hashes_order: VecDeque<String>,
}

/// Personal "My Devices" self-trust ring managing E2EE clipboard synchronization.
pub struct DeviceRing {
    ring_id: String,
    ring_name: String,
    local_identity: TlsIdentity,
    inner: RwLock<DeviceRingInner>,
    event_tx: broadcast::Sender<ClipboardEventDto>,
    client: reqwest::Client,
    mesh_router: RwLock<Option<Arc<crate::mesh::router::MeshRouter>>>,
}

impl DeviceRing {
    /// Creates a new `DeviceRing` with a shared `RingSecret`, registering the local device.
    pub fn new(
        ring_id: impl Into<String>,
        ring_name: impl Into<String>,
        local_identity: TlsIdentity,
        secret: RingSecret,
    ) -> Arc<Self> {
        let ring_id_str = ring_id.into();
        let ring_name_str = ring_name.into();
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;

        let mut roster = DeviceRingRosterDto::new(&ring_id_str, &ring_name_str);
        roster.add_device(RingDeviceDto::new(
            local_identity.fingerprint.clone(),
            "This Device",
            Some(local_identity.cert_pem.clone()),
            now_ms,
        ));

        let (event_tx, _) = broadcast::channel(128);

        Arc::new(Self {
            ring_id: ring_id_str,
            ring_name: ring_name_str,
            local_identity,
            inner: RwLock::new(DeviceRingInner {
                roster,
                secret,
                current_seq_no: 0,
                last_seen_seq: HashMap::new(),
                current_content_hash: None,
                current_payload: None,
                recent_hashes: HashSet::new(),
                recent_hashes_order: VecDeque::new(),
            }),
            event_tx,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_default(),
            mesh_router: RwLock::new(None),
        })
    }

    /// Creates or pairs into a `DeviceRing` using a mutual PIN code.
    pub fn pair_with_pin(
        ring_id: impl Into<String>,
        ring_name: impl Into<String>,
        local_identity: TlsIdentity,
        pin: &str,
    ) -> Arc<Self> {
        let ring_id_str = ring_id.into();
        let secret = RingSecret::derive_from_pin(&ring_id_str, pin);
        Self::new(ring_id_str, ring_name, local_identity, secret)
    }

    /// Returns the unique identifier of this personal ring.
    pub fn ring_id(&self) -> &str {
        &self.ring_id
    }

    /// Returns the user-friendly name of this ring.
    pub fn ring_name(&self) -> &str {
        &self.ring_name
    }

    /// Returns the local device's cryptographic identity.
    pub fn local_identity(&self) -> &TlsIdentity {
        &self.local_identity
    }

    /// Returns a copy of the current device roster.
    pub fn roster(&self) -> DeviceRingRosterDto {
        self.inner.read().expect("device ring lock poisoned").roster.clone()
    }

    /// Checks if a device fingerprint is a member of this ring.
    pub fn is_member(&self, fingerprint: &str) -> bool {
        self.inner.read().expect("device ring lock poisoned").roster.is_member(fingerprint)
    }

    /// Adds a device to this ring's roster.
    pub fn add_device(&self, device: RingDeviceDto) {
        self.inner.write().expect("device ring lock poisoned").roster.add_device(device);
    }

    /// Removes a device from this ring's roster.
    pub fn remove_device(&self, fingerprint: &str) -> bool {
        self.inner.write().expect("device ring lock poisoned").roster.remove_device(fingerprint)
    }

    /// Returns the currently active clipboard payload, if any.
    pub fn current_payload(&self) -> Option<ClipboardPayloadDto> {
        self.inner.read().expect("device ring lock poisoned").current_payload.clone()
    }

    /// Returns the SHA-256 content hash of the currently active clipboard.
    pub fn current_content_hash(&self) -> Option<String> {
        self.inner.read().expect("device ring lock poisoned").current_content_hash.clone()
    }

    /// Subscribes to local clipboard sync events.
    pub fn subscribe(&self) -> broadcast::Receiver<ClipboardEventDto> {
        self.event_tx.subscribe()
    }

    /// Attaches a `MeshRouter` for multi-hop mesh clipboard broadcasts.
    pub fn attach_mesh_router(&self, router: Arc<crate::mesh::router::MeshRouter>) {
        *self.mesh_router.write().expect("mesh router lock poisoned") = Some(router);
    }

    /// Creates an E2EE distribution envelope packaging the shared `RingSecret` for a new member.
    pub fn create_key_distribution_for(
        &self,
        recipient_fp: &str,
        recipient_cert_pem: &str,
    ) -> Result<RingKeyDistributionDto, ClipboardError> {
        let inner = self.inner.read().expect("device ring lock poisoned");
        let envelope = seal_envelope(
            &self.local_identity,
            recipient_fp,
            recipient_cert_pem,
            inner.secret.key_bytes(),
        )?;
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;

        Ok(RingKeyDistributionDto {
            ring_id: self.ring_id.clone(),
            epoch: inner.roster.epoch,
            recipient_fp: recipient_fp.to_ascii_uppercase(),
            sender_fp: self.local_identity.fingerprint.to_ascii_uppercase(),
            envelope,
            created_at_ms: now_ms,
        })
    }

    /// Unseals a `RingKeyDistributionDto` using the local device's TLS identity to extract the `RingSecret`.
    pub fn apply_key_distribution(
        local_identity: &TlsIdentity,
        dist: &RingKeyDistributionDto,
    ) -> Result<RingSecret, ClipboardError> {
        let plaintext_bytes = open_envelope(local_identity, None, &dist.envelope)?;
        if plaintext_bytes.len() != 32 {
            return Err(ClipboardError::KeyError(
                "Decrypted ring secret is not 32 bytes".into(),
            ));
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&plaintext_bytes);
        Ok(RingSecret::from_raw(key_bytes))
    }

    /// Creates a new local clipboard event, suppressing identical content.
    ///
    /// Returns `Ok(None)` if the content is identical to the current active clipboard.
    pub fn create_event(
        &self,
        payload: ClipboardPayloadDto,
    ) -> Result<Option<ClipboardEventDto>, ClipboardError> {
        let content_hash = payload.compute_content_hash();
        let mut inner = self.inner.write().expect("device ring lock poisoned");

        // 1. Check if hash matches current active clipboard (local re-broadcast suppression)
        if inner.current_content_hash.as_deref() == Some(content_hash.as_str()) {
            return Ok(None);
        }

        // 2. Advance monotonic sequence number
        inner.current_seq_no = inner.current_seq_no.saturating_add(1);
        let seq_no = inner.current_seq_no;
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let event_id = format!("evt-{}", Uuid::new_v4());

        let mut event = ClipboardEventDto::new(
            event_id,
            &self.ring_id,
            &self.local_identity.fingerprint,
            seq_no,
            now_ms,
            payload.clone(),
        );

        // 3. Digitally sign the canonical event bytes
        let sig_bytes = sign_with_identity(&self.local_identity.key_pem, &event.canonical_signing_bytes())?;
        event.signature = Some(STANDARD.encode(&sig_bytes));

        // 4. Update internal state and recent hashes
        inner.current_content_hash = Some(content_hash.clone());
        inner.current_payload = Some(payload);
        inner.recent_hashes.insert(content_hash.clone());
        inner.recent_hashes_order.push_back(content_hash);
        while inner.recent_hashes_order.len() > MAX_RECENT_HASHES {
            if let Some(old) = inner.recent_hashes_order.pop_front() {
                inner.recent_hashes.remove(&old);
            }
        }

        // 5. Emit event to local subscribers
        let _ = self.event_tx.send(event.clone());

        Ok(Some(event))
    }

    /// Encrypts a `ClipboardEventDto` into an `EncryptedClipboardDto` using the shared ring secret.
    pub fn encrypt_event(
        &self,
        event: &ClipboardEventDto,
    ) -> Result<EncryptedClipboardDto, ClipboardError> {
        let inner = self.inner.read().expect("device ring lock poisoned");
        let payload_json = serde_json::to_vec(&event.payload)
            .map_err(|e| ClipboardError::Serialization(e.to_string()))?;

        let (nonce, ciphertext) = inner.secret.encrypt(
            &event.ring_id,
            &event.origin_device_fp,
            event.seq_no,
            &payload_json,
        )?;

        let mut encrypted = EncryptedClipboardDto {
            ring_id: event.ring_id.clone(),
            origin_device_fp: event.origin_device_fp.to_ascii_uppercase(),
            seq_no: event.seq_no,
            timestamp_ms: event.timestamp_ms,
            content_hash: event.content_hash.clone(),
            nonce,
            ciphertext,
            signature: None,
        };

        // Sign encrypted wire DTO
        let sig_bytes = sign_with_identity(
            &self.local_identity.key_pem,
            &encrypted.canonical_signing_bytes(),
        )?;
        encrypted.signature = Some(STANDARD.encode(&sig_bytes));

        Ok(encrypted)
    }

    /// Handles an incoming `EncryptedClipboardDto` with full authentication and echo-loop suppression.
    pub fn handle_encrypted_clipboard(
        &self,
        encrypted: &EncryptedClipboardDto,
        caller_cert_fp: Option<&str>,
    ) -> Result<HandleClipboardResult, ClipboardError> {
        let mut inner = self.inner.write().expect("device ring lock poisoned");

        // 1. Verify Ring ID
        if encrypted.ring_id != self.ring_id {
            return Err(ClipboardError::MismatchedRing {
                expected: self.ring_id.clone(),
                got: encrypted.ring_id.clone(),
            });
        }

        // 2. Verify Origin Device is in the roster
        if !inner.roster.is_member(&encrypted.origin_device_fp) {
            return Err(ClipboardError::UnauthorizedPeer(
                encrypted.origin_device_fp.clone(),
            ));
        }

        // 3. If caller cert fingerprint is provided, verify caller is authorized
        if caller_cert_fp.is_some_and(|caller_fp| !inner.roster.is_member(caller_fp)) {
            return Err(ClipboardError::UnauthorizedPeer(
                caller_cert_fp.unwrap_or_default().to_string(),
            ));
        }

        // 4. Echo Suppression 1: Origin device is self
        if encrypted
            .origin_device_fp
            .eq_ignore_ascii_case(&self.local_identity.fingerprint)
        {
            return Ok(HandleClipboardResult::SuppressedEchoSelf);
        }

        // 5. Echo Suppression 2: Content hash matches current or recent hashes
        if inner.current_content_hash.as_deref() == Some(encrypted.content_hash.as_str())
            || inner.recent_hashes.contains(&encrypted.content_hash)
        {
            return Ok(HandleClipboardResult::SuppressedEchoHash);
        }

        // 6. Echo Suppression 3: Monotonic sequence number per origin device
        let origin_fp_upper = encrypted.origin_device_fp.to_ascii_uppercase();
        if let Some(&last_seq) = inner.last_seen_seq.get(&origin_fp_upper) {
            #[allow(clippy::collapsible_if)]
            if encrypted.seq_no <= last_seq {
                return Ok(HandleClipboardResult::SuppressedEchoSeq {
                    last_seen: last_seq,
                    received: encrypted.seq_no,
                });
            }
        }

        // 7. Decrypt ciphertext using shared ring secret
        let decrypted_bytes = inner.secret.decrypt(
            &encrypted.ring_id,
            &encrypted.origin_device_fp,
            encrypted.seq_no,
            &encrypted.nonce,
            &encrypted.ciphertext,
        )?;

        // 8. Deserialize payload and verify content hash
        let payload: ClipboardPayloadDto = serde_json::from_slice(&decrypted_bytes)
            .map_err(|e| ClipboardError::Serialization(e.to_string()))?;
        let computed_hash = payload.compute_content_hash();
        if computed_hash != encrypted.content_hash {
            return Err(ClipboardError::ContentHashMismatch {
                expected: encrypted.content_hash.clone(),
                computed: computed_hash,
            });
        }

        // 9. Optional signature verification if origin device has certificate
        #[allow(clippy::collapsible_if)]
        if let (Some(sig_b64), Some(device)) = (
            &encrypted.signature,
            inner.roster.get_device(&encrypted.origin_device_fp),
        ) {
            if let (Some(cert_pem), Ok(sig_bytes)) = (&device.cert_pem, STANDARD.decode(sig_b64)) {
                let _ = verify_signature_with_cert(
                    cert_pem,
                    &encrypted.canonical_signing_bytes(),
                    &sig_bytes,
                );
            }
        }

        // 10. Update state: sequence number, current content, and recent hash ring buffer
        inner.last_seen_seq.insert(origin_fp_upper, encrypted.seq_no);
        inner.current_content_hash = Some(encrypted.content_hash.clone());
        inner.current_payload = Some(payload.clone());
        inner.recent_hashes.insert(encrypted.content_hash.clone());
        inner.recent_hashes_order.push_back(encrypted.content_hash.clone());
        while inner.recent_hashes_order.len() > MAX_RECENT_HASHES {
            if let Some(old) = inner.recent_hashes_order.pop_front() {
                inner.recent_hashes.remove(&old);
            }
        }

        let event = ClipboardEventDto::new(
            format!("evt-{}", Uuid::new_v4()),
            &self.ring_id,
            &encrypted.origin_device_fp,
            encrypted.seq_no,
            encrypted.timestamp_ms,
            payload,
        );

        // 11. Emit to local subscribers
        let _ = self.event_tx.send(event.clone());

        Ok(HandleClipboardResult::Applied(event))
    }

    /// Hyper request dispatcher mounted at `POST /api/lanflare/v1/toolbox/clipboard`.
    pub async fn handle_hyper_request(
        self: &Arc<Self>,
        mut req: Request<Incoming>,
        client: Option<ClientConnectionInfo>,
    ) -> Response<ServerResponseBody> {
        let method = req.method().clone();
        let path = req.uri().path().to_string();

        if method != Method::POST || path != "/api/lanflare/v1/toolbox/clipboard" {
            return empty_box_response(StatusCode::NOT_FOUND);
        }

        let body_bytes = match req.body_mut().collect().await {
            Ok(collected) => {
                let bytes = collected.to_bytes();
                if bytes.len() > MAX_CONTROL_BODY_SIZE {
                    return empty_box_response(StatusCode::PAYLOAD_TOO_LARGE);
                }
                bytes
            }
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let encrypted_dto: EncryptedClipboardDto = match serde_json::from_slice(&body_bytes) {
            Ok(dto) => dto,
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let caller_fp = client.as_ref().and_then(|c| c.cert_fingerprint.as_deref());

        match self.handle_encrypted_clipboard(&encrypted_dto, caller_fp) {
            Ok(HandleClipboardResult::Applied(event)) => {
                json_response(
                    StatusCode::OK,
                    &ClipboardResponseDto {
                        status: "applied".to_string(),
                        reason: None,
                        content_hash: event.content_hash,
                    },
                )
            }
            Ok(HandleClipboardResult::SuppressedEchoSelf) => {
                json_response(
                    StatusCode::OK,
                    &ClipboardResponseDto {
                        status: "suppressed".to_string(),
                        reason: Some("echo_self".to_string()),
                        content_hash: encrypted_dto.content_hash,
                    },
                )
            }
            Ok(HandleClipboardResult::SuppressedEchoHash) => {
                json_response(
                    StatusCode::OK,
                    &ClipboardResponseDto {
                        status: "suppressed".to_string(),
                        reason: Some("echo_hash".to_string()),
                        content_hash: encrypted_dto.content_hash,
                    },
                )
            }
            Ok(HandleClipboardResult::SuppressedEchoSeq { .. }) => {
                json_response(
                    StatusCode::OK,
                    &ClipboardResponseDto {
                        status: "suppressed".to_string(),
                        reason: Some("echo_seq".to_string()),
                        content_hash: encrypted_dto.content_hash,
                    },
                )
            }
            Err(ClipboardError::UnauthorizedPeer(fp)) => {
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header(hyper::header::CONTENT_TYPE, "text/plain")
                    .body(full_box_body(format!("Unauthorized peer: {fp}")))
                    .unwrap()
            }
            Err(ClipboardError::MismatchedRing { .. }) => {
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header(hyper::header::CONTENT_TYPE, "text/plain")
                    .body(full_box_body("Ring ID mismatch"))
                    .unwrap()
            }
            Err(ClipboardError::DecryptionFailed(err)) => {
                Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .header(hyper::header::CONTENT_TYPE, "text/plain")
                    .body(full_box_body(format!("Decryption failed: {err}")))
                    .unwrap()
            }
            Err(e) => {
                Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header(hyper::header::CONTENT_TYPE, "text/plain")
                    .body(full_box_body(e.to_string()))
                    .unwrap()
            }
        }
    }

    /// Broadcasts an encrypted clipboard event to a remote peer endpoint over HTTP.
    pub async fn broadcast_to_peer_http(
        &self,
        encrypted: &EncryptedClipboardDto,
        peer_addr: &str,
    ) -> Result<ClipboardResponseDto, ClipboardError> {
        let url = format!(
            "http://{}/api/lanflare/v1/toolbox/clipboard",
            peer_addr.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(&url)
            .json(encrypted)
            .send()
            .await
            .map_err(|e| ClipboardError::Network(format!("Failed to send clipboard to {peer_addr}: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClipboardError::Network(format!(
                "Peer {peer_addr} rejected clipboard sync (HTTP {status}): {body}"
            )));
        }

        resp.json::<ClipboardResponseDto>()
            .await
            .map_err(|e| ClipboardError::Serialization(e.to_string()))
    }

    /// Broadcasts a newly authored clipboard payload to all known ring peers across 1-hop LAN.
    pub async fn broadcast_to_ring(
        &self,
        payload: ClipboardPayloadDto,
        peers: &[DiscoveredPeer],
    ) -> Result<Option<ClipboardEventDto>, ClipboardError> {
        let maybe_event = self.create_event(payload)?;
        let event = match maybe_event {
            Some(e) => e,
            None => return Ok(None),
        };

        let encrypted = self.encrypt_event(&event)?;
        let roster = self.roster();

        for peer in peers {
            if peer.fingerprint.eq_ignore_ascii_case(&self.local_identity.fingerprint) {
                continue;
            }
            if !roster.is_member(&peer.fingerprint) {
                continue;
            }

            if let Some(endpoint) = peer.endpoints.first() {
                let peer_addr = format!("{}:{}", endpoint.ip, peer.port);
                let _ = self.broadcast_to_peer_http(&encrypted, &peer_addr).await;
            }
        }

        Ok(Some(event))
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::crypto::cert::generate_self_signed;

    #[tokio::test]
    pub async fn test_device_ring_e2ee_clipboard_sync_and_echo_suppression() {
        let ident_a = generate_self_signed().unwrap();
        let ident_b = generate_self_signed().unwrap();

        let secret = RingSecret::generate().unwrap();

        // 1. Initialize Device A ring
        let ring_a = DeviceRing::new("ring-123", "Personal Ring", ident_a.clone(), secret.clone());

        // 2. Initialize Device B ring via key distribution from A
        let key_dist = ring_a
            .create_key_distribution_for(&ident_b.fingerprint, &ident_b.cert_pem)
            .unwrap();
        let secret_b = DeviceRing::apply_key_distribution(&ident_b, &key_dist).unwrap();
        assert_eq!(secret.key_bytes(), secret_b.key_bytes());

        let ring_b = DeviceRing::new("ring-123", "Personal Ring", ident_b.clone(), secret_b);

        // Add each other to rosters
        ring_a.add_device(RingDeviceDto::new(
            &ident_b.fingerprint,
            "Device B",
            Some(ident_b.cert_pem.clone()),
            1000,
        ));
        ring_b.add_device(RingDeviceDto::new(
            &ident_a.fingerprint,
            "Device A",
            Some(ident_a.cert_pem.clone()),
            1000,
        ));

        // 3. Device A copies text payload
        let payload = ClipboardPayloadDto::text("Rust + LanFlare Clipboard Sync");
        let event_opt = ring_a.create_event(payload.clone()).unwrap();
        assert!(event_opt.is_some());
        let event = event_opt.unwrap();
        assert_eq!(event.seq_no, 1);

        // Device A encrypts for wire
        let encrypted = ring_a.encrypt_event(&event).unwrap();

        // 4. Device B receives and decrypts
        let result = ring_b
            .handle_encrypted_clipboard(&encrypted, Some(&ident_a.fingerprint))
            .unwrap();
        match result {
            HandleClipboardResult::Applied(applied_event) => {
                assert_eq!(applied_event.content_hash, event.content_hash);
                assert_eq!(
                    applied_event.payload.as_text(),
                    Some("Rust + LanFlare Clipboard Sync")
                );
            }
            other => panic!("Expected Applied, got {:?}", other),
        }
        assert_eq!(ring_b.current_payload(), Some(payload.clone()));

        // 5. Test Echo Suppression:
        // Case A: Loopback to origin device (Device A receives its own broadcast)
        let echo_self = ring_a
            .handle_encrypted_clipboard(&encrypted, Some(&ident_a.fingerprint))
            .unwrap();
        assert_eq!(echo_self, HandleClipboardResult::SuppressedEchoSelf);

        // Case B: Content Hash echo (Device B receives duplicate or bounced content)
        let echo_hash = ring_b
            .handle_encrypted_clipboard(&encrypted, Some(&ident_a.fingerprint))
            .unwrap();
        assert_eq!(echo_hash, HandleClipboardResult::SuppressedEchoHash);

        // Case C: Sequence number echo / stale replay
        // Construct an event with old sequence number (seq_no = 1, while last seen is 1)
        let mut stale_encrypted = encrypted.clone();
        stale_encrypted.content_hash = "DIFFERENT_HASH_00000000000000000000000000000000000000000000000000".to_string();
        let echo_seq = ring_b
            .handle_encrypted_clipboard(&stale_encrypted, Some(&ident_a.fingerprint))
            .unwrap();
        assert!(matches!(
            echo_seq,
            HandleClipboardResult::SuppressedEchoSeq {
                last_seen: 1,
                received: 1
            }
        ));

        // Case D: Local re-broadcast suppression (Device B tries to create event with content it just received)
        let rebroadcast = ring_b.create_event(payload).unwrap();
        assert!(rebroadcast.is_none(), "Local re-broadcast of identical content must be suppressed");

        // 6. Test Image and Files payloads
        let img_payload = ClipboardPayloadDto::image("image/png", "QUJDREVGRw==");
        let img_event = ring_b.create_event(img_payload.clone()).unwrap().unwrap();
        let img_encrypted = ring_b.encrypt_event(&img_event).unwrap();
        let img_result = ring_a
            .handle_encrypted_clipboard(&img_encrypted, Some(&ident_b.fingerprint))
            .unwrap();
        match img_result {
            HandleClipboardResult::Applied(e) => {
                assert_eq!(e.payload, img_payload);
            }
            other => panic!("Expected Applied, got {:?}", other),
        }

        let files_payload = ClipboardPayloadDto::files(vec!["/docs/plan.md".into(), "/docs/spec.pdf".into()]);
        let files_event = ring_a.create_event(files_payload.clone()).unwrap().unwrap();
        let files_encrypted = ring_a.encrypt_event(&files_event).unwrap();
        let files_result = ring_b
            .handle_encrypted_clipboard(&files_encrypted, Some(&ident_a.fingerprint))
            .unwrap();
        match files_result {
            HandleClipboardResult::Applied(e) => {
                assert_eq!(e.payload, files_payload);
            }
            other => panic!("Expected Applied, got {:?}", other),
        }
    }

    #[tokio::test]
    pub async fn test_unauthorized_peer_clipboard_rejection() {
        let ident_a = generate_self_signed().unwrap();
        let ident_b = generate_self_signed().unwrap();
        let rogue_ident = generate_self_signed().unwrap();

        let secret = RingSecret::generate().unwrap();
        let ring_a = DeviceRing::new("ring-456", "Secure Ring", ident_a.clone(), secret.clone());
        ring_a.add_device(RingDeviceDto::new(
            &ident_b.fingerprint,
            "Device B",
            Some(ident_b.cert_pem.clone()),
            1000,
        ));

        // 1. Rogue device tries to send clipboard claiming rogue origin
        let rogue_payload = ClipboardPayloadDto::text("Malicious injection");
        let rogue_secret = RingSecret::generate().unwrap();
        let rogue_ring = DeviceRing::new("ring-456", "Secure Ring", rogue_ident.clone(), rogue_secret);
        let rogue_event = rogue_ring.create_event(rogue_payload).unwrap().unwrap();
        let rogue_encrypted = rogue_ring.encrypt_event(&rogue_event).unwrap();

        let rej1 = ring_a.handle_encrypted_clipboard(&rogue_encrypted, Some(&rogue_ident.fingerprint));
        assert!(matches!(rej1, Err(ClipboardError::UnauthorizedPeer(_))));

        // 2. Rogue device tries to forge origin as Device B, but encrypts with wrong secret
        let mut forged_encrypted = rogue_encrypted.clone();
        forged_encrypted.origin_device_fp = ident_b.fingerprint.clone();
        let rej2 = ring_a.handle_encrypted_clipboard(&forged_encrypted, Some(&rogue_ident.fingerprint));
        assert!(matches!(rej2, Err(ClipboardError::UnauthorizedPeer(_))));

        // Even without caller cert check, decryption fails because symmetric secret is wrong
        let rej3 = ring_a.handle_encrypted_clipboard(&forged_encrypted, None);
        assert!(matches!(rej3, Err(ClipboardError::DecryptionFailed(_))));

        // 3. Mismatched ring ID
        let mut mismatched = rogue_encrypted.clone();
        mismatched.ring_id = "wrong-ring".to_string();
        let rej4 = ring_a.handle_encrypted_clipboard(&mismatched, None);
        assert!(matches!(rej4, Err(ClipboardError::MismatchedRing { .. })));

        // Verify receiver state is unchanged
        assert_eq!(ring_a.current_payload(), None);
    }

    #[tokio::test]
    pub async fn test_hyper_clipboard_endpoint_e2e_sync_and_rejection() {
        use crate::config::AppConfig;
        use crate::discovery::registry::PeerRegistry;
        use crate::server::start_server_toolbox;

        let ident_a = generate_self_signed().unwrap();
        let ident_b = generate_self_signed().unwrap();
        let rogue_ident = generate_self_signed().unwrap();

        let secret = RingSecret::generate().unwrap();
        let ring_a = DeviceRing::new("ring-hyper", "Hyper Ring", ident_a.clone(), secret.clone());
        let ring_b = DeviceRing::new("ring-hyper", "Hyper Ring", ident_b.clone(), secret.clone());

        ring_a.add_device(RingDeviceDto::new(
            &ident_b.fingerprint,
            "Device B",
            Some(ident_b.cert_pem.clone()),
            1000,
        ));
        ring_b.add_device(RingDeviceDto::new(
            &ident_a.fingerprint,
            "Device A",
            Some(ident_a.cert_pem.clone()),
            1000,
        ));

        // Start server with ring_a mounted
        let cfg = AppConfig {
            port: 0,
            ..Default::default()
        };
        let registry = PeerRegistry::new();
        let handle = start_server_toolbox(
            cfg,
            None,
            registry,
            None,
            None,
            None,
            None,
            None,
            Some(ring_a.clone()),
            None,
            None,
        )
        .await
        .unwrap();

        let port = handle.bound_port();
        let peer_addr = format!("127.0.0.1:{port}");

        // 1. Device B sends clipboard update to Device A over HTTP
        let payload = ClipboardPayloadDto::text("Sync over Hyper Server");
        let event = ring_b.create_event(payload.clone()).unwrap().unwrap();
        let encrypted = ring_b.encrypt_event(&event).unwrap();

        let resp1 = ring_b.broadcast_to_peer_http(&encrypted, &peer_addr).await.unwrap();
        assert_eq!(resp1.status, "applied");
        assert_eq!(resp1.content_hash, event.content_hash);
        assert_eq!(ring_a.current_payload(), Some(payload));

        // 2. Resend same payload -> echo suppression over HTTP
        let resp2 = ring_b.broadcast_to_peer_http(&encrypted, &peer_addr).await.unwrap();
        assert_eq!(resp2.status, "suppressed");
        assert_eq!(resp2.reason.as_deref(), Some("echo_hash"));

        // 3. Rogue peer sends update -> HTTP 403 Forbidden
        let rogue_ring = DeviceRing::new("ring-hyper", "Hyper Ring", rogue_ident.clone(), secret.clone());
        let rogue_payload = ClipboardPayloadDto::text("Rogue Injection");
        let rogue_event = rogue_ring.create_event(rogue_payload).unwrap().unwrap();
        let rogue_encrypted = rogue_ring.encrypt_event(&rogue_event).unwrap();

        let err = ring_b.broadcast_to_peer_http(&rogue_encrypted, &peer_addr).await;
        assert!(err.is_err());
        let err_str = err.unwrap_err().to_string();
        assert!(err_str.contains("403") || err_str.contains("Unauthorized"), "Expected HTTP 403: {err_str}");

        handle.stop();
    }
}
