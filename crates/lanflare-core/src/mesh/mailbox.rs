//! Custodial mesh relay mailbox for asynchronous store-and-forward offline delivery.
//!
//! Sub-phase 9.3 implementation:
//! - Implements `RelayMailbox` with deposit and fetch operations:
//!   - `POST /api/lanflare/v1/mesh/mailbox/deposit`
//!   - `POST /api/lanflare/v1/mesh/mailbox/fetch`
//! - Bounded per-recipient and total memory ring-buffer quotas (`max_bytes_per_peer`, `max_total_bytes`, `default_ttl_secs`)
//! - Cryptographic recipient authentication for fetch operations (digital signature over canonical timestamped request)
//! - Automatic TTL expiration, item acknowledgement, and quota reclamation.

use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use lanflare_proto::mesh::{
    E2eeEnvelope, MailboxDepositRequest, MailboxDepositResponse, MailboxFetchRequest,
    MailboxFetchResponse, MailboxItemDto,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::crypto::cert::{fingerprint_from_cert_pem, TlsIdentity};
use crate::crypto::e2ee::{sign_with_identity, verify_signature_with_cert};
use crate::server::dufs::{empty_box_response, json_response, ServerResponseBody};

/// Default maximum bytes stored per recipient peer (5 MB).
pub const DEFAULT_MAX_MAILBOX_BYTES_PER_PEER: usize = 5 * 1024 * 1024;

/// Default maximum total bytes stored in the relay mailbox across all recipients (50 MB).
pub const DEFAULT_MAX_TOTAL_MAILBOX_BYTES: usize = 50 * 1024 * 1024;

/// Default maximum number of messages stored per recipient peer (500).
pub const DEFAULT_MAX_ITEMS_PER_PEER: usize = 500;

/// Default TTL for stored envelopes (86,400 seconds = 24 hours).
pub const DEFAULT_MAILBOX_TTL_SECS: u64 = 86_400;

/// Maximum allowed TTL for stored envelopes (7 days).
pub const MAX_MAILBOX_TTL_SECS: u64 = 7 * 86_400;

/// Maximum allowed JSON control body size (10 MB).
pub const MAX_MAILBOX_BODY_SIZE: usize = 10 * 1024 * 1024;

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Errors returned by `RelayMailbox` operations.
#[derive(Debug, Error)]
pub enum MailboxError {
    #[error(
        "Per-peer mailbox quota exceeded (recipient: {recipient_fp}, requested: {requested} bytes, current: {current} bytes, max: {max} bytes)"
    )]
    PeerQuotaExceeded {
        recipient_fp: String,
        requested: usize,
        current: usize,
        max: usize,
    },

    #[error(
        "Total mailbox capacity exceeded (requested: {requested} bytes, current: {current} bytes, max: {max} bytes)"
    )]
    TotalQuotaExceeded {
        requested: usize,
        current: usize,
        max: usize,
    },

    #[error("Authentication failed: {0}")]
    Unauthorized(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Internal mailbox error: {0}")]
    Internal(String),
}

/// Configuration settings for `RelayMailbox`.
#[derive(Clone, Debug)]
pub struct MailboxConfig {
    /// Maximum bytes allowed per recipient peer.
    pub max_bytes_per_peer: usize,

    /// Maximum total bytes across all recipients.
    pub max_total_bytes: usize,

    /// Maximum number of items stored per recipient peer.
    pub max_items_per_peer: usize,

    /// Default time-to-live for stored items in seconds.
    pub default_ttl_secs: u64,

    /// Maximum allowed time-to-live in seconds.
    pub max_ttl_secs: u64,
}

impl Default for MailboxConfig {
    fn default() -> Self {
        Self {
            max_bytes_per_peer: DEFAULT_MAX_MAILBOX_BYTES_PER_PEER,
            max_total_bytes: DEFAULT_MAX_TOTAL_MAILBOX_BYTES,
            max_items_per_peer: DEFAULT_MAX_ITEMS_PER_PEER,
            default_ttl_secs: DEFAULT_MAILBOX_TTL_SECS,
            max_ttl_secs: MAX_MAILBOX_TTL_SECS,
        }
    }
}

