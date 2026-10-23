//! Hybrid Logical Clock (HLC), Causal Merkle-DAG, and serverless group key management.
//!
//! Provides distributed causal ordering, tamper-evident DAG state tracking,
//! and pairwise-encrypted group epoch keys for decentralized mesh group chat.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use lanflare_proto::chat::{
    GroupDagNodeDto, GroupKeyDistributionDto, GroupMember, GroupMembershipRoster, GroupRole,
    HlcTimestamp,
};
use ring::aead::{
    Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, UnboundKey, CHACHA20_POLY1305,
};
use ring::rand::SecureRandom;

use crate::crypto::cert::TlsIdentity;
use crate::crypto::e2ee::{
    open_envelope, seal_envelope, sign_with_identity, verify_signature_with_cert, E2eeError,
};

/// Default maximum allowable clock skew in milliseconds (60 seconds).
pub const DEFAULT_MAX_CLOCK_SKEW_MS: u64 = 60_000;

/// Errors arising in decentralized group chat, HLC tracking, DAG verification, or key distribution.
#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    #[error("Remote clock skew exceeded: remote={remote_ms}ms, local={local_ms}ms, max_skew={max_skew_ms}ms")]
    ClockSkewExceeded {
        remote_ms: u64,
        local_ms: u64,
        max_skew_ms: u64,
    },

    #[error("Sender {0} is not an authorized member of group roster")]
    NotAMember(String),

    #[error("Content-addressed node ID mismatch: expected {expected}, actual {actual}")]
    InvalidNodeId { expected: String, actual: String },

    #[error("Digital signature verification failed: {0}")]
    InvalidSignature(String),

    #[error("Epoch key error: {0}")]
    KeyError(String),

    #[error("Message payload decryption failed: {0}")]
    DecryptionFailed(String),

    #[error("Membership roster error: {0}")]
    RosterError(String),

    #[error("Group ID mismatch: node specifies {node_group_id}, roster specifies {roster_group_id}")]
    GroupIdMismatch {
        node_group_id: String,
        roster_group_id: String,
    },

    #[error("E2EE envelope error: {0}")]
    E2ee(#[from] E2eeError),

    #[error("Base64 decode error: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),
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

/// Hybrid Logical Clock (HLC) ensuring causal monotonicity and bounded clock skew.
///
/// Implements the HLC algorithm by Kulkarni et al., combining physical wall-clock
/// milliseconds with a logical counter for causality tracking, tie-broken by
/// the canonical uppercase node fingerprint.
#[derive(Clone, Debug)]
pub struct HybridLogicalClock {
    wall_time_ms: u64,
    counter: u32,
    node_fp: String,
    max_clock_skew_ms: u64,
}

impl HybridLogicalClock {
    /// Creates a new `HybridLogicalClock` with counter 0.
    pub fn new(node_fp: impl Into<String>) -> Self {
        let node_fp_upper = node_fp.into().to_ascii_uppercase();
        Self {
            wall_time_ms: 0,
            counter: 0,
            node_fp: node_fp_upper,
            max_clock_skew_ms: DEFAULT_MAX_CLOCK_SKEW_MS,
        }
    }

    /// Sets the maximum allowable clock skew bound in milliseconds.
    pub fn with_max_skew(mut self, max_skew_ms: u64) -> Self {
        self.max_clock_skew_ms = max_skew_ms;
        self
    }

    /// Returns the canonical fingerprint bound to this clock.
    pub fn node_fp(&self) -> &str {
        &self.node_fp
    }

    /// Returns the latest timestamp recorded by this clock without advancing.
    pub fn current_timestamp(&self) -> HlcTimestamp {
        HlcTimestamp::new(self.wall_time_ms, self.counter, &self.node_fp)
    }

    /// Advances the clock for a newly authored local event using real physical time.
    pub fn now(&mut self) -> HlcTimestamp {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        self.now_with_physical_time(now_ms)
    }

    /// Advances the clock for a newly authored local event given an explicit physical time.
    pub fn now_with_physical_time(&mut self, now_ms: u64) -> HlcTimestamp {
        if now_ms > self.wall_time_ms {
            self.wall_time_ms = now_ms;
            self.counter = 0;
        } else {
            // Physical clock stalled or moved backwards; increment logical counter
            self.counter = self.counter.saturating_add(1);
        }
        HlcTimestamp::new(self.wall_time_ms, self.counter, &self.node_fp)
    }

    /// Updates the local clock upon receiving a remote event timestamp, bounded by clock skew.
    pub fn update(&mut self, remote: &HlcTimestamp) -> Result<HlcTimestamp, GroupError> {
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        self.update_with_physical_time(remote, now_ms)
    }

    /// Updates the local clock upon receiving a remote timestamp given an explicit physical time.
    pub fn update_with_physical_time(
        &mut self,
        remote: &HlcTimestamp,
        now_ms: u64,
    ) -> Result<HlcTimestamp, GroupError> {
        // Enforce clock skew bound: reject timestamps unreasonably far in the future
        if remote.wall_time_ms > now_ms.saturating_add(self.max_clock_skew_ms) {
            return Err(GroupError::ClockSkewExceeded {
                remote_ms: remote.wall_time_ms,
                local_ms: now_ms,
                max_skew_ms: self.max_clock_skew_ms,
            });
        }

        let next_wall = now_ms.max(self.wall_time_ms.max(remote.wall_time_ms));
        let next_counter = if next_wall == self.wall_time_ms && next_wall == remote.wall_time_ms {
            self.counter.max(remote.counter).saturating_add(1)
        } else if next_wall == self.wall_time_ms {
            self.counter.saturating_add(1)
        } else if next_wall == remote.wall_time_ms {
            remote.counter.saturating_add(1)
        } else {
            0
        };

        self.wall_time_ms = next_wall;
        self.counter = next_counter;

        Ok(HlcTimestamp::new(
            self.wall_time_ms,
            self.counter,
            &self.node_fp,
        ))
    }
}

/// Symmetric 32-byte group epoch key used for ChaCha20-Poly1305 encryption of group messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupEpochKey {
    /// Group identifier.
    pub group_id: String,
    /// Epoch sequence number.
    pub epoch: u64,
    /// 32-byte symmetric secret key.
    pub key_bytes: [u8; 32],
}

