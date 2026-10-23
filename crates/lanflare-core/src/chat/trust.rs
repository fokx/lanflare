//! 5-Layer Anti-Harassment & Trust Gate (`lanflare-core::chat::trust`).
//!
//! Enforces zero-trust protection on open LANs:
//! 1. 3-tier contact trust state machine (`Stranger`, `PendingApproval`, `TrustedContact`, `Blocked`).
//! 2. Anti-Sybil Hashcash Proof-of-Work (PoW) generator and O(1) verifier.
//! 3. Single 280-char plain-text introduction limit for strangers; strict media attachment block.
//! 4. Per-IP and per-fingerprint token-bucket rate limiting against identity rotation attacks.
//! 5. Global "Contacts Only" mode rejecting all unsolicited stranger handshakes.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, RwLock};

use lanflare_proto::chat::{
    ChatMessageDto, HashcashStamp, TrustRequestDto, DEFAULT_POW_DIFFICULTY_BITS,
    DEFAULT_TIME_BUCKET_SECS, MAX_INTRO_MESSAGE_LEN,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Contact trust states within the LanFlare trust state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContactTrustState {
    /// Unknown fingerprint. Can only send at most one <=280 char plain-text request with PoW.
    Stranger,
    /// Has sent a valid introduction request, awaiting local user review and approval.
    PendingApproval,
    /// Verified and accepted contact. Permitted full rich media, outbox queues, and resumable transfers.
    TrustedContact,
    /// Explicitly blocked peer. All traffic dropped silently in O(1) time before any disk I/O.
    Blocked,
}

/// Errors originating from the trust gate and anti-harassment engine.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrustError {
    /// Contact fingerprint or IP address is blocked.
    #[error("Contact or address is blocked: {0}")]
    Blocked(String),

    /// Stranger already has a pending introduction request locked awaiting approval.
    #[error("Stranger request pending approval; locked until accepted")]
    PendingApprovalLocked,

    /// "Contacts Only" mode is enabled, rejecting all unsolicited stranger requests.
    #[error("Contacts only mode enabled; stranger rejected")]
    ContactsOnlyMode,

    /// Hashcash Proof-of-Work was absent, invalid, or did not meet the required difficulty.
    #[error("Proof of work invalid or insufficient bits: required {required}, found {actual}")]
    InsufficientPoW { required: u32, actual: u32 },

    /// Hashcash Proof-of-Work time bucket has expired or exceeds allowed clock skew.
    #[error("Proof of work expired or invalid time bucket: expected {expected_bucket}, got {actual_bucket}")]
    PoWExpired {
        expected_bucket: u64,
        actual_bucket: u64,
    },

    /// Hashcash Proof-of-Work target fingerprint does not match the recipient node.
    #[error("Proof of work target mismatch: expected {expected}, got {actual}")]
    PoWTargetMismatch { expected: String, actual: String },

    /// Request rate limit exceeded for the specified IP or fingerprint.
    #[error("Rate limit exceeded for {0}")]
    RateLimited(String),

    /// Introduction message exceeds maximum allowed character length.
    #[error("Message too long: {length} chars (maximum {max})")]
    MessageTooLong { length: usize, max: usize },

    /// Stranger attempted to transmit media attachments (Image, File, Voice, Video).
    #[error("Media attachments not permitted for strangers")]
    MediaNotPermittedForStranger,

    /// Hashcash computation reached maximum search iterations without finding a valid nonce.
    #[error("PoW generation timed out or exceeded max iterations ({0})")]
    PoWExhausted(u64),

    /// Malformed or mismatched request parameters.
    #[error("Invalid request: {0}")]
    InvalidRequest(String),
}

/// Computes the number of leading zero bits in a slice of bytes.
pub fn count_leading_zeros(bytes: &[u8]) -> u32 {
    let mut zeros = 0;
    for &b in bytes {
        let lz = b.leading_zeros();
        zeros += lz;
        if lz < 8 {
            break;
        }
    }
    zeros
}

/// Computes the time bucket for a given timestamp in seconds.
pub fn compute_time_bucket(now_secs: u64, bucket_duration_secs: u64) -> u64 {
    if bucket_duration_secs == 0 {
        return 0;
    }
    now_secs / bucket_duration_secs
}