/// Operational statistics for the relay mailbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxStats {
    /// Total number of stored envelopes.
    pub total_items: usize,

    /// Total bytes currently stored.
    pub total_bytes: usize,

    /// Number of distinct recipients with active envelopes.
    pub peer_count: usize,
}

#[derive(Clone, Debug)]
struct StoredMailboxEntry {
    deposit_id: String,
    envelope: E2eeEnvelope,
    size_bytes: usize,
    deposited_at_ms: u64,
    expires_at_ms: u64,
}

#[derive(Debug, Default)]
struct MailboxState {
    /// Map of canonical uppercase recipient fingerprint to stored entries (FIFO).
    entries: HashMap<String, Vec<StoredMailboxEntry>>,

    /// Map of canonical uppercase recipient fingerprint to total stored bytes.
    bytes_per_peer: HashMap<String, usize>,

    /// Total stored bytes across all peers.
    total_bytes: usize,
}

impl MailboxState {
    fn evict_expired(&mut self, now_ms: u64) -> usize {
        let mut evicted = 0;
        let mut empty_peers = Vec::new();

        for (fp, peer_entries) in self.entries.iter_mut() {
            let initial_len = peer_entries.len();
            let mut retained_bytes = 0;
            peer_entries.retain(|entry| {
                if entry.expires_at_ms > now_ms {
                    retained_bytes += entry.size_bytes;
                    true
                } else {
                    evicted += 1;
                    false
                }
            });

            if peer_entries.len() < initial_len {
                let prev_bytes = self.bytes_per_peer.get(fp).copied().unwrap_or(0);
                let diff = prev_bytes.saturating_sub(retained_bytes);
                self.total_bytes = self.total_bytes.saturating_sub(diff);
                self.bytes_per_peer.insert(fp.clone(), retained_bytes);
            }

            if peer_entries.is_empty() {
                empty_peers.push(fp.clone());
            }
        }

        for fp in empty_peers {
            self.entries.remove(&fp);
            self.bytes_per_peer.remove(&fp);
        }

        evicted
    }

    fn remove_items(&mut self, fp: &str, ids: &[String]) -> usize {
        let mut removed = 0;
        let Some(peer_entries) = self.entries.get_mut(fp) else {
            return 0;
        };

        let mut bytes_removed = 0;
        peer_entries.retain(|entry| {
            if ids.contains(&entry.deposit_id) {
                bytes_removed += entry.size_bytes;
                removed += 1;
                false
            } else {
                true
            }
        });

        if bytes_removed > 0 {
            if let Some(b) = self.bytes_per_peer.get_mut(fp) {
                *b = b.saturating_sub(bytes_removed);
            }
            self.total_bytes = self.total_bytes.saturating_sub(bytes_removed);
        }

        if peer_entries.is_empty() {
            self.entries.remove(fp);
            self.bytes_per_peer.remove(fp);
        }

        removed
    }
}

/// Custodial relay mailbox providing bounded asynchronous store-and-forward delivery.
pub struct RelayMailbox {
    config: MailboxConfig,
    state: RwLock<MailboxState>,
}

impl Default for RelayMailbox {
    fn default() -> Self {
        Self::new(MailboxConfig::default())
    }
}

impl RelayMailbox {
    /// Creates a new `RelayMailbox` with the specified configuration.
    pub fn new(config: MailboxConfig) -> Self {
        Self {
            config,
            state: RwLock::new(MailboxState::default()),
        }
    }

    /// Returns a reference to the active configuration.
    pub fn config(&self) -> &MailboxConfig {
        &self.config
    }