impl GroupEpochKey {
    /// Generates a fresh random 32-byte symmetric key for the specified group and epoch.
    pub fn generate(group_id: impl Into<String>, epoch: u64) -> Self {
        let mut key_bytes = [0u8; 32];
        let sys_rng = ring::rand::SystemRandom::new();
        sys_rng
            .fill(&mut key_bytes)
            .expect("System RNG failure while generating GroupEpochKey");
        Self {
            group_id: group_id.into(),
            epoch,
            key_bytes,
        }
    }

    /// Constructs an epoch key from raw bytes.
    pub fn from_raw_bytes(group_id: impl Into<String>, epoch: u64, raw: [u8; 32]) -> Self {
        Self {
            group_id: group_id.into(),
            epoch,
            key_bytes: raw,
        }
    }

    /// Encrypts plaintext bytes using ChaCha20-Poly1305 with AAD binding group ID, epoch, and author.
    ///
    /// Returns `(nonce_base64, ciphertext_base64)`.
    pub fn encrypt(
        &self,
        author_fp: &str,
        plaintext: &[u8],
    ) -> Result<(String, String), GroupError> {
        let mut nonce_bytes = [0u8; 12];
        let sys_rng = ring::rand::SystemRandom::new();
        sys_rng
            .fill(&mut nonce_bytes)
            .map_err(|_| GroupError::KeyError("RNG failure generating nonce".into()))?;

        let aad_str = format!("{}:{}:{}", self.group_id, self.epoch, author_fp.to_ascii_uppercase());
        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.key_bytes)
            .map_err(|_| GroupError::KeyError("Invalid ChaCha20 key".into()))?;
        let mut sealing_key = SealingKey::new(unbound, SingleNonce(Some(nonce_bytes)));

        let mut ciphertext_buf = plaintext.to_vec();
        sealing_key
            .seal_in_place_append_tag(Aad::from(aad_str.as_bytes()), &mut ciphertext_buf)
            .map_err(|_| GroupError::KeyError("ChaCha20 seal failed".into()))?;

