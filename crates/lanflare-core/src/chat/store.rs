//! Persistent contact book, conversation history, and message delivery state store.
//!
//! Sub-phase 8.2 implementation:
//! - Contacts table persisting `fingerprint`, `alias`, `trust_state`, `avatar_thumbnail`, and `last_seen_ms`
//!   to disk under `<data_dir>/chat/contacts.json`.
//! - Conversations and message thread store persisting message histories and delivery states
//!   (`Queued`, `Sending`, `Relayed`, `Delivered`, `Read`, `Failed`) keyed by canonical `fingerprint`.
//! - Atomic disk writes (`.tmp` -> rename) ensuring crash-safe persistence.
//! - In-memory index with thread-safe `RwLock` for instantaneous queries and reloading recovery.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use lanflare_proto::chat::{
    ChatMessageDto, DeliveryReceiptDto, MessageDeliveryState, ReadReceiptDto,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::chat::trust::ContactTrustState;

/// Errors arising from chat store operations and disk persistence.
#[derive(Debug, Error)]
pub enum ChatStoreError {
    #[error("I/O error during chat store operation: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization/deserialization failure: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Message not found with id: {0}")]
    MessageNotFound(String),

    #[error("Conversation not found with id: {0}")]
    ConversationNotFound(String),

    #[error("Contact not found with fingerprint: {0}")]
    ContactNotFound(String),
}

/// Persisted contact descriptor in the local LanFlare contact book.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    /// Canonical 64-character uppercase SHA-256 fingerprint of peer TLS certificate.
    pub fingerprint: String,

    /// Human-readable device/user alias.
    pub alias: String,

    /// Trust level in the 5-layer anti-harassment gate.
    pub trust_state: ContactTrustState,

    /// Optional Base64 or URI avatar thumbnail image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_thumbnail: Option<String>,

    /// Timestamp in milliseconds since UNIX epoch when peer was last active.
    pub last_seen_ms: u64,

    /// Timestamp in milliseconds since UNIX epoch when contact was added/created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at_ms: Option<u64>,

    /// Optional user-authored notes or custom metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

impl Contact {
    /// Creates a new `Contact` descriptor.
    pub fn new(
        fingerprint: impl Into<String>,
        alias: impl Into<String>,
        trust_state: ContactTrustState,
        last_seen_ms: u64,
    ) -> Self {
        let canonical_fp = fingerprint.into().to_ascii_uppercase();
        Self {
            fingerprint: canonical_fp,
            alias: alias.into(),
            trust_state,
            avatar_thumbnail: None,
            last_seen_ms,
            added_at_ms: Some(last_seen_ms),
            notes: None,
        }
    }
}

/// Metadata header describing a 1:1 or group conversation thread.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Conversation {
    /// Unique conversation identifier (e.g. peer fingerprint or group UUID).
    pub id: String,

    /// Canonical uppercase SHA-256 fingerprint of the remote peer (or group creator).
    pub peer_fingerprint: String,

    /// Timestamp in milliseconds since UNIX epoch when conversation was established.
    pub created_at_ms: u64,

    /// Timestamp in milliseconds since UNIX epoch of the latest message in this thread.
    pub last_message_at_ms: u64,

    /// Number of unread incoming messages in this conversation.
    pub unread_count: usize,

    /// Short snippet or preview of the latest message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_preview: Option<String>,

    /// True if this conversation is a multi-peer group chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_group: Option<bool>,
}

/// Stored chat message entry with delivery lifecycle tracking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    /// Full message DTO.
    pub message: ChatMessageDto,

    /// Current delivery progression state (`Queued`, `Sending`, `Relayed`, `Delivered`, `Read`, `Failed`).
    pub delivery_state: MessageDeliveryState,

    /// True if this message was sent by the local node; false if received from peer.
    pub is_outgoing: bool,

    /// Timestamp in milliseconds when delivery was acknowledged by recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at_ms: Option<u64>,

    /// Timestamp in milliseconds when message was read/opened by recipient.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_at_ms: Option<u64>,

    /// Optional failure reason or diagnostic error string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// In-memory cache of contacts, conversations, messages, and fast index.