    /// Deposits an end-to-end encrypted envelope into custodial storage.
    pub async fn deposit(
        &self,
        req: MailboxDepositRequest,
    ) -> Result<MailboxDepositResponse, MailboxError> {
        let recipient_fp = req.envelope.recipient_fp.trim().to_ascii_uppercase();
        if recipient_fp.is_empty() {
            return Err(MailboxError::BadRequest(
                "Recipient fingerprint cannot be empty".into(),
            ));
        }

        let deposit_id = req
            .deposit_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now_ms = current_time_ms();

        // Calculate TTL and expiration
        let requested_ttl = req.ttl_seconds.unwrap_or(self.config.default_ttl_secs);
        let effective_ttl = requested_ttl.clamp(1, self.config.max_ttl_secs);
        let expires_at_ms = now_ms.saturating_add(effective_ttl.saturating_mul(1000));

        // Calculate envelope serialized byte size
        let size_bytes = serde_json::to_vec(&req.envelope)
            .map(|v| v.len())
            .unwrap_or(2048);

        let mut state = self.state.write().await;

        // Opportunistic eviction of expired items to reclaim space
        state.evict_expired(now_ms);

        // Check per-peer byte quota
        let current_peer_bytes = state
            .bytes_per_peer
            .get(&recipient_fp)
            .copied()
            .unwrap_or(0);
        if current_peer_bytes + size_bytes > self.config.max_bytes_per_peer {
            return Err(MailboxError::PeerQuotaExceeded {
                recipient_fp,
                requested: size_bytes,
                current: current_peer_bytes,
                max: self.config.max_bytes_per_peer,
            });
        }

        // Check per-peer item count quota
        let current_peer_count = state
            .entries
            .get(&recipient_fp)
            .map(|v| v.len())
            .unwrap_or(0);
        if current_peer_count + 1 > self.config.max_items_per_peer {
            return Err(MailboxError::PeerQuotaExceeded {
                recipient_fp,
                requested: size_bytes,
                current: current_peer_bytes,
                max: self.config.max_bytes_per_peer,
            });
        }

        // Check total mailbox capacity quota
        if state.total_bytes + size_bytes > self.config.max_total_bytes {
            return Err(MailboxError::TotalQuotaExceeded {
                requested: size_bytes,
                current: state.total_bytes,
                max: self.config.max_total_bytes,
            });
        }

        // Insert new entry
        let entry = StoredMailboxEntry {
            deposit_id: deposit_id.clone(),
            envelope: req.envelope,
            size_bytes,
            deposited_at_ms: now_ms,
            expires_at_ms,
        };

        state
            .entries
            .entry(recipient_fp.clone())
            .or_default()
            .push(entry);
        *state.bytes_per_peer.entry(recipient_fp.clone()).or_insert(0) += size_bytes;
        state.total_bytes += size_bytes;

        Ok(MailboxDepositResponse {
            deposit_id,
            recipient_fp,
            stored_bytes: size_bytes,
            expires_at_ms,
        })
    }

    /// Fetches pending envelopes for an authenticated recipient.
    pub async fn fetch(&self, req: MailboxFetchRequest) -> Result<MailboxFetchResponse, MailboxError> {
        // Authenticate the recipient
        self.verify_fetch_request(&req)?;

        let recipient_fp = req.recipient_fp.trim().to_ascii_uppercase();
        let now_ms = current_time_ms();

        let mut state = self.state.write().await;

        // Opportunistic eviction
        state.evict_expired(now_ms);

        // Process explicit item acknowledgements
        if !req.ack_deposit_ids.is_empty() {
            state.remove_items(&recipient_fp, &req.ack_deposit_ids);
        }

        let Some(peer_entries) = state.entries.get_mut(&recipient_fp) else {
            return Ok(MailboxFetchResponse {
                recipient_fp,
                items: Vec::new(),
                remaining_count: 0,
            });
        };

        let limit = req.limit.unwrap_or(usize::MAX);
        let total_available = peer_entries.len();
        let fetch_count = total_available.min(limit);

        let mut items = Vec::with_capacity(fetch_count);
        for entry in peer_entries.iter().take(fetch_count) {
            items.push(MailboxItemDto {
                deposit_id: entry.deposit_id.clone(),
                envelope: entry.envelope.clone(),
                deposited_at_ms: entry.deposited_at_ms,
                expires_at_ms: entry.expires_at_ms,
            });
        }

        let remaining_count = total_available.saturating_sub(fetch_count);

        // If auto_ack is enabled, drain the retrieved entries immediately
        if req.auto_ack && !items.is_empty() {
            let fetched_ids: Vec<String> = items.iter().map(|i| i.deposit_id.clone()).collect();
            state.remove_items(&recipient_fp, &fetched_ids);
        }

        Ok(MailboxFetchResponse {
            recipient_fp,
            items,
            remaining_count,
        })
    }