        Ok((
            STANDARD.encode(nonce_bytes),
            STANDARD.encode(&ciphertext_buf),
        ))
    }

    /// Decrypts ciphertext bytes using ChaCha20-Poly1305 with the same AAD binding.
    pub fn decrypt(
        &self,
        author_fp: &str,
        nonce_base64: &str,
        ciphertext_base64: &str,
    ) -> Result<Vec<u8>, GroupError> {
        let nonce_bytes = STANDARD.decode(nonce_base64)?;
        if nonce_bytes.len() != 12 {
            return Err(GroupError::DecryptionFailed(format!(
                "Invalid nonce length: expected 12, got {}",
                nonce_bytes.len()
            )));
        }
        let mut nonce_arr = [0u8; 12];
        nonce_arr.copy_from_slice(&nonce_bytes);

        let mut ciphertext_buf = STANDARD.decode(ciphertext_base64)?;
        let aad_str = format!("{}:{}:{}", self.group_id, self.epoch, author_fp.to_ascii_uppercase());

        let unbound = UnboundKey::new(&CHACHA20_POLY1305, &self.key_bytes)
            .map_err(|_| GroupError::KeyError("Invalid ChaCha20 key".into()))?;
        let mut opening_key = OpeningKey::new(unbound, SingleNonce(Some(nonce_arr)));

        let decrypted_slice = opening_key
            .open_in_place(Aad::from(aad_str.as_bytes()), &mut ciphertext_buf)
            .map_err(|_| GroupError::DecryptionFailed("AEAD authentication/decryption failed".into()))?;

        Ok(decrypted_slice.to_vec())
    }

    /// Seals this epoch key into an E2EE distribution envelope addressed to `member`.
    pub fn distribute_to_member(
        &self,
        sender_identity: &TlsIdentity,
        member: &GroupMember,
    ) -> Result<GroupKeyDistributionDto, GroupError> {
        let envelope = seal_envelope(
            sender_identity,
            &member.fingerprint,
            &member.cert_pem,
            &self.key_bytes,
        )?;

        Ok(GroupKeyDistributionDto {
            group_id: self.group_id.clone(),
            epoch: self.epoch,
            recipient_fp: member.fingerprint.to_ascii_uppercase(),
            envelope,
        })
    }

    /// Unseals a received distribution envelope using the recipient's identity.
    pub fn from_distribution(
        recipient_identity: &TlsIdentity,
        dist: &GroupKeyDistributionDto,
    ) -> Result<Self, GroupError> {
        Self::from_distribution_with_cert(recipient_identity, None, dist)
    }

    /// Unseals a received distribution envelope, optionally verifying sender signature against explicit certificate PEM.
    pub fn from_distribution_with_cert(
        recipient_identity: &TlsIdentity,
        sender_cert_pem: Option<&str>,
        dist: &GroupKeyDistributionDto,
    ) -> Result<Self, GroupError> {
        let key_bytes_vec = open_envelope(recipient_identity, sender_cert_pem, &dist.envelope)?;
        if key_bytes_vec.len() != 32 {
            return Err(GroupError::KeyError(format!(
                "Decrypted epoch key length mismatch: expected 32, got {}",
                key_bytes_vec.len()
            )));
        }
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(&key_bytes_vec);

        Ok(Self {
            group_id: dist.group_id.clone(),
            epoch: dist.epoch,
            key_bytes,
        })
    }
}

/// Computes the content-addressed message ID:
/// `SHA256(group_id || epoch || author_fp || hlc || sorted(parent_ids) || nonce || ciphertext)`.
pub fn compute_dag_msg_id(
    group_id: &str,
    epoch: u64,
    author_fp: &str,
    hlc: &HlcTimestamp,
    parent_ids: &[String],
    nonce: &str,
    ciphertext: &str,
) -> String {
    let mut sorted_parents = parent_ids.to_vec();
    sorted_parents.sort();
    let header = format!(
        "{}:{}:{}:{}:{}:{}:{}",
        group_id,
        epoch,
        author_fp.to_ascii_uppercase(),
        hlc.wall_time_ms,
        hlc.counter,
        hlc.node_fp.to_ascii_uppercase(),
        sorted_parents.join(",")
    );
    let raw = format!("{}:{}:{}", header, nonce, ciphertext);
    let digest = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes());
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

/// Authors and signs a new `GroupDagNodeDto` from a group member.
pub fn author_group_message(
    author_identity: &TlsIdentity,
    state: &mut GroupDagState,
    plaintext: &[u8],
) -> Result<GroupDagNodeDto, GroupError> {
    let author_fp = author_identity.fingerprint.to_ascii_uppercase();
    if !state.roster().is_member(&author_fp) {
        return Err(GroupError::NotAMember(author_fp));
    }

    let epoch = state.current_epoch();
    let epoch_key = state
        .get_epoch_key(epoch)
        .ok_or_else(|| GroupError::KeyError(format!("Missing epoch key for epoch {epoch}")))?
        .clone();

    // Select up to 4 parent hashes from current DAG frontier heads
    let parent_ids = state.select_parents_for_new_message(4);

    // Advance local HLC
    let hlc = state.hlc().lock().unwrap().now();

    // Encrypt payload via ChaCha20-Poly1305
    let (nonce, ciphertext) = epoch_key.encrypt(&author_fp, plaintext)?;

    // Compute content-addressed message ID
    let msg_id = compute_dag_msg_id(
        &state.roster().group_id,
        epoch,
        &author_fp,
        &hlc,
        &parent_ids,
        &nonce,
        &ciphertext,
    );

    let mut node = GroupDagNodeDto {
        msg_id,
        group_id: state.roster().group_id.clone(),
        epoch,
        author_fp,
        hlc,
        parent_ids,
        nonce,
        ciphertext,
        signature: None,
    };

    // Digitally sign canonical bytes
    let canonical_bytes = node.canonical_signing_bytes();
    let sig_bytes = sign_with_identity(&author_identity.key_pem, &canonical_bytes)?;
    node.signature = Some(STANDARD.encode(&sig_bytes));

    // Register node into local DAG state
    state.add_node(node.clone())?;

    Ok(node)
}