/// Generates a valid Hashcash Proof-of-Work stamp for a given target.
///
/// Sweeps `nonce` from 0 up to `max_iterations`, hashing `sender_fp:target_fp:time_bucket:nonce`
/// until `LeadingZeroBits(SHA256(payload)) >= bits`.
pub fn generate_hashcash_pow(
    sender_fp: &str,
    target_fp: &str,
    bits: u32,
    time_bucket: u64,
    max_iterations: u64,
) -> Result<HashcashStamp, TrustError> {
    let sender_canon = sender_fp.to_ascii_uppercase();
    let target_canon = target_fp.to_ascii_uppercase();

    for nonce in 0..max_iterations {
        let payload =
            HashcashStamp::canonical_payload(&sender_canon, &target_canon, time_bucket, nonce);
        let digest = Sha256::digest(&payload);
        let lz = count_leading_zeros(&digest);
        if lz >= bits {
            return Ok(HashcashStamp::new(
                bits,
                sender_canon,
                target_canon,
                time_bucket,
                nonce,
            ));
        }
    }

    Err(TrustError::PoWExhausted(max_iterations))
}

/// Verifies a Hashcash Proof-of-Work stamp in O(1) time (single SHA-256 computation).
pub fn verify_hashcash_pow(
    stamp: &HashcashStamp,
    expected_target_fp: &str,
    required_bits: u32,
    current_time_bucket: u64,
    max_bucket_skew: u64,
) -> Result<(), TrustError> {
    if !stamp.target_fp.eq_ignore_ascii_case(expected_target_fp) {
        return Err(TrustError::PoWTargetMismatch {
            expected: expected_target_fp.to_ascii_uppercase(),
            actual: stamp.target_fp.to_ascii_uppercase(),
        });
    }

    if stamp.time_bucket.abs_diff(current_time_bucket) > max_bucket_skew {
        return Err(TrustError::PoWExpired {
            expected_bucket: current_time_bucket,
            actual_bucket: stamp.time_bucket,
        });
    }

    if stamp.bits < required_bits {
        return Err(TrustError::InsufficientPoW {
            required: required_bits,
            actual: stamp.bits,
        });
    }

    let payload = stamp.canonical_bytes();
    let digest = Sha256::digest(&payload);
    let actual_lz = count_leading_zeros(&digest);

    if actual_lz < stamp.bits || actual_lz < required_bits {
        return Err(TrustError::InsufficientPoW {
            required: required_bits,
            actual: actual_lz,
        });
    }

    Ok(())
}

/// Token bucket algorithm for rate-limiting incoming requests per IP or fingerprint.
#[derive(Clone, Debug)]
pub struct TokenBucket {
    /// Maximum burst token capacity.
    pub capacity: f64,
    /// Currently available tokens.
    pub tokens: f64,
    /// Token refill rate per second.
    pub refill_rate_per_sec: f64,
    /// Timestamp in seconds of the last consumption / refill update.
    pub last_update_secs: f64,
}

impl TokenBucket {
    /// Creates a new `TokenBucket` initialized at full burst capacity.
    pub fn new(capacity: f64, refill_rate_per_sec: f64, now_secs: f64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            refill_rate_per_sec,
            last_update_secs: now_secs,
        }
    }

    /// Attempts to consume `requested` tokens at timestamp `now_secs`.
    ///
    /// Returns `true` if sufficient tokens were available, `false` otherwise.
    pub fn try_consume(&mut self, now_secs: f64, requested: f64) -> bool {
        let elapsed = (now_secs - self.last_update_secs).max(0.0);
        self.tokens = (self.tokens + elapsed * self.refill_rate_per_sec).min(self.capacity);
        self.last_update_secs = now_secs;

        if self.tokens >= requested {
            self.tokens -= requested;
            true
        } else {
            false
        }
    }
}

/// Configuration settings for the Anti-Harassment Trust Gate.
#[derive(Clone, Debug)]
pub struct TrustGateConfig {
    /// Number of leading zero bits required for Hashcash Proof-of-Work (default: 16).
    pub required_pow_bits: u32,

    /// Duration of each time bucket in seconds (default: 300 = 5 minutes).
    pub time_bucket_duration_secs: u64,

    /// Maximum allowed bucket skew (default: 1 bucket before or after).
    pub max_bucket_skew: u64,

    /// Maximum allowed length (characters) for an intro message from a stranger (default: 280).
    pub max_intro_message_len: usize,

    /// If true, reject all strangers immediately regardless of PoW (default: false).
    pub contacts_only: bool,

    /// Token bucket burst capacity per IP address (default: 5).
    pub ip_rate_limit_capacity: f64,

    /// Token bucket refill rate in tokens per second per IP (default: 0.1 = 1 per 10s).
    pub ip_rate_limit_refill_per_sec: f64,