    /// Verifies the digital signature and freshness of a fetch request.
    pub fn verify_fetch_request(&self, req: &MailboxFetchRequest) -> Result<(), MailboxError> {
        let now = current_time_ms();
        // Allow up to 1 minute clock skew into the future, and 5 minutes in the past
        if req.timestamp_ms > now + 60_000 || now.saturating_sub(req.timestamp_ms) > 300_000 {
            return Err(MailboxError::Unauthorized(
                "Fetch request timestamp expired or clock skew too large".into(),
            ));
        }

        let canonical = req.canonical_signing_bytes();
        let sig_bytes = STANDARD
            .decode(&req.signature)
            .map_err(|e| MailboxError::Unauthorized(format!("Invalid base64 signature: {e}")))?;

        if let Some(cert_pem) = &req.recipient_cert_pem {
            let fp = fingerprint_from_cert_pem(cert_pem)
                .map_err(|e| MailboxError::Unauthorized(format!("Invalid certificate PEM: {e}")))?;
            if !fp.eq_ignore_ascii_case(&req.recipient_fp) {
                return Err(MailboxError::Unauthorized(
                    "Recipient certificate fingerprint mismatch".into(),
                ));
            }

            verify_signature_with_cert(cert_pem, &canonical, &sig_bytes).map_err(|e| {
                MailboxError::Unauthorized(format!("Signature verification failed: {e}"))
            })?;

            Ok(())
        } else {
            Err(MailboxError::Unauthorized(
                "Missing recipient certificate PEM for authentication".into(),
            ))
        }
    }

    /// Evicts all expired items across all recipients and returns the number of evicted items.
    pub async fn evict_expired(&self) -> usize {
        let now_ms = current_time_ms();
        let mut state = self.state.write().await;
        state.evict_expired(now_ms)
    }

    /// Returns current mailbox statistics.
    pub async fn stats(&self) -> MailboxStats {
        let state = self.state.read().await;
        let total_items = state.entries.values().map(|v| v.len()).sum();
        MailboxStats {
            total_items,
            total_bytes: state.total_bytes,
            peer_count: state.entries.len(),
        }
    }

    /// Clears all entries for a specific recipient.
    pub async fn clear_peer(&self, recipient_fp: &str) -> usize {
        let fp = recipient_fp.trim().to_ascii_uppercase();
        let mut state = self.state.write().await;
        let removed = state.entries.remove(&fp).map(|v| v.len()).unwrap_or(0);
        if let Some(b) = state.bytes_per_peer.remove(&fp) {
            state.total_bytes = state.total_bytes.saturating_sub(b);
        }
        removed
    }

    /// Clears all entries from the mailbox.
    pub async fn clear_all(&self) {
        let mut state = self.state.write().await;
        state.entries.clear();
        state.bytes_per_peer.clear();
        state.total_bytes = 0;
    }

    // --- Hyper Endpoint Handlers ---

    /// Handles incoming HTTP `POST /api/lanflare/v1/mesh/mailbox/deposit`.
    pub async fn handle_deposit_endpoint(
        &self,
        req: Request<Incoming>,
    ) -> Response<ServerResponseBody> {
        let body = match Self::read_bounded_body(req.into_body()).await {
            Ok(bytes) => bytes,
            Err(status) => return empty_box_response(status),
        };

        // Try parsing MailboxDepositRequest, fallback to direct E2eeEnvelope
        let deposit_req: MailboxDepositRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(_) => match serde_json::from_slice::<E2eeEnvelope>(&body) {
                Ok(env) => MailboxDepositRequest {
                    deposit_id: None,
                    envelope: env,
                    ttl_seconds: None,
                },
                Err(_) => {
                    return json_response(
                        StatusCode::BAD_REQUEST,
                        &serde_json::json!({ "error": "Invalid mailbox deposit payload" }),
                    )
                }
            },
        };