/// Represents the in-memory state of a group's Causal Merkle-DAG.
#[derive(Clone, Debug)]
pub struct GroupDagState {
    roster: GroupMembershipRoster,
    epoch_keys: HashMap<u64, GroupEpochKey>,
    current_epoch: u64,
    hlc: Arc<Mutex<HybridLogicalClock>>,
    nodes: HashMap<String, GroupDagNodeDto>,
    heads: HashSet<String>,
    children: HashMap<String, Vec<String>>,
    parents: HashMap<String, Vec<String>>,
}

impl GroupDagState {
    /// Creates a new empty `GroupDagState` with the given roster and local node fingerprint.
    pub fn new(roster: GroupMembershipRoster, local_fp: impl Into<String>) -> Self {
        let current_epoch = roster.epoch;
        Self {
            roster,
            epoch_keys: HashMap::new(),
            current_epoch,
            hlc: Arc::new(Mutex::new(HybridLogicalClock::new(local_fp))),
            nodes: HashMap::new(),
            heads: HashSet::new(),
            children: HashMap::new(),
            parents: HashMap::new(),
        }
    }

    /// Initializes a new group with the creator as Admin, generates the genesis epoch key,
    /// and pre-computes pairwise distribution envelopes for all other initial members.
    pub fn create_group(
        creator_identity: &TlsIdentity,
        group_name: &str,
        other_members: Vec<GroupMember>,
    ) -> Result<(Self, GroupEpochKey, Vec<GroupKeyDistributionDto>), GroupError> {
        let group_id = uuid::Uuid::new_v4().to_string();
        let creator_fp = creator_identity.fingerprint.to_ascii_uppercase();
        let created_at_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;

        let admin_member = GroupMember {
            fingerprint: creator_fp.clone(),
            alias: "Admin".to_string(),
            cert_pem: creator_identity.cert_pem.clone(),
            role: GroupRole::Admin,
            joined_at_ms: created_at_ms,
        };

        let mut all_members = vec![admin_member];
        all_members.extend(other_members);

        let mut roster = GroupMembershipRoster {
            group_id: group_id.clone(),
            group_name: group_name.to_string(),
            epoch: 1,
            creator_fp: creator_fp.clone(),
            created_at_ms,
            members: all_members.clone(),
            signature: None,
        };

        // Sign roster with creator identity
        let canonical_bytes = roster.canonical_signing_bytes();
        let sig_bytes = sign_with_identity(&creator_identity.key_pem, &canonical_bytes)?;
        roster.signature = Some(STANDARD.encode(&sig_bytes));

        // Generate epoch 1 key
        let epoch_key = GroupEpochKey::generate(&group_id, 1);

        // Pre-compute pairwise key distribution envelopes for non-creator members
        let mut distributions = Vec::new();
        for member in &all_members {
            if !member.fingerprint.eq_ignore_ascii_case(&creator_fp) {
                let dist = epoch_key.distribute_to_member(creator_identity, member)?;
                distributions.push(dist);
            }
        }

        let mut state = Self::new(roster, creator_fp);
        state.set_epoch_key(epoch_key.clone());

        Ok((state, epoch_key, distributions))
    }

    /// Returns the active group membership roster.
    pub fn roster(&self) -> &GroupMembershipRoster {
        &self.roster
    }

    /// Returns a reference to the shared HLC instance.
    pub fn hlc(&self) -> &Arc<Mutex<HybridLogicalClock>> {
        &self.hlc
    }

    /// Returns the current epoch sequence number.
    pub fn current_epoch(&self) -> u64 {
        self.current_epoch
    }

    /// Stores a `GroupEpochKey` for message decryption/authoring.
    pub fn set_epoch_key(&mut self, key: GroupEpochKey) {
        if key.epoch > self.current_epoch {
            self.current_epoch = key.epoch;
        }
        self.epoch_keys.insert(key.epoch, key);
    }

    /// Retrieves an epoch key by epoch sequence number.
    pub fn get_epoch_key(&self, epoch: u64) -> Option<&GroupEpochKey> {
        self.epoch_keys.get(&epoch)
    }

    /// Returns the current DAG frontier heads (sorted deterministically).
    pub fn heads(&self) -> Vec<String> {
        let mut list: Vec<String> = self.heads.iter().cloned().collect();
        list.sort();
        list
    }