#[derive(Default)]
struct ChatStoreState {
    contacts: HashMap<String, Contact>,
    conversations: HashMap<String, Conversation>,
    messages: HashMap<String, Vec<StoredMessage>>,
    message_index: HashMap<String, (String, usize)>, // message_id -> (conversation_id, index)
}

/// Persistent contact book, conversation history, and message delivery state store.
pub struct ChatStore {
    chat_dir: PathBuf,
    contacts_path: PathBuf,
    conversations_path: PathBuf,
    messages_dir: PathBuf,
    state: RwLock<ChatStoreState>,
}

impl ChatStore {
    /// Opens or initializes a persistent `ChatStore` at the specified directory.
    ///
    /// Reads existing `contacts.json`, `conversations.json`, and `messages/*.json` from disk.
    /// If files or directories do not exist, they are created automatically.
    pub async fn open(chat_dir: impl AsRef<Path>) -> Result<Arc<Self>, ChatStoreError> {
        let dir = chat_dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;

        let messages_dir = dir.join("messages");
        tokio::fs::create_dir_all(&messages_dir).await?;

        let contacts_path = dir.join("contacts.json");
        let conversations_path = dir.join("conversations.json");

        let mut initial_state = ChatStoreState::default();

        // 1. Load contacts if contacts.json exists
        if let Ok(bytes) = tokio::fs::read(&contacts_path).await {
            let contacts_list = serde_json::from_slice::<Vec<Contact>>(&bytes).unwrap_or_default();
            for c in contacts_list {
                initial_state
                    .contacts
                    .insert(c.fingerprint.to_ascii_uppercase(), c);
            }
        }

        // 2. Load conversations if conversations.json exists
        if let Ok(bytes) = tokio::fs::read(&conversations_path).await {
            let conv_list = serde_json::from_slice::<Vec<Conversation>>(&bytes).unwrap_or_default();
            for conv in conv_list {
                initial_state.conversations.insert(conv.id.clone(), conv);
            }
        }

        // 3. Load message threads from messages/ directory
        if let Ok(mut entries) = tokio::fs::read_dir(&messages_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                let is_json = path.extension().and_then(|ext| ext.to_str()) == Some("json");
                if !is_json {
                    continue;
                }
                let Ok(bytes) = tokio::fs::read(&path).await else {
                    continue;
                };
                let Ok(thread) = serde_json::from_slice::<Vec<StoredMessage>>(&bytes) else {
                    continue;
                };
                if let Some(first) = thread.first() {
                    let conv_id = first.message.conversation_id.clone();
                    for (idx, msg) in thread.iter().enumerate() {
                        initial_state
                            .message_index
                            .insert(msg.message.id.clone(), (conv_id.clone(), idx));
                    }
                    initial_state.messages.insert(conv_id, thread);
                }
            }
        }