        match self.deposit(deposit_req).await {
            Ok(resp) => json_response(StatusCode::OK, &resp),
            Err(MailboxError::PeerQuotaExceeded {
                recipient_fp,
                requested,
                current,
                max,
            }) => json_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &serde_json::json!({
                    "error": "Per-peer mailbox quota exceeded",
                    "recipientFp": recipient_fp,
                    "requestedBytes": requested,
                    "currentBytes": current,
                    "maxBytes": max
                }),
            ),
            Err(MailboxError::TotalQuotaExceeded {
                requested,
                current,
                max,
            }) => json_response(
                StatusCode::TOO_MANY_REQUESTS,
                &serde_json::json!({
                    "error": "Total mailbox capacity exceeded",
                    "requestedBytes": requested,
                    "currentBytes": current,
                    "maxBytes": max
                }),
            ),
            Err(MailboxError::BadRequest(msg)) => json_response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "error": msg }),
            ),
            Err(MailboxError::Unauthorized(msg)) => json_response(
                StatusCode::UNAUTHORIZED,
                &serde_json::json!({ "error": msg }),
            ),
            Err(MailboxError::Internal(msg)) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &serde_json::json!({ "error": msg }),
            ),
        }
    }

    /// Handles incoming HTTP `POST /api/lanflare/v1/mesh/mailbox/fetch`.
    pub async fn handle_fetch_endpoint(
        &self,
        req: Request<Incoming>,
    ) -> Response<ServerResponseBody> {
        let body = match Self::read_bounded_body(req.into_body()).await {
            Ok(bytes) => bytes,
            Err(status) => return empty_box_response(status),
        };

        let fetch_req: MailboxFetchRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(_) => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    &serde_json::json!({ "error": "Invalid mailbox fetch payload" }),
                )
            }
        };

        match self.fetch(fetch_req).await {
            Ok(resp) => json_response(StatusCode::OK, &resp),
            Err(MailboxError::Unauthorized(msg)) => json_response(
                StatusCode::UNAUTHORIZED,
                &serde_json::json!({ "error": msg }),
            ),
            Err(MailboxError::BadRequest(msg)) => json_response(
                StatusCode::BAD_REQUEST,
                &serde_json::json!({ "error": msg }),
            ),
            Err(err) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &serde_json::json!({ "error": err.to_string() }),
            ),
        }
    }

    async fn read_bounded_body(body: Incoming) -> Result<Bytes, StatusCode> {
        let limited = http_body_util::Limited::new(body, MAX_MAILBOX_BODY_SIZE);
        match limited.collect().await {
            Ok(collected) => Ok(collected.to_bytes()),
            Err(_) => Err(StatusCode::PAYLOAD_TOO_LARGE),
        }
    }

    // --- HTTP Client Helpers ---

    /// Deposits an envelope to a remote relay mailbox via HTTP.
    pub async fn deposit_to_relay_http(
        client: &reqwest::Client,
        relay_base_url: &str,
        req: &MailboxDepositRequest,
    ) -> Result<MailboxDepositResponse, MailboxError> {
        let url = format!(
            "{}/api/lanflare/v1/mesh/mailbox/deposit",
            relay_base_url.trim_end_matches('/')
        );
        let res = client
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| MailboxError::Internal(format!("Network deposit request failed: {e}")))?;

        let status = res.status();
        if status.is_success() {
            res.json::<MailboxDepositResponse>()
                .await
                .map_err(|e| MailboxError::Internal(format!("Failed to parse deposit response: {e}")))
        } else if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            Err(MailboxError::PeerQuotaExceeded {
                recipient_fp: req.envelope.recipient_fp.clone(),
                requested: 0,
                current: 0,
                max: 0,
            })
        } else if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            Err(MailboxError::TotalQuotaExceeded {
                requested: 0,
                current: 0,
                max: 0,
            })
        } else if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            Err(MailboxError::Unauthorized(format!("Server returned HTTP {status}")))
        } else {
            let error_text = res.text().await.unwrap_or_default();
            Err(MailboxError::Internal(format!(
                "Relay deposit returned {status}: {error_text}"
            )))
        }
    }

    /// Fetches pending envelopes from a remote relay mailbox via HTTP.
    pub async fn fetch_from_relay_http(
        client: &reqwest::Client,
        relay_base_url: &str,
        req: &MailboxFetchRequest,
    ) -> Result<MailboxFetchResponse, MailboxError> {
        let url = format!(
            "{}/api/lanflare/v1/mesh/mailbox/fetch",
            relay_base_url.trim_end_matches('/')
        );
        let res = client
            .post(&url)
            .json(req)
            .send()
            .await
            .map_err(|e| MailboxError::Internal(format!("Network fetch request failed: {e}")))?;

        let status = res.status();
        if status.is_success() {
            res.json::<MailboxFetchResponse>()
                .await
                .map_err(|e| MailboxError::Internal(format!("Failed to parse fetch response: {e}")))
        } else if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            Err(MailboxError::Unauthorized(format!("Server returned HTTP {status}")))
        } else {
            let error_text = res.text().await.unwrap_or_default();
            Err(MailboxError::Internal(format!(
                "Relay fetch returned {status}: {error_text}"
            )))
        }
    }
}