    /// Selects up to `max_parents` frontier heads for referencing as parents in a new message.
    pub fn select_parents_for_new_message(&self, max_parents: usize) -> Vec<String> {
        let mut heads = self.heads();
        if heads.len() > max_parents {
            heads.truncate(max_parents);
        }
        heads
    }

    /// Retrieves a message node by ID.
    pub fn get_node(&self, msg_id: &str) -> Option<&GroupDagNodeDto> {
        self.nodes.get(msg_id)
    }

    /// Total number of message nodes currently stored in the DAG.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Checks if the DAG contains no messages.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Ingests a new message node into the DAG, performing strict verification:
    /// 1. Idempotent deduplication.
    /// 2. Group ID matching.
    /// 3. Author membership validation.
    /// 4. Content-addressed `msg_id` verification (`SHA256(header || parents || payload)`).
    /// 5. Digital signature authentication against author's certificate in the roster.
    /// 6. HLC causality update.
    /// 7. DAG adjacency and frontier head updates.
    ///
    /// Returns `Ok(true)` if newly added, or `Ok(false)` if already present.
    pub fn add_node(&mut self, node: GroupDagNodeDto) -> Result<bool, GroupError> {
        // 1. Idempotent deduplication
        if self.nodes.contains_key(&node.msg_id) {
            return Ok(false);
        }

        // 2. Group ID validation
        if node.group_id != self.roster.group_id {
            return Err(GroupError::GroupIdMismatch {
                node_group_id: node.group_id,
                roster_group_id: self.roster.group_id.clone(),
            });
        }

        // 3. Author membership validation
        let member = self
            .roster
            .get_member(&node.author_fp)
            .ok_or_else(|| GroupError::NotAMember(node.author_fp.clone()))?;

        // 4. Content-addressing verification
        let expected_msg_id = compute_dag_msg_id(
            &node.group_id,
            node.epoch,
            &node.author_fp,
            &node.hlc,
            &node.parent_ids,
            &node.nonce,
            &node.ciphertext,
        );

        if !node.msg_id.eq_ignore_ascii_case(&expected_msg_id) {
            return Err(GroupError::InvalidNodeId {
                expected: expected_msg_id,
                actual: node.msg_id,
            });
        }

        // 5. Digital signature verification
        let sig_str = node
            .signature
            .as_deref()
            .ok_or_else(|| GroupError::InvalidSignature("Missing signature on group message".into()))?;
        let sig_bytes = STANDARD.decode(sig_str)?;
        let canonical_bytes = node.canonical_signing_bytes();
        verify_signature_with_cert(&member.cert_pem, &canonical_bytes, &sig_bytes)
            .map_err(|e| GroupError::InvalidSignature(format!("{e}")))?;

        // 6. HLC causality update
        self.hlc.lock().unwrap().update(&node.hlc)?;

        // 7. Update DAG topology
        let msg_id = node.msg_id.clone();
        for parent_id in &node.parent_ids {
            self.children
                .entry(parent_id.clone())
                .or_default()
                .push(msg_id.clone());
            self.parents
                .entry(msg_id.clone())
                .or_default()
                .push(parent_id.clone());
            // Parent now has a child; remove from frontier heads
            self.heads.remove(parent_id);
        }

        // Newly added node is now a frontier head
        self.heads.insert(msg_id.clone());
        self.nodes.insert(msg_id, node);

        Ok(true)
    }