    /// Token bucket burst capacity per fingerprint (default: 10).
    pub fp_rate_limit_capacity: f64,

    /// Token bucket refill rate in tokens per second per fingerprint (default: 0.5 = 1 per 2s).
    pub fp_rate_limit_refill_per_sec: f64,
}

impl Default for TrustGateConfig {
    fn default() -> Self {
        Self {
            required_pow_bits: DEFAULT_POW_DIFFICULTY_BITS,
            time_bucket_duration_secs: DEFAULT_TIME_BUCKET_SECS,
            max_bucket_skew: 1,
            max_intro_message_len: MAX_INTRO_MESSAGE_LEN,
            contacts_only: false,
            ip_rate_limit_capacity: 5.0,
            ip_rate_limit_refill_per_sec: 0.1,
            fp_rate_limit_capacity: 10.0,
            fp_rate_limit_refill_per_sec: 0.5,
        }
    }
}

#[derive(Debug)]
struct TrustGateInner {
    contacts: HashMap<String, ContactTrustState>,
    pending_requests: HashMap<String, TrustRequestDto>,
    blocked_ips: HashSet<IpAddr>,
    ip_limiters: HashMap<IpAddr, TokenBucket>,
    fp_limiters: HashMap<String, TokenBucket>,
    contacts_only: bool,
}

/// 5-Layer Anti-Harassment Trust Gate protecting local nodes on untrusted LANs.
#[derive(Clone, Debug)]
pub struct TrustGate {
    local_fp: String,
    config: TrustGateConfig,
    inner: Arc<RwLock<TrustGateInner>>,
}

impl TrustGate {
    /// Creates a new `TrustGate` instance bound to the local certificate SHA-256 fingerprint.
    pub fn new(local_fp: impl Into<String>, config: TrustGateConfig) -> Self {
        let contacts_only = config.contacts_only;
        Self {
            local_fp: local_fp.into().to_ascii_uppercase(),
            config,
            inner: Arc::new(RwLock::new(TrustGateInner {
                contacts: HashMap::new(),
                pending_requests: HashMap::new(),
                blocked_ips: HashSet::new(),
                ip_limiters: HashMap::new(),
                fp_limiters: HashMap::new(),
                contacts_only,
            })),
        }
    }

    /// Returns the canonical local fingerprint.
    pub fn local_fp(&self) -> &str {
        &self.local_fp
    }

    /// Returns the configuration currently in effect.
    pub fn config(&self) -> &TrustGateConfig {
        &self.config
    }

    /// Returns the current contact trust state for a given fingerprint.
    pub fn get_trust_state(&self, fingerprint: &str) -> ContactTrustState {
        let inner = self.inner.read().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner
            .contacts
            .get(&canon)
            .copied()
            .unwrap_or(ContactTrustState::Stranger)
    }