        Ok(Arc::new(Self {
            chat_dir: dir,
            contacts_path,
            conversations_path,
            messages_dir,
            state: RwLock::new(initial_state),
        }))
    }

    /// Returns the root filesystem path of this chat store.
    pub fn chat_dir(&self) -> &Path {
        &self.chat_dir
    }

    // --- Contact Book CRUD ---

    /// Inserts or updates a contact record in the contact book and commits to `contacts.json`.
    pub async fn upsert_contact(&self, mut contact: Contact) -> Result<(), ChatStoreError> {
        contact.fingerprint = contact.fingerprint.to_ascii_uppercase();
        let mut state = self.state.write().await;
        state.contacts.insert(contact.fingerprint.clone(), contact);
        let contacts_vec: Vec<Contact> = state.contacts.values().cloned().collect();
        drop(state);

        self.persist_contacts(&contacts_vec).await
    }

    /// Retrieves a cloned contact record by canonical fingerprint.
    pub async fn get_contact(&self, fingerprint: &str) -> Option<Contact> {
        let key = fingerprint.to_ascii_uppercase();
        let state = self.state.read().await;
        state.contacts.get(&key).cloned()
    }

    /// Returns all contacts registered in the store.
    pub async fn list_contacts(&self) -> Vec<Contact> {
        let state = self.state.read().await;
        let mut list: Vec<Contact> = state.contacts.values().cloned().collect();
        list.sort_by(|a, b| a.alias.cmp(&b.alias));
        list
    }

    /// Removes a contact by fingerprint. Returns true if removed, false if not found.
    pub async fn delete_contact(&self, fingerprint: &str) -> Result<bool, ChatStoreError> {
        let key = fingerprint.to_ascii_uppercase();
        let mut state = self.state.write().await;
        let removed = state.contacts.remove(&key).is_some();
        if removed {
            let contacts_vec: Vec<Contact> = state.contacts.values().cloned().collect();
            drop(state);
            self.persist_contacts(&contacts_vec).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Updates the trust state for a contact.
    pub async fn update_trust_state(
        &self,
        fingerprint: &str,
        trust_state: ContactTrustState,
    ) -> Result<bool, ChatStoreError> {
        let key = fingerprint.to_ascii_uppercase();
        let mut state = self.state.write().await;
        if let Some(contact) = state.contacts.get_mut(&key) {
            contact.trust_state = trust_state;
            let contacts_vec: Vec<Contact> = state.contacts.values().cloned().collect();
            drop(state);
            self.persist_contacts(&contacts_vec).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Updates the `last_seen_ms` timestamp for a contact.
    pub async fn update_contact_last_seen(
        &self,
        fingerprint: &str,
        last_seen_ms: u64,
    ) -> Result<bool, ChatStoreError> {
        let key = fingerprint.to_ascii_uppercase();
        let mut state = self.state.write().await;
        if let Some(contact) = state.contacts.get_mut(&key) {
            contact.last_seen_ms = last_seen_ms;
            let contacts_vec: Vec<Contact> = state.contacts.values().cloned().collect();
            drop(state);
            self.persist_contacts(&contacts_vec).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // --- Message & Conversation Thread Store ---

    /// Inserts a new chat message into conversation thread history and commits to disk.
    ///
    /// Automatically initializes the `Conversation` header if not yet present.
    pub async fn insert_message(
        &self,
        message: ChatMessageDto,
        delivery_state: MessageDeliveryState,
        is_outgoing: bool,
    ) -> Result<StoredMessage, ChatStoreError> {
        let conv_id = message.conversation_id.clone();
        let msg_id = message.id.clone();
        let preview = message.content.text_content().map(|s| {
            if s.len() > 60 {
                format!("{}...", &s[..60])
            } else {
                s.to_string()
            }
        }).or_else(|| {
            message.content.file_name().map(|f| format!("[Attachment: {f}]"))
        });

        let stored = StoredMessage {
            message,
            delivery_state,
            is_outgoing,
            delivered_at_ms: if delivery_state == MessageDeliveryState::Delivered
                || delivery_state == MessageDeliveryState::Read
            {
                Some(chrono::Utc::now().timestamp_millis() as u64)
            } else {
                None
            },
            read_at_ms: if delivery_state == MessageDeliveryState::Read {
                Some(chrono::Utc::now().timestamp_millis() as u64)
            } else {
                None
            },
            error: None,
        };

        let mut state = self.state.write().await;

        // 1. Update or create Conversation record
        let peer_fp = if is_outgoing {
            stored.message.recipient_fp.to_ascii_uppercase()
        } else {
            stored.message.sender_fp.to_ascii_uppercase()
        };

        let conv = state.conversations.entry(conv_id.clone()).or_insert_with(|| {
            Conversation {
                id: conv_id.clone(),
                peer_fingerprint: peer_fp,
                created_at_ms: stored.message.timestamp_ms,
                last_message_at_ms: stored.message.timestamp_ms,
                unread_count: 0,
                last_message_preview: None,
                is_group: None,
            }
        });

        conv.last_message_at_ms = conv.last_message_at_ms.max(stored.message.timestamp_ms);
        conv.last_message_preview = preview;
        if !is_outgoing && stored.delivery_state != MessageDeliveryState::Read {
            conv.unread_count += 1;
        }

        // 2. Append to message thread
        let (idx, thread_snapshot) = {
            let thread = state.messages.entry(conv_id.clone()).or_default();
            let idx = thread.len();
            thread.push(stored.clone());
            (idx, thread.clone())
        };
        state.message_index.insert(msg_id, (conv_id.clone(), idx));

        let conv_snapshot: Vec<Conversation> = state.conversations.values().cloned().collect();
        drop(state);

        // 3. Persist to disk atomically
        self.persist_conversation_thread(&conv_id, &thread_snapshot).await?;
        self.persist_conversations(&conv_snapshot).await?;

        Ok(stored)
    }

    /// Retrieves a stored message by its unique message ID.
    pub async fn get_message(&self, message_id: &str) -> Option<StoredMessage> {
        let state = self.state.read().await;
        if let Some((conv_id, idx)) = state.message_index.get(message_id) {
            state.messages.get(conv_id).and_then(|thread| thread.get(*idx).cloned())
        } else {
            None
        }
    }

    /// Retrieves conversation metadata by its conversation ID.
    pub async fn get_conversation(&self, conversation_id: &str) -> Option<Conversation> {
        let state = self.state.read().await;
        state.conversations.get(conversation_id).cloned()
    }

    /// Lists all conversations sorted by `last_message_at_ms` descending.
    pub async fn list_conversations(&self) -> Vec<Conversation> {
        let state = self.state.read().await;
        let mut list: Vec<Conversation> = state.conversations.values().cloned().collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.last_message_at_ms));
        list
    }

    /// Retrieves messages for a conversation with optional pagination.
    pub async fn get_conversation_messages(
        &self,
        conversation_id: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Vec<StoredMessage> {
        let state = self.state.read().await;
        if let Some(thread) = state.messages.get(conversation_id) {
            let offset_val = offset.unwrap_or(0);
            let limit_val = limit.unwrap_or(thread.len());
            thread.iter().skip(offset_val).take(limit_val).cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// Updates the delivery progression state of a specific message.
    pub async fn update_message_delivery_state(
        &self,
        message_id: &str,
        state: MessageDeliveryState,
    ) -> Result<bool, ChatStoreError> {
        let mut store_state = self.state.write().await;
        let Some((conv_id, idx)) = store_state.message_index.get(message_id).cloned() else {
            return Ok(false);
        };
        let Some(thread) = store_state.messages.get_mut(&conv_id) else {
            return Ok(false);
        };
        let Some(msg) = thread.get_mut(idx) else {
            return Ok(false);
        };
        if msg.delivery_state.apply_monotonic_transition(state) {
            let thread_snapshot = thread.clone();
            drop(store_state);
            self.persist_conversation_thread(&conv_id, &thread_snapshot).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Applies a cryptographically validated delivery receipt, transitioning the target message
    /// monotonically to `MessageDeliveryState::Delivered`.
    ///
    /// Monotonicity guarantee: `Read` messages will not regress to `Delivered`.
    pub async fn apply_delivery_receipt(
        &self,
        receipt: &DeliveryReceiptDto,
    ) -> Result<bool, ChatStoreError> {
        let mut store_state = self.state.write().await;
        let Some((conv_id, idx)) = store_state.message_index.get(&receipt.message_id).cloned() else {
            return Ok(false);
        };
        let Some(thread) = store_state.messages.get_mut(&conv_id) else {
            return Ok(false);
        };
        let Some(msg) = thread.get_mut(idx) else {
            return Ok(false);
        };
        if msg
            .delivery_state
            .apply_monotonic_transition(MessageDeliveryState::Delivered)
        {
            msg.delivered_at_ms = Some(receipt.delivered_at_ms);
            let thread_snapshot = thread.clone();
            drop(store_state);
            self.persist_conversation_thread(&conv_id, &thread_snapshot).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Applies a cryptographically validated read receipt, transitioning the target message
    /// monotonically to `MessageDeliveryState::Read`.
    pub async fn apply_read_receipt(
        &self,
        receipt: &ReadReceiptDto,
    ) -> Result<bool, ChatStoreError> {
        let mut store_state = self.state.write().await;
        let Some((conv_id, idx)) = store_state.message_index.get(&receipt.message_id).cloned() else {
            return Ok(false);
        };
        let Some(thread) = store_state.messages.get_mut(&conv_id) else {
            return Ok(false);
        };
        let Some(msg) = thread.get_mut(idx) else {
            return Ok(false);
        };
        if msg
            .delivery_state
            .apply_monotonic_transition(MessageDeliveryState::Read)
        {
            if msg.delivered_at_ms.is_none() {
                msg.delivered_at_ms = Some(receipt.read_at_ms);
            }
            msg.read_at_ms = Some(receipt.read_at_ms);
            let thread_snapshot = thread.clone();
            drop(store_state);
            self.persist_conversation_thread(&conv_id, &thread_snapshot).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Marks all unread incoming messages in a conversation as read and resets `unread_count = 0`.
    pub async fn mark_conversation_as_read(
        &self,
        conversation_id: &str,
    ) -> Result<usize, ChatStoreError> {
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;
        let mut store_state = self.state.write().await;
        let mut modified = 0;

        if let Some(conv) = store_state.conversations.get_mut(conversation_id) {
            conv.unread_count = 0;
        }

        let thread_snapshot = if let Some(thread) = store_state.messages.get_mut(conversation_id) {
            for msg in thread.iter_mut() {
                if !msg.is_outgoing && msg.delivery_state != MessageDeliveryState::Read {
                    msg.delivery_state = MessageDeliveryState::Read;
                    msg.read_at_ms = Some(now_ms);
                    modified += 1;
                }
            }
            Some(thread.clone())
        } else {
            None
        };

        let conv_snapshot: Vec<Conversation> = store_state.conversations.values().cloned().collect();
        drop(store_state);

        if let Some(snapshot) = thread_snapshot {
            self.persist_conversation_thread(conversation_id, &snapshot).await?;
        }
        self.persist_conversations(&conv_snapshot).await?;

        Ok(modified)
    }

    /// Deletes a specific message from thread history.
    pub async fn delete_message(&self, message_id: &str) -> Result<bool, ChatStoreError> {
        let mut store_state = self.state.write().await;
        if let Some((conv_id, _)) = store_state.message_index.remove(message_id) {
            let thread_snapshot = if let Some(thread) = store_state.messages.get_mut(&conv_id) {
                thread.retain(|m| m.message.id != message_id);
                Some(thread.clone())
            } else {
                None
            };

            if let Some(snapshot) = thread_snapshot {
                // Re-index remaining messages
                for (new_idx, m) in snapshot.iter().enumerate() {
                    store_state
                        .message_index
                        .insert(m.message.id.clone(), (conv_id.clone(), new_idx));
                }
                drop(store_state);
                self.persist_conversation_thread(&conv_id, &snapshot).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Deletes an entire conversation thread and unlinks its message file from disk.
    pub async fn delete_conversation(&self, conversation_id: &str) -> Result<bool, ChatStoreError> {
        let mut store_state = self.state.write().await;
        let removed_conv = store_state.conversations.remove(conversation_id).is_some();
        if let Some(thread) = store_state.messages.remove(conversation_id) {
            for m in thread {
                store_state.message_index.remove(&m.message.id);
            }
        }

        if removed_conv {
            let conv_snapshot: Vec<Conversation> = store_state.conversations.values().cloned().collect();
            drop(store_state);

            let thread_path = self.thread_path(conversation_id);
            if thread_path.exists() {
                let _ = tokio::fs::remove_file(thread_path).await;
            }
            self.persist_conversations(&conv_snapshot).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Returns pending outgoing messages (`delivery_state == Queued || delivery_state == Sending`),
    /// optionally filtered by recipient fingerprint.
    pub async fn get_pending_outgoing_messages(
        &self,
        recipient_fp: Option<&str>,
    ) -> Vec<StoredMessage> {
        let canonical_target = recipient_fp.map(|fp| fp.to_ascii_uppercase());
        let store_state = self.state.read().await;
        let mut pending = Vec::new();

        for thread in store_state.messages.values() {
            for msg in thread {
                if msg.is_outgoing
                    && (msg.delivery_state == MessageDeliveryState::Queued
                        || msg.delivery_state == MessageDeliveryState::Sending)
                {
                    if let Some(ref target) = canonical_target {
                        if msg.message.recipient_fp.to_ascii_uppercase() == *target {
                            pending.push(msg.clone());
                        }
                    } else {
                        pending.push(msg.clone());
                    }
                }
            }
        }

        pending.sort_by_key(|m| m.message.timestamp_ms);
        pending
    }

    // --- Atomic Disk Persistence Helpers ---

    fn thread_path(&self, conversation_id: &str) -> PathBuf {
        let safe_name: String = conversation_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        self.messages_dir.join(format!("{safe_name}.json"))
    }

    async fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), ChatStoreError> {
        let json = serde_json::to_vec_pretty(value)?;
        let tmp_path = path.with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, path).await?;
        Ok(())
    }

    async fn persist_contacts(&self, contacts: &[Contact]) -> Result<(), ChatStoreError> {
        Self::write_atomic_json(&self.contacts_path, &contacts).await
    }

    async fn persist_conversations(&self, convs: &[Conversation]) -> Result<(), ChatStoreError> {
        Self::write_atomic_json(&self.conversations_path, &convs).await
    }

    async fn persist_conversation_thread(
        &self,
        conversation_id: &str,
        thread: &[StoredMessage],
    ) -> Result<(), ChatStoreError> {
        let path = self.thread_path(conversation_id);
        Self::write_atomic_json(&path, &thread).await
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use lanflare_proto::chat::ChatContentDto;

    #[tokio::test]
    pub async fn test_persistent_store_crud_and_reload_recovery() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_chat_store_{}", uuid::Uuid::new_v4()));
        let chat_dir = temp_dir.join("chat");

        let alice_fp = "AAAA111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0001";
        let bob_fp = "BBBB111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0002";

        // 1. Initialize store and verify empty state
        {
            let store = ChatStore::open(&chat_dir).await.expect("open store");
            assert!(store.list_contacts().await.is_empty());
            assert!(store.list_conversations().await.is_empty());

            // 2. Contact CRUD operations
            let contact_alice = Contact::new(alice_fp, "Alice", ContactTrustState::TrustedContact, 1700000000000);
            let mut contact_bob = Contact::new(bob_fp, "Bob", ContactTrustState::PendingApproval, 1700000001000);
            contact_bob.avatar_thumbnail = Some("data:image/png;base64,bob_avatar".to_string());

            store.upsert_contact(contact_alice.clone()).await.expect("upsert alice");
            store.upsert_contact(contact_bob.clone()).await.expect("upsert bob");

            assert_eq!(store.list_contacts().await.len(), 2);
            let fetched_alice = store.get_contact(alice_fp).await.expect("get alice");
            assert_eq!(fetched_alice.alias, "Alice");
            assert_eq!(fetched_alice.trust_state, ContactTrustState::TrustedContact);

            // Update trust state & last seen
            assert!(store.update_trust_state(bob_fp, ContactTrustState::TrustedContact).await.unwrap());
            assert!(store.update_contact_last_seen(bob_fp, 1700000005000).await.unwrap());

            let updated_bob = store.get_contact(bob_fp).await.expect("get bob");
            assert_eq!(updated_bob.trust_state, ContactTrustState::TrustedContact);
            assert_eq!(updated_bob.last_seen_ms, 1700000005000);

            // 3. Conversation & Message Thread insertions
            let msg1 = ChatMessageDto {
                id: "msg-001".to_string(),
                conversation_id: alice_fp.to_string(),
                sender_fp: bob_fp.to_string(),
                recipient_fp: alice_fp.to_string(),
                timestamp_ms: 1700000010000,
                content: ChatContentDto::Text {
                    text: "Hello Alice, this is Bob!".to_string(),
                },
                reply_to_id: None,
                signature: None,
            };

            let stored1 = store
                .insert_message(msg1.clone(), MessageDeliveryState::Queued, true)
                .await
                .expect("insert msg1");
            assert_eq!(stored1.delivery_state, MessageDeliveryState::Queued);
            assert!(stored1.is_outgoing);

            let msg2 = ChatMessageDto {
                id: "msg-002".to_string(),
                conversation_id: alice_fp.to_string(),
                sender_fp: alice_fp.to_string(),
                recipient_fp: bob_fp.to_string(),
                timestamp_ms: 1700000020000,
                content: ChatContentDto::Image {
                    file_name: "screenshot.png".to_string(),
                    size_bytes: 409600,
                    mime_type: "image/png".to_string(),
                    sha256: "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855".to_string(),
                    thumbnail_base64: None,
                },
                reply_to_id: Some("msg-001".to_string()),
                signature: None,
            };

            let stored2 = store
                .insert_message(msg2.clone(), MessageDeliveryState::Delivered, false)
                .await
                .expect("insert msg2");
            assert_eq!(stored2.delivery_state, MessageDeliveryState::Delivered);
            assert!(!stored2.is_outgoing);

            // Verify conversation metadata
            let conv = store.get_conversation(alice_fp).await.expect("get conversation");
            assert_eq!(conv.unread_count, 1);
            assert_eq!(conv.last_message_at_ms, 1700000020000);

            // Apply delivery receipt to msg1
            let receipt = DeliveryReceiptDto {
                receipt_id: "rcpt-001".to_string(),
                message_id: "msg-001".to_string(),
                conversation_id: alice_fp.to_string(),
                sender_fp: bob_fp.to_string(),
                recipient_fp: alice_fp.to_string(),
                delivered_at_ms: 1700000015000,
                signature: None,
            };
            assert!(store.apply_delivery_receipt(&receipt).await.unwrap());

            let updated_msg1 = store.get_message("msg-001").await.expect("get msg1");
            assert_eq!(updated_msg1.delivery_state, MessageDeliveryState::Delivered);
            assert_eq!(updated_msg1.delivered_at_ms, Some(1700000015000));

            // Mark conversation as read
            let marked = store.mark_conversation_as_read(alice_fp).await.unwrap();
            assert_eq!(marked, 1);
            let conv_after_read = store.get_conversation(alice_fp).await.unwrap();
            assert_eq!(conv_after_read.unread_count, 0);

            // Verify files exist on disk
            assert!(chat_dir.join("contacts.json").exists());
            assert!(chat_dir.join("conversations.json").exists());
            assert!(chat_dir.join("messages").join(format!("{alice_fp}.json")).exists());
        }

        // 4. Reload recovery: instantiate a new ChatStore pointing to the same directory
        {
            let reloaded_store = ChatStore::open(&chat_dir).await.expect("reload store");

            // Verify contacts recovered
            let contacts = reloaded_store.list_contacts().await;
            assert_eq!(contacts.len(), 2);
            let bob = reloaded_store.get_contact(bob_fp).await.expect("reloaded bob");
            assert_eq!(bob.trust_state, ContactTrustState::TrustedContact);
            assert_eq!(bob.last_seen_ms, 1700000005000);
            assert_eq!(bob.avatar_thumbnail.as_deref(), Some("data:image/png;base64,bob_avatar"));

            // Verify conversations recovered
            let convs = reloaded_store.list_conversations().await;
            assert_eq!(convs.len(), 1);
            assert_eq!(convs[0].id, alice_fp);
            assert_eq!(convs[0].unread_count, 0);

            // Verify message thread recovered
            let msgs = reloaded_store.get_conversation_messages(alice_fp, None, None).await;
            assert_eq!(msgs.len(), 2);
            assert_eq!(msgs[0].message.id, "msg-001");
            assert_eq!(msgs[0].delivery_state, MessageDeliveryState::Delivered);
            assert_eq!(msgs[0].delivered_at_ms, Some(1700000015000));
            assert_eq!(msgs[1].message.id, "msg-002");
            assert_eq!(msgs[1].delivery_state, MessageDeliveryState::Read);

            // Test deletion
            assert!(reloaded_store.delete_message("msg-001").await.unwrap());
            assert_eq!(reloaded_store.get_conversation_messages(alice_fp, None, None).await.len(), 1);
            assert!(reloaded_store.delete_conversation(alice_fp).await.unwrap());
            assert!(reloaded_store.list_conversations().await.is_empty());
            assert!(reloaded_store.delete_contact(bob_fp).await.unwrap());
            assert_eq!(reloaded_store.list_contacts().await.len(), 1);
        }

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