/// Helper to generate a signed `MailboxFetchRequest` using a local `TlsIdentity`.
pub fn create_signed_fetch_request(
    identity: &TlsIdentity,
    ack_deposit_ids: Vec<String>,
    limit: Option<usize>,
    auto_ack: bool,
) -> Result<MailboxFetchRequest, MailboxError> {
    let timestamp_ms = current_time_ms();
    let canonical = format!(
        "{}:{}",
        identity.fingerprint.to_ascii_uppercase(),
        timestamp_ms
    );
    let sig_bytes = sign_with_identity(&identity.key_pem, canonical.as_bytes())
        .map_err(|e| MailboxError::Unauthorized(format!("Signing failed: {e}")))?;
    let signature = STANDARD.encode(&sig_bytes);

    Ok(MailboxFetchRequest {
        recipient_fp: identity.fingerprint.clone(),
        timestamp_ms,
        signature,
        recipient_cert_pem: Some(identity.cert_pem.clone()),
        ack_deposit_ids,
        limit,
        auto_ack,
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::server::conn::auto::Builder;

    use crate::crypto::cert::generate_self_signed;
    use crate::crypto::e2ee::{open_envelope, seal_envelope};

    /// Spawns a background Hyper HTTP server running `RelayMailbox` endpoints.
    async fn spawn_test_mailbox_server(
        mailbox: Arc<RelayMailbox>,
    ) -> (SocketAddr, oneshot::Sender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            let builder = Builder::new(TokioExecutor::new());
            loop {
                tokio::select! {
                    Ok((stream, _)) = listener.accept() => {
                        let m = mailbox.clone();
                        let b = builder.clone();
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            let service = hyper::service::service_fn(move |req: Request<Incoming>| {
                                let m2 = m.clone();
                                async move {
                                    let path = req.uri().path().to_string();
                                    let resp = if path == "/api/lanflare/v1/mesh/mailbox/deposit" {
                                        m2.handle_deposit_endpoint(req).await
                                    } else if path == "/api/lanflare/v1/mesh/mailbox/fetch" {
                                        m2.handle_fetch_endpoint(req).await
                                    } else {
                                        empty_box_response(StatusCode::NOT_FOUND)
                                    };
                                    Ok::<_, hyper::Error>(resp)
                                }
                            });
                            let _ = b.serve_connection(io, service).await;
                        });
                    }
                    _ = &mut shutdown_rx => break,
                }
            }
        });

        (addr, shutdown_tx)
    }

    #[tokio::test]
    pub async fn test_asynchronous_never_simultaneously_online_delivery() {
        // 1. Generate cryptographic identities:
        //    Node A (sender), Relay B (custodial mailbox), Node C (offline recipient)
        let id_a = generate_self_signed().expect("generate Node A identity");
        let id_b = generate_self_signed().expect("generate Relay B identity");
        let id_c = generate_self_signed().expect("generate Node C identity");

        assert_ne!(id_a.fingerprint, id_b.fingerprint);
        assert_ne!(id_b.fingerprint, id_c.fingerprint);

        // 2. Start Relay B's HTTP server with RelayMailbox
        let mailbox_b = Arc::new(RelayMailbox::new(MailboxConfig::default()));
        let (addr_b, shutdown_b) = spawn_test_mailbox_server(mailbox_b.clone()).await;
        let relay_b_url = format!("http://{addr_b}");

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        // 3. Node C is completely offline (no server running for C)

        // 4. Node A creates and seals an E2EE message for Node C
        let secret_payload = b"Top secret custodial message: Hello Node C from Node A via Relay B!";
        let sealed_envelope = seal_envelope(
            &id_a,
            &id_c.fingerprint,
            &id_c.cert_pem,
            secret_payload,
        )
        .expect("seal E2EE envelope for Node C");

        let deposit_req = MailboxDepositRequest {
            deposit_id: Some("deposit-msg-001".to_string()),
            envelope: sealed_envelope.clone(),
            ttl_seconds: Some(3600),
        };

        // 5. Node A deposits the envelope on Relay B
        let deposit_resp =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_b_url, &deposit_req)
                .await
                .expect("deposit envelope on Relay B");

        assert_eq!(deposit_resp.deposit_id, "deposit-msg-001");
        assert_eq!(deposit_resp.recipient_fp, id_c.fingerprint);
        assert!(deposit_resp.stored_bytes > 0);

        // 6. Node A goes offline (we drop Node A's identity and client)
        drop(id_a);

        // 7. Verify Relay B CANNOT decrypt the message
        //    Relay B attempts to open the envelope with its own identity id_b -> MUST FAIL with RecipientMismatch
        let decrypt_attempt = open_envelope(&id_b, None, &sealed_envelope);
        assert!(
            decrypt_attempt.is_err(),
            "Relay B must never have access to the plaintext"
        );

        // 8. Node C comes online and fetches pending envelopes from Relay B
        let fetch_req = create_signed_fetch_request(&id_c, vec![], None, true)
            .expect("create signed fetch request for Node C");

        let fetch_resp = RelayMailbox::fetch_from_relay_http(&http_client, &relay_b_url, &fetch_req)
            .await
            .expect("fetch envelopes from Relay B");

        assert_eq!(fetch_resp.items.len(), 1);
        let fetched_item = &fetch_resp.items[0];
        assert_eq!(fetched_item.deposit_id, "deposit-msg-001");

        // 9. Node C decrypts the retrieved envelope with id_c
        let decrypted = open_envelope(&id_c, None, &fetched_item.envelope)
            .expect("Node C must successfully decrypt the envelope");
        assert_eq!(decrypted, secret_payload);

        // 10. Node C performs a second fetch -> since auto_ack was true, mailbox is now empty
        let fetch_req_2 = create_signed_fetch_request(&id_c, vec![], None, true).unwrap();
        let fetch_resp_2 =
            RelayMailbox::fetch_from_relay_http(&http_client, &relay_b_url, &fetch_req_2)
                .await
                .unwrap();
        assert_eq!(fetch_resp_2.items.len(), 0);
        assert_eq!(fetch_resp_2.remaining_count, 0);

        let _ = shutdown_b.send(());
    }

    #[tokio::test]
    pub async fn test_mailbox_quota_enforcement_and_ttl_eviction() {
        // Set strict test quotas:
        // max 2,500 bytes per peer, max 4,000 bytes total, default TTL = 2 seconds
        let config = MailboxConfig {
            max_bytes_per_peer: 2500,
            max_total_bytes: 4000,
            max_items_per_peer: 10,
            default_ttl_secs: 2,
            max_ttl_secs: 10,
        };

        let mailbox = Arc::new(RelayMailbox::new(config));
        let (addr, shutdown) = spawn_test_mailbox_server(mailbox.clone()).await;
        let relay_url = format!("http://{addr}");

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();

        let id_a = crate::crypto::cert::generate_self_signed_ed25519().unwrap();
        let id_c = generate_self_signed().unwrap();
        let id_d = generate_self_signed().unwrap();

        // 1. First deposit for Recipient C (~2100 bytes) -> SUCCESS
        let env_c1 = seal_envelope(&id_a, &id_c.fingerprint, &id_c.cert_pem, b"First message for C")
            .unwrap();
        let dep_c1 = MailboxDepositRequest {
            deposit_id: Some("c1".into()),
            envelope: env_c1,
            ttl_seconds: Some(1), // 1 second TTL
        };
        let resp_c1 =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_url, &dep_c1).await;
        assert!(resp_c1.is_ok(), "First deposit for C should succeed");

        // 2. Second deposit for Recipient C (~2100 bytes) -> Total for C is ~4200 <= 5000 -> SUCCESS
        let env_c2 = seal_envelope(&id_a, &id_c.fingerprint, &id_c.cert_pem, b"Second message for C")
            .unwrap();
        let dep_c2 = MailboxDepositRequest {
            deposit_id: Some("c2".into()),
            envelope: env_c2,
            ttl_seconds: Some(1),
        };
        let resp_c2 =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_url, &dep_c2).await;
        assert!(resp_c2.is_ok(), "Second deposit for C should succeed");

        // 3. Third deposit for Recipient C (~2100 bytes) -> Total for C would be ~6300 > 5000
        //    MUST FAIL with HTTP 413 / PeerQuotaExceeded
        let env_c3 = seal_envelope(&id_a, &id_c.fingerprint, &id_c.cert_pem, b"Third message for C")
            .unwrap();
        let dep_c3 = MailboxDepositRequest {
            deposit_id: Some("c3".into()),
            envelope: env_c3,
            ttl_seconds: Some(1),
        };
        let resp_c3 =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_url, &dep_c3).await;
        match resp_c3 {
            Err(MailboxError::PeerQuotaExceeded { .. }) => {
                // Expected!
            }
            other => panic!("Expected PeerQuotaExceeded, got: {other:?}"),
        }

        // 4. Deposit for Recipient D (~2100 bytes)
        //    Recipient D has 0 bytes used, so D is NOT starved despite C exceeding quota!
        let env_d1 = seal_envelope(&id_a, &id_d.fingerprint, &id_d.cert_pem, b"Message for D")
            .unwrap();
        let dep_d1 = MailboxDepositRequest {
            deposit_id: Some("d1".into()),
            envelope: env_d1,
            ttl_seconds: Some(5),
        };
        let resp_d1 =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_url, &dep_d1).await;
        assert!(
            resp_d1.is_ok(),
            "Recipient D must not be starved by Recipient C"
        );

        // 5. Test total mailbox quota:
        //    Currently used: C1 (~2100) + C2 (~2100) + D1 (~2100) = ~6300 bytes <= 8000 bytes.
        //    Attempting another deposit for D (~2100 bytes) would push total to ~8400 > 8000 bytes.
        let env_d2 = seal_envelope(&id_a, &id_d.fingerprint, &id_d.cert_pem, b"Second message for D")
            .unwrap();
        let resp_total_overflow = RelayMailbox::deposit_to_relay_http(
            &http_client,
            &relay_url,
            &MailboxDepositRequest {
                deposit_id: Some("d2".into()),
                envelope: env_d2,
                ttl_seconds: Some(5),
            },
        )
        .await;

        match resp_total_overflow {
            Err(MailboxError::TotalQuotaExceeded { .. }) => {
                // Expected!
            }
            other => panic!("Expected TotalQuotaExceeded, got: {other:?}"),
        }

        // 6. Test TTL eviction: C1 and C2 had 1-second TTL.
        //    Wait 1.2 seconds for C1 and C2 to expire.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let evicted = mailbox.evict_expired().await;
        assert!(evicted >= 2, "Expired C entries should be evicted");

        // Now Recipient C has 0 bytes used again, so a new deposit for C MUST succeed!
        let env_c4 = seal_envelope(&id_a, &id_c.fingerprint, &id_c.cert_pem, b"Fresh message for C")
            .unwrap();
        let dep_c4 = MailboxDepositRequest {
            deposit_id: Some("c4".into()),
            envelope: env_c4,
            ttl_seconds: Some(5),
        };
        let resp_c4 =
            RelayMailbox::deposit_to_relay_http(&http_client, &relay_url, &dep_c4).await;
        assert!(
            resp_c4.is_ok(),
            "Deposit for C should succeed after TTL eviction freed quota"
        );

        let _ = shutdown.send(());
    }
}