    /// Explicitly sets the trust state for a given fingerprint.
    pub fn set_trust_state(&self, fingerprint: &str, state: ContactTrustState) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner.contacts.insert(canon, state);
    }

    /// Approves an introduction request, transitioning the contact to `TrustedContact`.
    pub fn approve_contact(&self, fingerprint: &str) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner
            .contacts
            .insert(canon.clone(), ContactTrustState::TrustedContact);
        inner.pending_requests.remove(&canon);
    }

    /// Blocks a contact by fingerprint, transitioning to `Blocked`.
    pub fn block_contact(&self, fingerprint: &str) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner
            .contacts
            .insert(canon.clone(), ContactTrustState::Blocked);
        inner.pending_requests.remove(&canon);
    }

    /// Unblocks a contact, resetting to `Stranger`.
    pub fn unblock_contact(&self, fingerprint: &str) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner.contacts.remove(&canon);
    }

    /// Adds an IP address to the explicit blocklist.
    pub fn block_ip(&self, ip: IpAddr) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        inner.blocked_ips.insert(ip);
    }

    /// Removes an IP address from the explicit blocklist.
    pub fn unblock_ip(&self, ip: &IpAddr) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        inner.blocked_ips.remove(ip);
    }

    /// Returns true if an IP address is blocked.
    pub fn is_ip_blocked(&self, ip: &IpAddr) -> bool {
        let inner = self.inner.read().expect("trust gate lock poisoned");
        inner.blocked_ips.contains(ip)
    }

    /// Enables or disables "Contacts Only" mode.
    pub fn set_contacts_only(&self, enabled: bool) {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        inner.contacts_only = enabled;
    }

    /// Returns whether "Contacts Only" mode is enabled.
    pub fn is_contacts_only(&self) -> bool {
        let inner = self.inner.read().expect("trust gate lock poisoned");
        inner.contacts_only
    }

    /// Retrieves a recorded pending introduction request for a fingerprint if present.
    pub fn get_pending_request(&self, fingerprint: &str) -> Option<TrustRequestDto> {
        let inner = self.inner.read().expect("trust gate lock poisoned");
        let canon = fingerprint.to_ascii_uppercase();
        inner.pending_requests.get(&canon).cloned()
    }

    /// Lists all pending introduction requests.
    pub fn list_pending_requests(&self) -> Vec<TrustRequestDto> {
        let inner = self.inner.read().expect("trust gate lock poisoned");
        inner.pending_requests.values().cloned().collect()
    }

    /// Consumes one token from the IP rate limiter, returning error if rate-limited.
    fn consume_ip_rate_limit(&self, ip: IpAddr, now_secs: u64) -> Result<(), TrustError> {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let capacity = self.config.ip_rate_limit_capacity;
        let refill_rate = self.config.ip_rate_limit_refill_per_sec;
        let now_f64 = now_secs as f64;

        let bucket = inner
            .ip_limiters
            .entry(ip)
            .or_insert_with(|| TokenBucket::new(capacity, refill_rate, now_f64));

        if bucket.try_consume(now_f64, 1.0) {
            Ok(())
        } else {
            Err(TrustError::RateLimited(format!("IP {ip}")))
        }
    }

    /// Consumes one token from the fingerprint rate limiter, returning error if rate-limited.
    fn consume_fp_rate_limit(&self, fp: &str, now_secs: u64) -> Result<(), TrustError> {
        let mut inner = self.inner.write().expect("trust gate lock poisoned");
        let capacity = self.config.fp_rate_limit_capacity;
        let refill_rate = self.config.fp_rate_limit_refill_per_sec;
        let now_f64 = now_secs as f64;
        let canon = fp.to_ascii_uppercase();

        let bucket = inner
            .fp_limiters
            .entry(canon.clone())
            .or_insert_with(|| TokenBucket::new(capacity, refill_rate, now_f64));

        if bucket.try_consume(now_f64, 1.0) {
            Ok(())
        } else {
            Err(TrustError::RateLimited(format!("Fingerprint {canon}")))
        }
    }

    /// Evaluates an incoming `TrustRequestDto` from a remote peer.
    ///
    /// Validates IP blocklist, IP/FP rate limits, contacts-only mode, 280-char message bounds,
    /// and Hashcash Proof-of-Work difficulty before transitioning state to `PendingApproval`.
    pub fn evaluate_trust_request(
        &self,
        request: &TrustRequestDto,
        client_ip: IpAddr,
        now_secs: u64,
    ) -> Result<(), TrustError> {
        // 1. IP Blocklist check
        if self.is_ip_blocked(&client_ip) {
            return Err(TrustError::Blocked(format!("IP {client_ip}")));
        }

        // 2. IP Rate limiter check
        self.consume_ip_rate_limit(client_ip, now_secs)?;

        // 3. Sender fingerprint canonicalization and rate limit
        let sender_fp = request.sender_fp.to_ascii_uppercase();
        self.consume_fp_rate_limit(&sender_fp, now_secs)?;

        // 4. Contact trust state check
        let state = self.get_trust_state(&sender_fp);
        match state {
            ContactTrustState::Blocked => Err(TrustError::Blocked(sender_fp)),
            ContactTrustState::TrustedContact => Ok(()),
            ContactTrustState::PendingApproval => Err(TrustError::PendingApprovalLocked),
            ContactTrustState::Stranger => {
                // Check contacts only
                if self.is_contacts_only() {
                    return Err(TrustError::ContactsOnlyMode);
                }

                // Check message character limit (max 280 chars)
                let char_count = request.message.chars().count();
                if char_count > self.config.max_intro_message_len {
                    return Err(TrustError::MessageTooLong {
                        length: char_count,
                        max: self.config.max_intro_message_len,
                    });
                }

                // Validate PoW sender matches request
                if !request
                    .pow_stamp
                    .sender_fp
                    .eq_ignore_ascii_case(&request.sender_fp)
                {
                    return Err(TrustError::InvalidRequest(
                        "Sender fingerprint in stamp does not match request".into(),
                    ));
                }

                // Verify Hashcash PoW stamp
                let current_bucket =
                    compute_time_bucket(now_secs, self.config.time_bucket_duration_secs);
                verify_hashcash_pow(
                    &request.pow_stamp,
                    &self.local_fp,
                    self.config.required_pow_bits,
                    current_bucket,
                    self.config.max_bucket_skew,
                )?;

                // Valid introduction! Transition state to PendingApproval
                let mut inner = self.inner.write().expect("trust gate lock poisoned");
                inner
                    .contacts
                    .insert(sender_fp.clone(), ContactTrustState::PendingApproval);
                inner.pending_requests.insert(sender_fp, request.clone());

                Ok(())
            }
        }
    }

    /// Evaluates an incoming `ChatMessageDto`.
    ///
    /// If the sender is `TrustedContact`, message is accepted immediately.
    /// If the sender is `PendingApproval`, rejects with `TrustError::PendingApprovalLocked`.
    /// If the sender is `Blocked`, rejects with `TrustError::Blocked`.
    /// If the sender is `Stranger`:
    /// - Rejects rich media attachments (Image, File, Voice, Video) immediately.
    /// - Enforces 280-char plain-text limit.
    /// - Requires and verifies a valid `HashcashStamp`.
    /// - On success, transitions state to `PendingApproval`.
    pub fn evaluate_chat_message(
        &self,
        message: &ChatMessageDto,
        pow_stamp: Option<&HashcashStamp>,
        client_ip: IpAddr,
        now_secs: u64,
    ) -> Result<(), TrustError> {
        // 1. IP Blocklist check
        if self.is_ip_blocked(&client_ip) {
            return Err(TrustError::Blocked(format!("IP {client_ip}")));
        }

        // 2. Sender fingerprint canonicalization
        let sender_fp = message.sender_fp.to_ascii_uppercase();
        let state = self.get_trust_state(&sender_fp);

        match state {
            ContactTrustState::Blocked => Err(TrustError::Blocked(sender_fp)),
            ContactTrustState::PendingApproval => Err(TrustError::PendingApprovalLocked),
            ContactTrustState::TrustedContact => {
                // Trusted contacts pass straight through without PoW or character limits
                Ok(())
            }
            ContactTrustState::Stranger => {
                // IP & FP rate limiter
                self.consume_ip_rate_limit(client_ip, now_secs)?;
                self.consume_fp_rate_limit(&sender_fp, now_secs)?;

                // Contacts only check
                if self.is_contacts_only() {
                    return Err(TrustError::ContactsOnlyMode);
                }

                // Strangers cannot send media attachments (zero disk I/O guarantee)
                if message.content.is_media() {
                    return Err(TrustError::MediaNotPermittedForStranger);
                }

                // Check text length limit (max 280 chars)
                let text = message.content.text_content().unwrap_or("");
                let char_count = text.chars().count();
                if char_count > self.config.max_intro_message_len {
                    return Err(TrustError::MessageTooLong {
                        length: char_count,
                        max: self.config.max_intro_message_len,
                    });
                }

                // Require valid PoW stamp for stranger handshake
                let stamp = pow_stamp.ok_or(TrustError::InsufficientPoW {
                    required: self.config.required_pow_bits,
                    actual: 0,
                })?;

                if !stamp.sender_fp.eq_ignore_ascii_case(&message.sender_fp) {
                    return Err(TrustError::InvalidRequest(
                        "Sender fingerprint in stamp does not match message".into(),
                    ));
                }

                let current_bucket =
                    compute_time_bucket(now_secs, self.config.time_bucket_duration_secs);
                verify_hashcash_pow(
                    stamp,
                    &self.local_fp,
                    self.config.required_pow_bits,
                    current_bucket,
                    self.config.max_bucket_skew,
                )?;

                // Valid first message! Transition to PendingApproval
                let mut inner = self.inner.write().expect("trust gate lock poisoned");
                inner
                    .contacts
                    .insert(sender_fp, ContactTrustState::PendingApproval);

                Ok(())
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use lanflare_proto::chat::ChatContentDto;
    use std::net::Ipv4Addr;

    const LOCAL_FP: &str =
        "111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF00000001";
    const SENDER_FP: &str =
        "22223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF000000010002";
    const ROGUE_FP: &str =
        "999988887777666655554444333322221111AAAABBBBCCCCDDDDEEEEFFFF00000099";

    #[test]
    fn test_hashcash_pow_generation_and_verification() {
        let now_secs = 1700000000;
        let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);
        let difficulty = 12; // 12 bits leading zeros = rapid generation

        // 1. Generate valid stamp
        let stamp = generate_hashcash_pow(SENDER_FP, LOCAL_FP, difficulty, time_bucket, 1_000_000)
            .expect("generate pow stamp");
        assert_eq!(stamp.bits, difficulty);
        assert_eq!(stamp.sender_fp, SENDER_FP);
        assert_eq!(stamp.target_fp, LOCAL_FP);
        assert_eq!(stamp.time_bucket, time_bucket);

        // 2. Successful O(1) verification
        assert!(verify_hashcash_pow(&stamp, LOCAL_FP, difficulty, time_bucket, 1).is_ok());

        // 3. Case-insensitive fingerprint matching
        assert!(verify_hashcash_pow(
            &stamp,
            &LOCAL_FP.to_ascii_lowercase(),
            difficulty,
            time_bucket,
            1
        )
        .is_ok());

        // 4. Reject target mismatch
        let mismatch_err =
            verify_hashcash_pow(&stamp, ROGUE_FP, difficulty, time_bucket, 1).unwrap_err();
        match mismatch_err {
            TrustError::PoWTargetMismatch { expected, actual } => {
                assert_eq!(expected, ROGUE_FP);
                assert_eq!(actual, LOCAL_FP);
            }
            other => panic!("expected PoWTargetMismatch, got {:?}", other),
        }

        // 5. Reject expired time bucket (skew > max_bucket_skew)
        let expired_err =
            verify_hashcash_pow(&stamp, LOCAL_FP, difficulty, time_bucket + 5, 1).unwrap_err();
        match expired_err {
            TrustError::PoWExpired {
                expected_bucket,
                actual_bucket,
            } => {
                assert_eq!(expected_bucket, time_bucket + 5);
                assert_eq!(actual_bucket, time_bucket);
            }
            other => panic!("expected PoWExpired, got {:?}", other),
        }

        // 6. Reject tampered nonce / insufficient bits
        let mut tampered_stamp = stamp.clone();
        tampered_stamp.nonce += 1;
        // With high probability nonce+1 does not have 12 leading zeros
        let tampered_res =
            verify_hashcash_pow(&tampered_stamp, LOCAL_FP, difficulty, time_bucket, 1);
        if let Err(TrustError::InsufficientPoW { required, .. }) = tampered_res {
            assert_eq!(required, difficulty);
        } else {
            // In the minuscule chance nonce+1 also hits 12 zero bits, check bits requirement
            let impossible_difficulty_err =
                verify_hashcash_pow(&stamp, LOCAL_FP, 64, time_bucket, 1).unwrap_err();
            assert!(matches!(
                impossible_difficulty_err,
                TrustError::InsufficientPoW { .. }
            ));
        }
    }

    #[test]
    fn test_stranger_single_message_gate_and_media_block() {
        let config = TrustGateConfig {
            required_pow_bits: 12,
            max_intro_message_len: 280,
            ip_rate_limit_capacity: 10.0,
            fp_rate_limit_capacity: 10.0,
            ..Default::default()
        };
        let gate = TrustGate::new(LOCAL_FP, config);
        let client_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        let now_secs = 1700000000;
        let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);

        assert_eq!(gate.get_trust_state(SENDER_FP), ContactTrustState::Stranger);

        // 1. Stranger sends media attachment directly -> Rejected immediately (Zero disk I/O)
        let image_msg = ChatMessageDto {
            id: "msg-img-01".to_string(),
            conversation_id: "conv-01".to_string(),
            sender_fp: SENDER_FP.to_string(),
            recipient_fp: LOCAL_FP.to_string(),
            timestamp_ms: now_secs * 1000,
            content: ChatContentDto::Image {
                file_name: "payload.jpg".to_string(),
                size_bytes: 5242880,
                mime_type: "image/jpeg".to_string(),
                sha256: "AAAA".to_string(),
                thumbnail_base64: None,
            },
            reply_to_id: None,
            signature: None,
        };
        let media_err = gate
            .evaluate_chat_message(&image_msg, None, client_ip, now_secs)
            .unwrap_err();
        assert_eq!(media_err, TrustError::MediaNotPermittedForStranger);

        // 2. Stranger sends message exceeding 280 characters -> Rejected
        let long_text = "A".repeat(281);
        let long_msg = ChatMessageDto {
            id: "msg-long-01".to_string(),
            conversation_id: "conv-01".to_string(),
            sender_fp: SENDER_FP.to_string(),
            recipient_fp: LOCAL_FP.to_string(),
            timestamp_ms: now_secs * 1000,
            content: ChatContentDto::Text { text: long_text },
            reply_to_id: None,
            signature: None,
        };
        let long_err = gate
            .evaluate_chat_message(&long_msg, None, client_ip, now_secs)
            .unwrap_err();
        assert_eq!(
            long_err,
            TrustError::MessageTooLong {
                length: 281,
                max: 280
            }
        );

        // 3. Stranger sends valid length text without PoW -> Rejected
        let valid_text_msg = ChatMessageDto {
            id: "msg-valid-01".to_string(),
            conversation_id: "conv-01".to_string(),
            sender_fp: SENDER_FP.to_string(),
            recipient_fp: LOCAL_FP.to_string(),
            timestamp_ms: now_secs * 1000,
            content: ChatContentDto::Text {
                text: "Hello! May I connect?".to_string(),
            },
            reply_to_id: None,
            signature: None,
        };
        let no_pow_err = gate
            .evaluate_chat_message(&valid_text_msg, None, client_ip, now_secs)
            .unwrap_err();
        assert_eq!(
            no_pow_err,
            TrustError::InsufficientPoW {
                required: 12,
                actual: 0
            }
        );

        // 4. Stranger sends valid text message WITH valid PoW -> Accepted & transitions to PendingApproval
        let stamp = generate_hashcash_pow(SENDER_FP, LOCAL_FP, 12, time_bucket, 1_000_000)
            .expect("generate pow");
        let ok_result = gate.evaluate_chat_message(
            &valid_text_msg,
            Some(&stamp),
            client_ip,
            now_secs,
        );
        assert!(ok_result.is_ok());
        assert_eq!(
            gate.get_trust_state(SENDER_FP),
            ContactTrustState::PendingApproval
        );

        // 5. Second message from stranger while in PendingApproval -> Rejected with PendingApprovalLocked
        let second_msg = ChatMessageDto {
            id: "msg-valid-02".to_string(),
            conversation_id: "conv-01".to_string(),
            sender_fp: SENDER_FP.to_string(),
            recipient_fp: LOCAL_FP.to_string(),
            timestamp_ms: now_secs * 1000 + 500,
            content: ChatContentDto::Text {
                text: "Are you there?".to_string(),
            },
            reply_to_id: None,
            signature: None,
        };
        let second_stamp =
            generate_hashcash_pow(SENDER_FP, LOCAL_FP, 12, time_bucket, 1_000_000)
                .expect("generate pow");
        let locked_err = gate
            .evaluate_chat_message(
                &second_msg,
                Some(&second_stamp),
                client_ip,
                now_secs,
            )
            .unwrap_err();
        assert_eq!(locked_err, TrustError::PendingApprovalLocked);

        // 6. User approves contact -> State transitions to TrustedContact
        gate.approve_contact(SENDER_FP);
        assert_eq!(
            gate.get_trust_state(SENDER_FP),
            ContactTrustState::TrustedContact
        );

        // 7. Now peer can send rich media and unlimited messages without PoW
        let trusted_image_res =
            gate.evaluate_chat_message(&image_msg, None, client_ip, now_secs);
        assert!(trusted_image_res.is_ok());

        // 8. User blocks contact -> State transitions to Blocked
        gate.block_contact(SENDER_FP);
        assert_eq!(
            gate.get_trust_state(SENDER_FP),
            ContactTrustState::Blocked
        );
        let blocked_err = gate
            .evaluate_chat_message(&second_msg, None, client_ip, now_secs)
            .unwrap_err();
        assert_eq!(blocked_err, TrustError::Blocked(SENDER_FP.to_string()));

        // 9. Contacts only mode rejects new strangers immediately
        gate.set_contacts_only(true);
        assert!(gate.is_contacts_only());
        let new_stranger_fp = "3333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF000000010003";
        let stranger_stamp =
            generate_hashcash_pow(new_stranger_fp, LOCAL_FP, 12, time_bucket, 1_000_000)
                .expect("pow");
        let stranger_req = TrustRequestDto {
            request_id: "req-01".to_string(),
            sender_fp: new_stranger_fp.to_string(),
            sender_alias: "Bob".to_string(),
            target_fp: LOCAL_FP.to_string(),
            message: "Hi".to_string(),
            pow_stamp: stranger_stamp,
            timestamp_ms: now_secs * 1000,
            signature: None,
        };
        let contacts_only_err = gate
            .evaluate_trust_request(&stranger_req, client_ip, now_secs)
            .unwrap_err();
        assert_eq!(contacts_only_err, TrustError::ContactsOnlyMode);
    }

    #[test]
    fn test_sybil_identity_rotation_ip_rate_limiter() {
        let config = TrustGateConfig {
            required_pow_bits: 10, // 10 bits = lightning fast PoW
            ip_rate_limit_capacity: 3.0,
            ip_rate_limit_refill_per_sec: 0.0, // No refill during test burst
            fp_rate_limit_capacity: 10.0,
            ..Default::default()
        };
        let gate = TrustGate::new(LOCAL_FP, config);
        let attacker_ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 99));
        let now_secs = 1700000000;
        let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);

        // Attacker attempts Sybil rotation: generates 4 distinct fingerprints from the same IP
        let sybil_fps = [
            "1111111111111111111111111111111111111111111111111111111111110001",
            "2222222222222222222222222222222222222222222222222222222222220002",
            "3333333333333333333333333333333333333333333333333333333333330003",
            "4444444444444444444444444444444444444444444444444444444444440004",
        ];

        // 1st request from IP: succeeds
        let stamp1 = generate_hashcash_pow(sybil_fps[0], LOCAL_FP, 10, time_bucket, 500_000).unwrap();
        let req1 = TrustRequestDto {
            request_id: "req-sybil-1".to_string(),
            sender_fp: sybil_fps[0].to_string(),
            sender_alias: "Sybil1".to_string(),
            target_fp: LOCAL_FP.to_string(),
            message: "Hello 1".to_string(),
            pow_stamp: stamp1,
            timestamp_ms: now_secs * 1000,
            signature: None,
        };
        assert!(gate.evaluate_trust_request(&req1, attacker_ip, now_secs).is_ok());

        // 2nd request from IP (different FP): succeeds
        let stamp2 = generate_hashcash_pow(sybil_fps[1], LOCAL_FP, 10, time_bucket, 500_000).unwrap();
        let req2 = TrustRequestDto {
            request_id: "req-sybil-2".to_string(),
            sender_fp: sybil_fps[1].to_string(),
            sender_alias: "Sybil2".to_string(),
            target_fp: LOCAL_FP.to_string(),
            message: "Hello 2".to_string(),
            pow_stamp: stamp2,
            timestamp_ms: now_secs * 1000,
            signature: None,
        };
        assert!(gate.evaluate_trust_request(&req2, attacker_ip, now_secs).is_ok());

        // 3rd request from IP (different FP): succeeds (capacity 3.0 exhausted)
        let stamp3 = generate_hashcash_pow(sybil_fps[2], LOCAL_FP, 10, time_bucket, 500_000).unwrap();
        let req3 = TrustRequestDto {
            request_id: "req-sybil-3".to_string(),
            sender_fp: sybil_fps[2].to_string(),
            sender_alias: "Sybil3".to_string(),
            target_fp: LOCAL_FP.to_string(),
            message: "Hello 3".to_string(),
            pow_stamp: stamp3,
            timestamp_ms: now_secs * 1000,
            signature: None,
        };
        assert!(gate.evaluate_trust_request(&req3, attacker_ip, now_secs).is_ok());

        // 4th request from IP (different FP, fresh PoW): REJECTED by IP rate limiter!
        let stamp4 = generate_hashcash_pow(sybil_fps[3], LOCAL_FP, 10, time_bucket, 500_000).unwrap();
        let req4 = TrustRequestDto {
            request_id: "req-sybil-4".to_string(),
            sender_fp: sybil_fps[3].to_string(),
            sender_alias: "Sybil4".to_string(),
            target_fp: LOCAL_FP.to_string(),
            message: "Hello 4".to_string(),
            pow_stamp: stamp4,
            timestamp_ms: now_secs * 1000,
            signature: None,
        };
        let rate_limited_err = gate
            .evaluate_trust_request(&req4, attacker_ip, now_secs)
            .unwrap_err();
        assert_eq!(
            rate_limited_err,
            TrustError::RateLimited(format!("IP {attacker_ip}"))
        );

        // 5. Block the attacker's IP explicitly
        gate.block_ip(attacker_ip);
        assert!(gate.is_ip_blocked(&attacker_ip));

        let blocked_ip_err = gate
            .evaluate_trust_request(&req1, attacker_ip, now_secs)
            .unwrap_err();
        assert_eq!(
            blocked_ip_err,
            TrustError::Blocked(format!("IP {attacker_ip}"))
        );
    }
}