    /// Ingests a collection of DAG nodes in batch (e.g. during sync).
    ///
    /// Returns the number of newly inserted nodes.
    pub fn merge_dag(&mut self, nodes: Vec<GroupDagNodeDto>) -> Result<usize, GroupError> {
        let mut count = 0;
        for node in nodes {
            if self.add_node(node)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Decrypts the payload of a group message node using the appropriate epoch key.
    pub fn decrypt_node_payload(&self, node: &GroupDagNodeDto) -> Result<Vec<u8>, GroupError> {
        let epoch_key = self
            .get_epoch_key(node.epoch)
            .ok_or_else(|| GroupError::KeyError(format!("Missing epoch key for epoch {}", node.epoch)))?;
        epoch_key.decrypt(&node.author_fp, &node.nonce, &node.ciphertext)
    }

    /// Computes a conflict-free, deterministic topological sort order of all message IDs in the DAG.
    ///
    /// Invariants:
    /// - Causality: Any parent message $P$ is guaranteed to precede all its children $C$.
    /// - Determinism: Concurrent branches are ordered strictly by `HlcTimestamp`
    ///   (`wall_time_ms` -> `counter` -> `node_fp`), guaranteeing that all nodes yield
    ///   the exact same message sequence regardless of insertion order or network arrival jitter.
    pub fn topological_sort(&self) -> Vec<String> {
        if self.nodes.is_empty() {
            return Vec::new();
        }

        // 1. Calculate in-degrees (number of parents known in this graph)
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for (id, node) in &self.nodes {
            let count = node
                .parent_ids
                .iter()
                .filter(|p| self.nodes.contains_key(p.as_str()))
                .count();
            in_degree.insert(id.as_str(), count);
        }

        // 2. Deterministic Min-Heap prioritized by HlcTimestamp
        #[derive(Eq, PartialEq)]
        struct QueueItem<'a> {
            hlc: &'a HlcTimestamp,
            msg_id: &'a str,
        }

        impl<'a> Ord for QueueItem<'a> {
            fn cmp(&self, other: &Self) -> std::cmp::Ordering {
                self.hlc
                    .cmp(other.hlc)
                    .then_with(|| self.msg_id.cmp(other.msg_id))
            }
        }

        impl<'a> PartialOrd for QueueItem<'a> {
            fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        let mut heap: BinaryHeap<Reverse<QueueItem>> = BinaryHeap::new();
        for (id, deg) in &in_degree {
            if *deg == 0 {
                let node = &self.nodes[*id];
                heap.push(Reverse(QueueItem {
                    hlc: &node.hlc,
                    msg_id: id,
                }));
            }
        }

        // 3. Process nodes via Kahn's algorithm
        let mut sorted: Vec<String> = Vec::with_capacity(self.nodes.len());
        while let Some(Reverse(item)) = heap.pop() {
            sorted.push(item.msg_id.to_string());
            if let Some(children) = self.children.get(item.msg_id) {
                for child_id in children {
                    if let Some(deg) = in_degree.get_mut(child_id.as_str()) {
                        *deg = deg.saturating_sub(1);
                        if *deg == 0 {
                            let child_node = &self.nodes[child_id.as_str()];
                            heap.push(Reverse(QueueItem {
                                hlc: &child_node.hlc,
                                msg_id: child_id.as_str(),
                            }));
                        }
                    }
                }
            }
        }

        // 4. Fallback for disconnected components or cycle defense
        if sorted.len() < self.nodes.len() {
            let mut remaining: Vec<&GroupDagNodeDto> = self
                .nodes
                .values()
                .filter(|n| !sorted.contains(&n.msg_id))
                .collect();
            remaining.sort_by(|a, b| a.hlc.cmp(&b.hlc).then_with(|| a.msg_id.cmp(&b.msg_id)));
            for n in remaining {
                sorted.push(n.msg_id.clone());
            }
        }

        sorted
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::crypto::cert::generate_self_signed;

    #[test]
    pub fn test_hlc_monotonicity_and_clock_skew_bound() {
        let node_fp = "AAAA111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0001";
        let mut hlc = HybridLogicalClock::new(node_fp).with_max_skew(5000);

        // 1. Monotonicity under same physical millisecond: counter increments
        let t1 = hlc.now_with_physical_time(1000);
        let t2 = hlc.now_with_physical_time(1000);
        let t3 = hlc.now_with_physical_time(1000);
        assert_eq!(t1.wall_time_ms, 1000);
        assert_eq!(t1.counter, 0);
        assert_eq!(t2.wall_time_ms, 1000);
        assert_eq!(t2.counter, 1);
        assert_eq!(t3.wall_time_ms, 1000);
        assert_eq!(t3.counter, 2);
        assert!(t1 < t2);
        assert!(t2 < t3);

        // 2. Physical clock advances: counter resets to 0
        let t4 = hlc.now_with_physical_time(1500);
        assert_eq!(t4.wall_time_ms, 1500);
        assert_eq!(t4.counter, 0);
        assert!(t3 < t4);

        // 3. Physical clock moves backwards: wall time preserved, counter increments
        let t5 = hlc.now_with_physical_time(1200);
        assert_eq!(t5.wall_time_ms, 1500);
        assert_eq!(t5.counter, 1);
        assert!(t4 < t5);

        // 4. Remote causality update within skew: advances wall time
        let remote_fp = "BBBB111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0002";
        let remote_t = HlcTimestamp::new(2000, 5, remote_fp);
        let updated = hlc.update_with_physical_time(&remote_t, 1600).expect("valid update within skew");
        assert_eq!(updated.wall_time_ms, 2000);
        assert_eq!(updated.counter, 6);
        assert!(t5 < updated);

        // 5. Remote timestamp exceeding max clock skew (5000ms): rejected
        let far_future_t = HlcTimestamp::new(10000, 0, remote_fp);
        let err = hlc.update_with_physical_time(&far_future_t, 1600).unwrap_err();
        match err {
            GroupError::ClockSkewExceeded { remote_ms, local_ms, max_skew_ms } => {
                assert_eq!(remote_ms, 10000);
                assert_eq!(local_ms, 1600);
                assert_eq!(max_skew_ms, 5000);
            }
            other => panic!("Unexpected error: {other:?}"),
        }
    }

    #[test]
    pub fn test_causal_dag_topological_merge_determinism() {
        let alice_id = generate_self_signed().unwrap();
        let bob_id = generate_self_signed().unwrap();
        let charlie_id = generate_self_signed().unwrap();

        let bob_member = GroupMember {
            fingerprint: bob_id.fingerprint.clone(),
            alias: "Bob".to_string(),
            cert_pem: bob_id.cert_pem.clone(),
            role: GroupRole::Member,
            joined_at_ms: 1000,
        };

        let charlie_member = GroupMember {
            fingerprint: charlie_id.fingerprint.clone(),
            alias: "Charlie".to_string(),
            cert_pem: charlie_id.cert_pem.clone(),
            role: GroupRole::Member,
            joined_at_ms: 1000,
        };

        // 1. Alice creates group and shares key with Bob and Charlie
        let (mut alice_state, epoch_key, distributions) = GroupDagState::create_group(
            &alice_id,
            "Engineering Mesh",
            vec![bob_member.clone(), charlie_member.clone()],
        )
        .unwrap();

        // 2. Initialize Bob and Charlie states with roster and decrypted epoch key
        let mut bob_state = GroupDagState::new(alice_state.roster().clone(), &bob_id.fingerprint);
        let bob_dist = distributions.iter().find(|d| d.recipient_fp == bob_id.fingerprint).unwrap();
        let bob_key = GroupEpochKey::from_distribution(&bob_id, bob_dist).unwrap();
        bob_state.set_epoch_key(bob_key);

        let mut charlie_state = GroupDagState::new(alice_state.roster().clone(), &charlie_id.fingerprint);
        let charlie_dist = distributions.iter().find(|d| d.recipient_fp == charlie_id.fingerprint).unwrap();
        let charlie_key = GroupEpochKey::from_distribution(&charlie_id, charlie_dist).unwrap();
        charlie_state.set_epoch_key(charlie_key);

        // 3. Genesis message authored by Alice
        let genesis_msg = author_group_message(&alice_id, &mut alice_state, b"Genesis Message").unwrap();
        assert!(genesis_msg.parent_ids.is_empty());

        // Sync genesis to Bob and Charlie
        bob_state.add_node(genesis_msg.clone()).unwrap();
        charlie_state.add_node(genesis_msg.clone()).unwrap();

        // 4. Network Partition: Alice, Bob, and Charlie author concurrent messages offline
        // All 3 messages reference genesis as parent
        let alice_concurrent = author_group_message(&alice_id, &mut alice_state, b"Alice branch 1").unwrap();
        let bob_concurrent = author_group_message(&bob_id, &mut bob_state, b"Bob branch 1").unwrap();
        let charlie_concurrent = author_group_message(&charlie_id, &mut charlie_state, b"Charlie branch 1").unwrap();

        assert_eq!(alice_concurrent.parent_ids, vec![genesis_msg.msg_id.clone()]);
        assert_eq!(bob_concurrent.parent_ids, vec![genesis_msg.msg_id.clone()]);
        assert_eq!(charlie_concurrent.parent_ids, vec![genesis_msg.msg_id.clone()]);

        // Alice authors follow-up message on her own branch
        let alice_followup = author_group_message(&alice_id, &mut alice_state, b"Alice branch 2").unwrap();
        assert_eq!(alice_followup.parent_ids, vec![alice_concurrent.msg_id.clone()]);

        // 5. Test determinism: merge all messages in 3 completely different shuffled orders
        // Node 1 receives: [Genesis, Alice1, Bob1, Charlie1, Alice2]
        let mut node1_state = GroupDagState::new(alice_state.roster().clone(), &alice_id.fingerprint);
        node1_state.set_epoch_key(epoch_key.clone());
        node1_state.merge_dag(vec![
            genesis_msg.clone(),
            alice_concurrent.clone(),
            bob_concurrent.clone(),
            charlie_concurrent.clone(),
            alice_followup.clone(),
        ]).unwrap();

        // Node 2 receives: [Charlie1, Alice2, Genesis, Bob1, Alice1]
        let mut node2_state = GroupDagState::new(alice_state.roster().clone(), &bob_id.fingerprint);
        node2_state.set_epoch_key(epoch_key.clone());
        node2_state.merge_dag(vec![
            charlie_concurrent.clone(),
            alice_followup.clone(),
            genesis_msg.clone(),
            bob_concurrent.clone(),
            alice_concurrent.clone(),
        ]).unwrap();

        // Node 3 receives: [Bob1, Alice2, Charlie1, Alice1, Genesis]
        let mut node3_state = GroupDagState::new(alice_state.roster().clone(), &charlie_id.fingerprint);
        node3_state.set_epoch_key(epoch_key.clone());
        node3_state.merge_dag(vec![
            bob_concurrent.clone(),
            alice_followup.clone(),
            charlie_concurrent.clone(),
            alice_concurrent.clone(),
            genesis_msg.clone(),
        ]).unwrap();

        let order1 = node1_state.topological_sort();
        let order2 = node2_state.topological_sort();
        let order3 = node3_state.topological_sort();

        // All 3 nodes produce the EXACT SAME deterministic sequence
        assert_eq!(order1, order2);
        assert_eq!(order2, order3);
        assert_eq!(order1.len(), 5);

        // Causality assertions:
        // Genesis must be first
        assert_eq!(order1[0], genesis_msg.msg_id);
        // Alice branch 1 must precede Alice branch 2
        let pos_alice1 = order1.iter().position(|id| id == &alice_concurrent.msg_id).unwrap();
        let pos_alice2 = order1.iter().position(|id| id == &alice_followup.msg_id).unwrap();
        assert!(pos_alice1 < pos_alice2);
    }

    #[test]
    pub fn test_group_key_sealing_and_non_member_rejection() {
        let alice_id = generate_self_signed().unwrap();
        let bob_id = generate_self_signed().unwrap();
        let eve_id = generate_self_signed().unwrap(); // Non-member

        let bob_member = GroupMember {
            fingerprint: bob_id.fingerprint.clone(),
            alias: "Bob".to_string(),
            cert_pem: bob_id.cert_pem.clone(),
            role: GroupRole::Member,
            joined_at_ms: 1000,
        };

        // 1. Group creation with Alice and Bob
        let (mut alice_state, epoch_key, distributions) = GroupDagState::create_group(
            &alice_id,
            "Secret Squad",
            vec![bob_member.clone()],
        )
        .unwrap();

        // 2. Bob opens distribution envelope and decrypts epoch key
        let bob_dist = distributions.iter().find(|d| d.recipient_fp == bob_id.fingerprint).unwrap();
        let bob_key = GroupEpochKey::from_distribution(&bob_id, bob_dist).expect("Bob decapsulates key");
        assert_eq!(bob_key.key_bytes, epoch_key.key_bytes);

        // 3. Eve (non-member) attempts to decapsulate Bob's key envelope -> fails
        let eve_err = GroupEpochKey::from_distribution(&eve_id, bob_dist).unwrap_err();
        match eve_err {
            GroupError::E2ee(E2eeError::RecipientMismatch { .. }) => {}
            other => panic!("Expected RecipientMismatch error, got: {other:?}"),
        }

        // 4. Bob authors a valid encrypted group message
        let mut bob_state = GroupDagState::new(alice_state.roster().clone(), &bob_id.fingerprint);
        bob_state.set_epoch_key(bob_key);
        let bob_msg = author_group_message(&bob_id, &mut bob_state, b"Hello Secret Squad!").unwrap();

        // Alice receives and verifies Bob's message
        assert!(alice_state.add_node(bob_msg.clone()).unwrap());
        let decrypted_by_alice = alice_state.decrypt_node_payload(&bob_msg).unwrap();
        assert_eq!(decrypted_by_alice, b"Hello Secret Squad!");

        // 5. Eve attempts to author a message in the group -> rejected as not a member
        let mut eve_state = GroupDagState::new(alice_state.roster().clone(), &eve_id.fingerprint);
        let eve_dummy_key = GroupEpochKey::generate(&alice_state.roster().group_id, 1);
        eve_state.set_epoch_key(eve_dummy_key);
        let author_err = author_group_message(&eve_id, &mut eve_state, b"Eve intruder").unwrap_err();
        match author_err {
            GroupError::NotAMember(fp) => assert_eq!(fp, eve_id.fingerprint),
            other => panic!("Expected NotAMember error, got: {other:?}"),
        }

        // 6. Eve forges a node claiming to be Bob but with invalid signature -> rejected by Alice
        let mut forged_node = bob_msg.clone();
        forged_node.ciphertext = STANDARD.encode(b"Forged text altered payload");
        // Recompute content-addressed msg_id to pass ID check
        forged_node.msg_id = compute_dag_msg_id(
            &forged_node.group_id,
            forged_node.epoch,
            &forged_node.author_fp,
            &forged_node.hlc,
            &forged_node.parent_ids,
            &forged_node.nonce,
            &forged_node.ciphertext,
        );
        // Signature no longer matches payload
        let add_err = alice_state.add_node(forged_node).unwrap_err();
        match add_err {
            GroupError::InvalidSignature(_) => {}
            other => panic!("Expected InvalidSignature error, got: {other:?}"),
        }
    }
}
