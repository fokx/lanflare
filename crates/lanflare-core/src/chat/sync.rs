//! Anti-entropy DAG frontier synchronization engine for group chat catch-up.
//!
//! Sub-phase 9.2 implementation:
//! - Implements the DAG frontier anti-entropy sync protocol.
//! - Reconciles missing Causal Merkle-DAG nodes and pairwise epoch keys between peers.
//! - Provides bidirectional HTTP and multi-hop mesh synchronization routines.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use hyper::StatusCode;
use lanflare_proto::chat::{
    GroupDagNodeDto, GroupSyncRequestDto, GroupSyncResponseDto,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::RwLock;

use crate::chat::group::{GroupDagState, GroupError};
use crate::crypto::cert::TlsIdentity;

/// Errors arising during anti-entropy group frontier synchronization.
#[derive(Debug, Error)]
pub enum SyncError {
    #[error("Group not found: {0}")]
    GroupNotFound(String),

    #[error("Requester {0} is not an authorized member of group roster")]
    NotAMember(String),

    #[error("Group DAG error: {0}")]
    Group(#[from] GroupError),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("HTTP status {status}: {message}")]
    HttpStatus {
        status: StatusCode,
        message: String,
    },

    #[error("Timeout waiting for mesh sync response")]
    Timeout,
}

impl SyncError {
    /// Maps this sync error to an appropriate HTTP status code.
    pub fn status_code(&self) -> StatusCode {
        match self {
            SyncError::GroupNotFound(_) => StatusCode::NOT_FOUND,
            SyncError::NotAMember(_) => StatusCode::FORBIDDEN,
            SyncError::Group(GroupError::ClockSkewExceeded { .. })
            | SyncError::Group(GroupError::InvalidSignature(_))
            | SyncError::Group(GroupError::InvalidNodeId { .. }) => StatusCode::UNPROCESSABLE_ENTITY,
            SyncError::Group(GroupError::NotAMember(_)) => StatusCode::FORBIDDEN,
            SyncError::Serde(_) => StatusCode::BAD_REQUEST,
            SyncError::HttpStatus { status, .. } => *status,
            SyncError::Timeout => StatusCode::GATEWAY_TIMEOUT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Statistics returned after an anti-entropy synchronization cycle completes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStats {
    /// Number of missing DAG nodes received from remote peer and merged into local state.
    pub nodes_pulled: usize,

    /// Number of local DAG nodes successfully pushed to remote peer.
    pub nodes_pushed: usize,

    /// Number of new epoch keys decapsulated and added.
    pub epoch_keys_added: usize,
}

/// Anti-entropy DAG synchronization engine managing multi-group causal catch-up.
pub struct GroupSyncEngine {
    identity: TlsIdentity,
    groups: Arc<RwLock<HashMap<String, Arc<RwLock<GroupDagState>>>>>,
    http_client: reqwest::Client,
}

impl GroupSyncEngine {
    /// Creates a new `GroupSyncEngine` for the given local identity.
    pub fn new(identity: TlsIdentity) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_default();

        Self {
            identity,
            groups: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    /// Creates an engine with a custom `reqwest::Client`.
    pub fn with_http_client(identity: TlsIdentity, http_client: reqwest::Client) -> Self {
        Self {
            identity,
            groups: Arc::new(RwLock::new(HashMap::new())),
            http_client,
        }
    }

    /// Returns a reference to the local TLS identity.
    pub fn identity(&self) -> &TlsIdentity {
        &self.identity
    }

    /// Registers a group's DAG state with the engine.
    pub async fn register_group(&self, state: GroupDagState) -> Arc<RwLock<GroupDagState>> {
        let group_id = state.roster().group_id.clone();
        let group_arc = Arc::new(RwLock::new(state));
        let mut groups = self.groups.write().await;
        groups.insert(group_id, group_arc.clone());
        group_arc
    }

    /// Retrieves the thread-safe DAG state for a group by ID.
    pub async fn get_group(&self, group_id: &str) -> Option<Arc<RwLock<GroupDagState>>> {
        let groups = self.groups.read().await;
        groups.get(group_id).cloned()
    }

    /// Lists all registered group IDs.
    pub async fn list_groups(&self) -> Vec<String> {
        let groups = self.groups.read().await;
        groups.keys().cloned().collect()
    }

    /// Handles an incoming anti-entropy `GroupSyncRequestDto`, computing missing DAG nodes
    /// and epoch keys to return in a `GroupSyncResponseDto`.
    pub async fn handle_sync_request(
        &self,
        req: GroupSyncRequestDto,
    ) -> Result<GroupSyncResponseDto, SyncError> {
        let group_arc = self
            .get_group(&req.group_id)
            .await
            .ok_or_else(|| SyncError::GroupNotFound(req.group_id.clone()))?;

        let mut state = group_arc.write().await;

        // 1. Validate that the requester is an authorized member in the group roster
        let requester_fp = req.requester_fp.to_ascii_uppercase();
        if !state.roster().is_member(&requester_fp) {
            return Err(SyncError::NotAMember(requester_fp));
        }

        // 2. Ingest pushed nodes (if remote peer sent newly authored nodes)
        if !req.push_nodes.is_empty() {
            state.merge_dag(req.push_nodes)?;
        }

        // 3. Compute missing nodes for the requester
        let missing_nodes = state.find_missing_nodes(&req.frontier_heads);

        // 4. Gather key distributions for epochs newer than requester's epoch
        let epoch_keys = state.key_distributions_for(&requester_fp, req.current_epoch);

        // 5. Build response
        let frontier_heads = state.heads();
        let current_epoch = state.current_epoch();

        Ok(GroupSyncResponseDto {
            group_id: req.group_id,
            responder_fp: self.identity.fingerprint.clone(),
            current_epoch,
            frontier_heads,
            missing_nodes,
            epoch_keys,
            signature: None,
        })
    }

    /// Applies a received `GroupSyncResponseDto`, decapsulating any epoch keys and merging
    /// missing DAG nodes into the local state.
    pub async fn apply_sync_response(
        &self,
        resp: GroupSyncResponseDto,
    ) -> Result<SyncStats, SyncError> {
        let group_arc = self
            .get_group(&resp.group_id)
            .await
            .ok_or_else(|| SyncError::GroupNotFound(resp.group_id.clone()))?;

        let mut state = group_arc.write().await;
        let mut epoch_keys_added = 0;

        // 1. Process epoch key distributions addressed to this node
        for dist in &resp.epoch_keys {
            if dist.recipient_fp.eq_ignore_ascii_case(&self.identity.fingerprint) {
                if state.apply_epoch_key_distribution(&self.identity, dist)? {
                    epoch_keys_added += 1;
                }
            } else {
                // Store relayable key distribution for other members
                state.add_key_distribution(dist.clone());
            }
        }

        // 2. Ingest missing DAG nodes in causal topological order
        let nodes_pulled = state.merge_dag(resp.missing_nodes)?;

        Ok(SyncStats {
            nodes_pulled,
            nodes_pushed: 0,
            epoch_keys_added,
        })
    }

    /// Executes an HTTP anti-entropy synchronization exchange against a remote peer endpoint.
    pub async fn sync_with_peer_http(
        &self,
        peer_base_url: &str,
        group_id: &str,
    ) -> Result<SyncStats, SyncError> {
        self.sync_with_peer_http_with_push(peer_base_url, group_id, Vec::new())
            .await
    }

    /// Executes an HTTP anti-entropy synchronization exchange while pushing local nodes.
    pub async fn sync_with_peer_http_with_push(
        &self,
        peer_base_url: &str,
        group_id: &str,
        push_nodes: Vec<GroupDagNodeDto>,
    ) -> Result<SyncStats, SyncError> {
        let group_arc = self
            .get_group(group_id)
            .await
            .ok_or_else(|| SyncError::GroupNotFound(group_id.to_string()))?;

        let (frontier_heads, current_epoch) = {
            let state = group_arc.read().await;
            (state.heads(), state.highest_possessed_epoch())
        };

        let nodes_pushed_count = push_nodes.len();
        let sync_req = GroupSyncRequestDto {
            group_id: group_id.to_string(),
            requester_fp: self.identity.fingerprint.clone(),
            current_epoch,
            frontier_heads,
            push_nodes,
            signature: None,
        };

        let url = format!("{}/api/lanflare/v1/chat/group-sync", peer_base_url.trim_end_matches('/'));
        let resp = self
            .http_client
            .post(&url)
            .json(&sync_req)
            .send()
            .await
            .map_err(|e| SyncError::Network(format!("HTTP request failed: {e}")))?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            return Err(SyncError::HttpStatus {
                status,
                message: err_text,
            });
        }

        let sync_resp: GroupSyncResponseDto = resp
            .json()
            .await
            .map_err(|e| SyncError::Network(format!("Failed to parse sync response: {e}")))?;

        let mut stats = self.apply_sync_response(sync_resp).await?;
        stats.nodes_pushed = nodes_pushed_count;

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::group::author_group_message;
    use crate::crypto::cert::generate_self_signed;
    use lanflare_proto::chat::{GroupMember, GroupRole};

    #[tokio::test]
    async fn test_group_sync_handle_request_and_apply_response() {
        let alice_id = generate_self_signed().unwrap();
        let bob_id = generate_self_signed().unwrap();

        let engine_alice = GroupSyncEngine::new(alice_id.clone());
        let engine_bob = GroupSyncEngine::new(bob_id.clone());

        // 1. Alice creates group with Bob
        let bob_member = GroupMember {
            fingerprint: bob_id.fingerprint.clone(),
            alias: "Bob".to_string(),
            cert_pem: bob_id.cert_pem.clone(),
            role: GroupRole::Member,
            joined_at_ms: 1000,
        };

        let (alice_dag, _epoch_key, _dists) =
            GroupDagState::create_group(&alice_id, "Secret Squad", vec![bob_member.clone()]).unwrap();
        let group_id = alice_dag.roster().group_id.clone();
        let roster = alice_dag.roster().clone();

        let alice_group = engine_alice.register_group(alice_dag).await;

        // Bob initializes empty group with roster (has no epoch key and no messages yet)
        let bob_dag = GroupDagState::new(roster, bob_id.fingerprint.clone());
        let bob_group = engine_bob.register_group(bob_dag).await;

        // 2. Alice authors 3 sequential and branching messages
        {
            let mut state = alice_group.write().await;
            author_group_message(&alice_id, &mut state, b"Msg 1: Genesis").unwrap();
            author_group_message(&alice_id, &mut state, b"Msg 2: Branch").unwrap();
            author_group_message(&alice_id, &mut state, b"Msg 3: Followup").unwrap();
            assert_eq!(state.len(), 3);
        }

        // 3. Bob creates a sync request (frontier heads empty, epoch 0)
        let (bob_heads, bob_epoch) = {
            let state = bob_group.read().await;
            (state.heads(), state.current_epoch())
        };
        assert_eq!(bob_heads.len(), 0);
        assert_eq!(bob_epoch, 1); // Group epoch from roster

        let sync_req = GroupSyncRequestDto {
            group_id: group_id.clone(),
            requester_fp: bob_id.fingerprint.clone(),
            current_epoch: 0, // Bob has not decapsulated epoch 1 yet
            frontier_heads: bob_heads,
            push_nodes: Vec::new(),
            signature: None,
        };

        // 4. Alice handles the sync request
        let sync_resp = engine_alice
            .handle_sync_request(sync_req)
            .await
            .expect("Alice handles Bob's sync request");

        assert_eq!(sync_resp.group_id, group_id);
        assert_eq!(sync_resp.missing_nodes.len(), 3);
        assert_eq!(sync_resp.epoch_keys.len(), 1); // Bob's epoch key included
        assert_eq!(sync_resp.epoch_keys[0].recipient_fp, bob_id.fingerprint);

        // 5. Bob applies the sync response
        let stats = engine_bob
            .apply_sync_response(sync_resp)
            .await
            .expect("Bob applies sync response");

        assert_eq!(stats.nodes_pulled, 3);
        assert_eq!(stats.epoch_keys_added, 1);

        // 6. Verify Bob has all 3 messages and can decrypt them
        {
            let state = bob_group.read().await;
            assert_eq!(state.len(), 3);
            let sorted_ids = state.topological_sort();
            assert_eq!(sorted_ids.len(), 3);

            for (idx, id) in sorted_ids.iter().enumerate() {
                let node = state.get_node(id).unwrap();
                let plaintext = state.decrypt_node_payload(node).unwrap();
                match idx {
                    0 => assert_eq!(plaintext, b"Msg 1: Genesis"),
                    1 => assert_eq!(plaintext, b"Msg 2: Branch"),
                    2 => assert_eq!(plaintext, b"Msg 3: Followup"),
                    _ => unreachable!(),
                }
            }
        }

        // 7. Verify subsequent sync round from Bob yields 0 missing nodes
        let (bob_heads_now, bob_epoch_now) = {
            let state = bob_group.read().await;
            (state.heads(), state.current_epoch())
        };
        assert!(!bob_heads_now.is_empty());

        let sync_req2 = GroupSyncRequestDto {
            group_id: group_id.clone(),
            requester_fp: bob_id.fingerprint.clone(),
            current_epoch: bob_epoch_now,
            frontier_heads: bob_heads_now,
            push_nodes: Vec::new(),
            signature: None,
        };

        let sync_resp2 = engine_alice
            .handle_sync_request(sync_req2)
            .await
            .expect("Alice handles 2nd sync");
        assert_eq!(sync_resp2.missing_nodes.len(), 0);
        assert_eq!(sync_resp2.epoch_keys.len(), 0);
    }

    #[tokio::test]
    async fn test_non_member_and_unknown_group_rejection() {
        let alice_id = generate_self_signed().unwrap();
        let eve_id = generate_self_signed().unwrap();

        let engine = GroupSyncEngine::new(alice_id.clone());

        // Unknown group rejection
        let req_unknown = GroupSyncRequestDto {
            group_id: "unknown-group".to_string(),
            requester_fp: eve_id.fingerprint.clone(),
            current_epoch: 1,
            frontier_heads: Vec::new(),
            push_nodes: Vec::new(),
            signature: None,
        };
        let err = engine.handle_sync_request(req_unknown).await.unwrap_err();
        assert!(matches!(err, SyncError::GroupNotFound(_)));
        assert_eq!(err.status_code(), StatusCode::NOT_FOUND);

        // Register valid group without Eve
        let (dag, _, _) = GroupDagState::create_group(&alice_id, "Admins Only", Vec::new()).unwrap();
        let group_id = dag.roster().group_id.clone();
        engine.register_group(dag).await;

        // Eve (non-member) requests sync
        let req_eve = GroupSyncRequestDto {
            group_id,
            requester_fp: eve_id.fingerprint.clone(),
            current_epoch: 1,
            frontier_heads: Vec::new(),
            push_nodes: Vec::new(),
            signature: None,
        };
        let err2 = engine.handle_sync_request(req_eve).await.unwrap_err();
        assert!(matches!(err2, SyncError::NotAMember(_)));
        assert_eq!(err2.status_code(), StatusCode::FORBIDDEN);
    }
}
