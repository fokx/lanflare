//! Reactive delay-tolerant outbox dispatcher and cryptographic receipt state machine.
//!
//! Sub-phase 8.2 implementation:
//! - Delay-tolerant queue storing pending outgoing messages and receipts when recipients are offline.
//! - Reactive subscriber listening to `PeerRegistry` events (`PeerAdded`, `PeerUpdated`) and
//!   mesh route triggers to automatically flush queued items in causal FIFO order.
//! - Cryptographic receipt processing guaranteeing monotonic state progression
//!   (`Queued -> Sending -> Relayed -> Delivered -> Read`).
//! - Crash-resilient persistence via `<data_dir>/chat/outbox.json`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use lanflare_proto::chat::{
    ChatMessageDto, DeliveryReceiptDto, MessageDeliveryState, ReadReceiptDto,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::chat::store::{ChatStore, ChatStoreError};
use crate::discovery::registry::{PeerRegistry, RegistryEvent};

/// Errors arising from outbox queue management, dispatching, and persistence.
#[derive(Debug, Error)]
pub enum OutboxError {
    #[error("Store error: {0}")]
    Store(#[from] ChatStoreError),

    #[error("I/O error during outbox persistence: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Item not found in outbox: {0}")]
    NotFound(String),
}

/// Errors returned by transport dispatch implementations.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum OutboxDispatchError {
    #[error("Peer unreachable: {0}")]
    PeerUnreachable(String),

    #[error("Network transfer error: {0}")]
    NetworkError(String),

    #[error("Payload rejected by recipient: {0}")]
    Rejected(String),

    #[error("Max delivery attempts exceeded ({0})")]
    MaxAttemptsExceeded(u32),
}

/// Variant of data payload held in the outbox queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OutboxPayload {
    /// Outgoing chat message (text or rich media descriptor).
    Message(ChatMessageDto),
    /// Outgoing cryptographic delivery receipt sent back to message author.
    DeliveryReceipt(DeliveryReceiptDto),
    /// Outgoing cryptographic read receipt sent back to message author.
    ReadReceipt(ReadReceiptDto),
}

impl OutboxPayload {
    /// Returns the unique item identifier for this payload.
    pub fn item_id(&self) -> &str {
        match self {
            Self::Message(m) => &m.id,
            Self::DeliveryReceipt(r) => &r.receipt_id,
            Self::ReadReceipt(r) => &r.receipt_id,
        }
    }

    /// Returns the canonical destination peer fingerprint.
    pub fn target_fingerprint(&self) -> &str {
        match self {
            Self::Message(m) => &m.recipient_fp,
            Self::DeliveryReceipt(r) => &r.sender_fp,
            Self::ReadReceipt(r) => &r.sender_fp,
        }
    }
}

/// Single queued item in the delay-tolerant outbox.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxItem {
    /// Unique item ID.
    pub id: String,

    /// Canonical uppercase SHA-256 fingerprint of destination peer.
    pub recipient_fp: String,

    /// Outbox payload (Message, DeliveryReceipt, or ReadReceipt).
    pub payload: OutboxPayload,

    /// UNIX timestamp in milliseconds when enqueued.
    pub queued_at_ms: u64,

    /// Number of delivery attempts made so far.
    pub attempts: u32,

    /// UNIX timestamp in milliseconds of the latest dispatch attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_ms: Option<u64>,
}

impl OutboxItem {
    /// Creates a new `OutboxItem` wrapping a payload.
    pub fn new(payload: OutboxPayload, queued_at_ms: u64) -> Self {
        let recipient_fp = payload.target_fingerprint().to_ascii_uppercase();
        let id = payload.item_id().to_string();
        Self {
            id,
            recipient_fp,
            payload,
            queued_at_ms,
            attempts: 0,
            last_attempt_ms: None,
        }
    }
}

/// Abstract transport trait for outbox item transmission over 1-hop LAN or multi-hop mesh.
pub trait OutboxTransport: Send + Sync {
    /// Dispatches an outbox item over the network.
    fn dispatch<'a>(
        &'a self,
        item: &'a OutboxItem,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), OutboxDispatchError>> + Send + 'a>>;
}

impl<F, Fut> OutboxTransport for F
where
    F: Fn(&OutboxItem) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = Result<(), OutboxDispatchError>> + Send + 'static,
{
    fn dispatch<'a>(
        &'a self,
        item: &'a OutboxItem,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), OutboxDispatchError>> + Send + 'a>> {
        Box::pin(self(item))
    }
}

/// Configuration parameters for `OutboxDispatcher`.
#[derive(Clone, Debug)]
pub struct OutboxConfig {
    /// Maximum number of delivery retry attempts before marking a message Failed.
    pub max_attempts: u32,
    /// Minimum backoff interval between retries in milliseconds.
    pub retry_backoff_ms: u64,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            retry_backoff_ms: 1000,
        }
    }
}

/// Reactive delay-tolerant outbox dispatcher.
pub struct OutboxDispatcher {
    outbox_path: PathBuf,
    config: OutboxConfig,
    store: Arc<ChatStore>,
    transport: Arc<dyn OutboxTransport>,
    queue: RwLock<VecDeque<OutboxItem>>,
}

impl OutboxDispatcher {
    /// Initializes an `OutboxDispatcher` with persistent disk queue backing.
    pub async fn new(
        chat_dir: impl AsRef<Path>,
        store: Arc<ChatStore>,
        transport: Arc<dyn OutboxTransport>,
        config: OutboxConfig,
    ) -> Result<Arc<Self>, OutboxError> {
        let dir = chat_dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;

        let outbox_path = dir.join("outbox.json");
        let mut initial_items = VecDeque::new();

        if let Ok(bytes) = tokio::fs::read(&outbox_path).await {
            let items = serde_json::from_slice::<Vec<OutboxItem>>(&bytes).unwrap_or_default();
            for item in items {
                initial_items.push_back(item);
            }
        }

        Ok(Arc::new(Self {
            outbox_path,
            config,
            store,
            transport,
            queue: RwLock::new(initial_items),
        }))
    }

    /// Enqueues an outgoing chat message for delay-tolerant delivery.
    ///
    /// Persists the message into `ChatStore` with `MessageDeliveryState::Queued`
    /// and adds an `OutboxItem` to the persistent FIFO queue.
    pub async fn enqueue_message(&self, message: ChatMessageDto) -> Result<String, OutboxError> {
        let msg_id = message.id.clone();
        let timestamp_ms = message.timestamp_ms;

        // 1. Persist to ChatStore
        self.store
            .insert_message(message.clone(), MessageDeliveryState::Queued, true)
            .await?;

        // 2. Add to outbox queue
        let item = OutboxItem::new(OutboxPayload::Message(message), timestamp_ms);
        let mut queue = self.queue.write().await;
        queue.push_back(item);
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        self.persist_queue(&snapshot).await?;
        Ok(msg_id)
    }

    /// Enqueues an outgoing delivery receipt for delay-tolerant delivery to the message author.
    pub async fn enqueue_delivery_receipt(
        &self,
        receipt: DeliveryReceiptDto,
    ) -> Result<String, OutboxError> {
        let receipt_id = receipt.receipt_id.clone();
        let timestamp_ms = receipt.delivered_at_ms;

        let item = OutboxItem::new(OutboxPayload::DeliveryReceipt(receipt), timestamp_ms);
        let mut queue = self.queue.write().await;
        queue.push_back(item);
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        self.persist_queue(&snapshot).await?;
        Ok(receipt_id)
    }

    /// Enqueues an outgoing read receipt for delay-tolerant delivery to the message author.
    pub async fn enqueue_read_receipt(&self, receipt: ReadReceiptDto) -> Result<String, OutboxError> {
        let receipt_id = receipt.receipt_id.clone();
        let timestamp_ms = receipt.read_at_ms;

        let item = OutboxItem::new(OutboxPayload::ReadReceipt(receipt), timestamp_ms);
        let mut queue = self.queue.write().await;
        queue.push_back(item);
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        self.persist_queue(&snapshot).await?;
        Ok(receipt_id)
    }

    /// Flushes all queued items destined for `recipient_fp` in FIFO causal order.
    pub async fn flush_peer(&self, recipient_fp: &str) -> Vec<Result<String, OutboxDispatchError>> {
        let target = recipient_fp.to_ascii_uppercase();
        let now_ms = chrono::Utc::now().timestamp_millis() as u64;

        // 1. Extract items matching recipient_fp in FIFO order
        let mut queue = self.queue.write().await;
        let mut target_items = Vec::new();
        let mut remaining_items = VecDeque::new();

        while let Some(item) = queue.pop_front() {
            if item.recipient_fp == target {
                target_items.push(item);
            } else {
                remaining_items.push_back(item);
            }
        }
        *queue = remaining_items;
        drop(queue);

        let mut results = Vec::new();
        let mut items_to_requeue = Vec::new();

        // 2. Dispatch each item sequentially in causal order
        for mut item in target_items {
            let item_id = item.id.clone();
            item.attempts += 1;
            item.last_attempt_ms = Some(now_ms);

            match self.transport.dispatch(&item).await {
                Ok(()) => {
                    // Success: mark message as Sending if it was a message
                    if let OutboxPayload::Message(_) = &item.payload {
                        let _ = self
                            .store
                            .update_message_delivery_state(&item.id, MessageDeliveryState::Sending)
                            .await;
                    }
                    results.push(Ok(item_id));
                }
                Err(e) => {
                    if item.attempts >= self.config.max_attempts {
                        // Max attempts reached: mark message as Failed
                        if let OutboxPayload::Message(_) = &item.payload {
                            let _ = self
                                .store
                                .update_message_delivery_state(&item.id, MessageDeliveryState::Failed)
                                .await;
                        }
                        results.push(Err(OutboxDispatchError::MaxAttemptsExceeded(item.attempts)));
                    } else {
                        // Keep for retry
                        items_to_requeue.push(item);
                        results.push(Err(e));
                    }
                }
            }
        }

        // 3. Put failed/retryable items back into queue
        let mut queue = self.queue.write().await;
        for item in items_to_requeue {
            queue.push_back(item);
        }
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        let _ = self.persist_queue(&snapshot).await;
        results
    }

    /// Flushes all queued items across all recipients.
    pub async fn flush_all(&self) -> Vec<Result<String, OutboxDispatchError>> {
        let queue = self.queue.read().await;
        let mut peers: Vec<String> = queue.iter().map(|i| i.recipient_fp.clone()).collect();
        drop(queue);

        peers.sort();
        peers.dedup();

        let mut results = Vec::new();
        for peer in peers {
            results.extend(self.flush_peer(&peer).await);
        }
        results
    }

    /// Returns the current number of queued items in the outbox.
    pub async fn queue_len(&self) -> usize {
        let queue = self.queue.read().await;
        queue.len()
    }

    /// Returns all pending queued items for a specific peer.
    pub async fn pending_items_for_peer(&self, recipient_fp: &str) -> Vec<OutboxItem> {
        let target = recipient_fp.to_ascii_uppercase();
        let queue = self.queue.read().await;
        queue.iter().filter(|i| i.recipient_fp == target).cloned().collect()
    }

    /// Processes an incoming delivery receipt from a peer, advancing the message state monotonically.
    pub async fn process_incoming_delivery_receipt(
        &self,
        receipt: &DeliveryReceiptDto,
    ) -> Result<bool, OutboxError> {
        // Remove from outbox queue if still present
        let mut queue = self.queue.write().await;
        let prev_len = queue.len();
        queue.retain(|item| item.id != receipt.message_id);
        let queue_changed = queue.len() != prev_len;
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        if queue_changed {
            self.persist_queue(&snapshot).await?;
        }

        Ok(self.store.apply_delivery_receipt(receipt).await?)
    }

    /// Processes an incoming read receipt from a peer, advancing the message state monotonically.
    pub async fn process_incoming_read_receipt(
        &self,
        receipt: &ReadReceiptDto,
    ) -> Result<bool, OutboxError> {
        // Remove from outbox queue if still present
        let mut queue = self.queue.write().await;
        let prev_len = queue.len();
        queue.retain(|item| item.id != receipt.message_id);
        let queue_changed = queue.len() != prev_len;
        let snapshot: Vec<OutboxItem> = queue.iter().cloned().collect();
        drop(queue);

        if queue_changed {
            self.persist_queue(&snapshot).await?;
        }

        Ok(self.store.apply_read_receipt(receipt).await?)
    }

    /// Starts a reactive listener task that subscribes to `PeerRegistry` events (`PeerAdded`, `PeerUpdated`)
    /// and automatically flushes outbox items for newly discovered or updated online peers.
    pub fn start_reactive_listener(
        self: &Arc<Self>,
        registry: Arc<PeerRegistry>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let dispatcher = Arc::clone(self);
        let mut rx = registry.subscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event_res = rx.recv() => {
                        match event_res {
                            Ok(event) => {
                                match event {
                                    RegistryEvent::PeerAdded(peer)
                                    | RegistryEvent::PeerUpdated { peer, .. } => {
                                        let fp = peer.fingerprint.clone();
                                        let d = Arc::clone(&dispatcher);
                                        tokio::spawn(async move {
                                            d.flush_peer(&fp).await;
                                        });
                                    }
                                    RegistryEvent::PeerExpired { .. }
                                    | RegistryEvent::PeerRemoved { .. } => {}
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        })
    }

    /// Notifies the outbox dispatcher that a route is now available (e.g. via `MeshTopologyGraph`),
    /// triggering an immediate FIFO outbox flush for that peer.
    pub async fn notify_route_available(&self, peer_fp: &str) -> Vec<Result<String, OutboxDispatchError>> {
        self.flush_peer(peer_fp).await
    }

    // --- Persistence Helper ---

    async fn persist_queue(&self, items: &[OutboxItem]) -> Result<(), OutboxError> {
        let json = serde_json::to_vec_pretty(items)?;
        let tmp_path = self
            .outbox_path
            .with_extension(format!("tmp.{}", uuid::Uuid::new_v4()));
        tokio::fs::write(&tmp_path, json).await?;
        tokio::fs::rename(&tmp_path, &self.outbox_path).await?;
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use lanflare_proto::chat::ChatContentDto;
    use lanflare_proto::discovery::ProtocolType;
    use crate::discovery::registry::DiscoveredPeer;
    use std::sync::Mutex;
    use std::time::Duration;

    type DispatchedList = Arc<Mutex<Vec<OutboxItem>>>;
    type ShouldFailFlag = Arc<Mutex<bool>>;

    struct MockTransport {
        dispatched: DispatchedList,
        should_fail: ShouldFailFlag,
    }

    struct MockTransportHandle {
        transport: MockTransport,
        dispatched: DispatchedList,
        #[allow(dead_code)]
        should_fail: ShouldFailFlag,
    }

    impl MockTransportHandle {
        fn new() -> Self {
            let dispatched = Arc::new(Mutex::new(Vec::new()));
            let should_fail = Arc::new(Mutex::new(false));
            Self {
                transport: MockTransport {
                    dispatched: Arc::clone(&dispatched),
                    should_fail: Arc::clone(&should_fail),
                },
                dispatched,
                should_fail,
            }
        }
    }

    impl OutboxTransport for MockTransport {
        fn dispatch<'a>(
            &'a self,
            item: &'a OutboxItem,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), OutboxDispatchError>> + Send + 'a>> {
            let item_clone = item.clone();
            let dispatched = Arc::clone(&self.dispatched);
            let should_fail = Arc::clone(&self.should_fail);

            Box::pin(async move {
                if *should_fail.lock().unwrap() {
                    Err(OutboxDispatchError::PeerUnreachable(item_clone.recipient_fp))
                } else {
                    dispatched.lock().unwrap().push(item_clone);
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    pub async fn test_offline_queue_auto_flush_on_peer_discovery() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_outbox_test_{}", uuid::Uuid::new_v4()));
        let chat_dir = temp_dir.join("chat");

        let alice_fp = "AAAA111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0001";
        let local_fp = "BBBB111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0002";

        let store = ChatStore::open(&chat_dir).await.expect("open chat store");
        let mock_handle = MockTransportHandle::new();
        let dispatched = mock_handle.dispatched.clone();
        let transport = Arc::new(mock_handle.transport);

        let dispatcher = OutboxDispatcher::new(
            &chat_dir,
            store.clone(),
            transport,
            OutboxConfig::default(),
        )
        .await
        .expect("create outbox dispatcher");

        let registry = PeerRegistry::new();
        let cancel = CancellationToken::new();

        // Start reactive listener
        let _listener_handle = dispatcher.start_reactive_listener(registry.clone(), cancel.clone());

        // Alice is currently offline. User authors 3 messages in causal sequence.
        let msg1 = ChatMessageDto {
            id: "msg-001".to_string(),
            conversation_id: alice_fp.to_string(),
            sender_fp: local_fp.to_string(),
            recipient_fp: alice_fp.to_string(),
            timestamp_ms: 1700000001000,
            content: ChatContentDto::Text {
                text: "First causal message".to_string(),
            },
            reply_to_id: None,
            signature: None,
        };
        let msg2 = ChatMessageDto {
            id: "msg-002".to_string(),
            conversation_id: alice_fp.to_string(),
            sender_fp: local_fp.to_string(),
            recipient_fp: alice_fp.to_string(),
            timestamp_ms: 1700000002000,
            content: ChatContentDto::Text {
                text: "Second causal message".to_string(),
            },
            reply_to_id: Some("msg-001".to_string()),
            signature: None,
        };
        let msg3 = ChatMessageDto {
            id: "msg-003".to_string(),
            conversation_id: alice_fp.to_string(),
            sender_fp: local_fp.to_string(),
            recipient_fp: alice_fp.to_string(),
            timestamp_ms: 1700000003000,
            content: ChatContentDto::Text {
                text: "Third causal message".to_string(),
            },
            reply_to_id: Some("msg-002".to_string()),
            signature: None,
        };

        dispatcher.enqueue_message(msg1).await.expect("enqueue msg1");
        dispatcher.enqueue_message(msg2).await.expect("enqueue msg2");
        dispatcher.enqueue_message(msg3).await.expect("enqueue msg3");

        // Verify all 3 messages are queued in outbox
        assert_eq!(dispatcher.queue_len().await, 3);
        assert_eq!(dispatched.lock().unwrap().len(), 0);

        // Verify stored state is Queued
        let stored_m1 = store.get_message("msg-001").await.unwrap();
        assert_eq!(stored_m1.delivery_state, MessageDeliveryState::Queued);

        // Alice comes online in PeerRegistry
        let alice_peer = DiscoveredPeer {
            fingerprint: alice_fp.to_string(),
            alias: "Alice".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            ..Default::default()
        };

        registry.insert_or_update(alice_peer).await;

        // Await auto-flush trigger (within 200ms)
        let start = std::time::Instant::now();
        while dispatcher.queue_len().await > 0 && start.elapsed() < Duration::from_millis(500) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        // Verify outbox queue is now 0
        assert_eq!(dispatcher.queue_len().await, 0);

        // Verify mock transport received all 3 items in exact FIFO causal order
        let recorded = dispatched.lock().unwrap().clone();
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0].id, "msg-001");
        assert_eq!(recorded[1].id, "msg-002");
        assert_eq!(recorded[2].id, "msg-003");

        // Verify stored messages transitioned to Sending
        let updated_m1 = store.get_message("msg-001").await.unwrap();
        assert_eq!(updated_m1.delivery_state, MessageDeliveryState::Sending);
        let updated_m3 = store.get_message("msg-003").await.unwrap();
        assert_eq!(updated_m3.delivery_state, MessageDeliveryState::Sending);

        cancel.cancel();
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[tokio::test]
    pub async fn test_delivery_and_read_receipt_monotonic_transitions() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_receipt_test_{}", uuid::Uuid::new_v4()));
        let chat_dir = temp_dir.join("chat");

        let bob_fp = "BBBB111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0002";
        let local_fp = "AAAA111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0001";

        let store = ChatStore::open(&chat_dir).await.expect("open chat store");
        let mock_handle = MockTransportHandle::new();
        let dispatcher = OutboxDispatcher::new(
            &chat_dir,
            store.clone(),
            Arc::new(mock_handle.transport),
            OutboxConfig::default(),
        )
        .await
        .expect("create outbox dispatcher");

        // 1. Author message to Bob
        let msg = ChatMessageDto {
            id: "msg-100".to_string(),
            conversation_id: bob_fp.to_string(),
            sender_fp: local_fp.to_string(),
            recipient_fp: bob_fp.to_string(),
            timestamp_ms: 1700000000000,
            content: ChatContentDto::Text {
                text: "Hey Bob!".to_string(),
            },
            reply_to_id: None,
            signature: None,
        };

        dispatcher.enqueue_message(msg).await.unwrap();
        assert_eq!(dispatcher.queue_len().await, 1);

        // Flush to Bob
        dispatcher.flush_peer(bob_fp).await;
        assert_eq!(dispatcher.queue_len().await, 0);

        let stored = store.get_message("msg-100").await.unwrap();
        assert_eq!(stored.delivery_state, MessageDeliveryState::Sending);

        // 2. Incoming DeliveryReceiptDto arrives from Bob
        let delivery_rcpt = DeliveryReceiptDto {
            receipt_id: "rcpt-del-100".to_string(),
            message_id: "msg-100".to_string(),
            conversation_id: bob_fp.to_string(),
            sender_fp: local_fp.to_string(),
            recipient_fp: bob_fp.to_string(),
            delivered_at_ms: 1700000001000,
            signature: None,
        };

        assert!(dispatcher.process_incoming_delivery_receipt(&delivery_rcpt).await.unwrap());
        let delivered_msg = store.get_message("msg-100").await.unwrap();
        assert_eq!(delivered_msg.delivery_state, MessageDeliveryState::Delivered);
        assert_eq!(delivered_msg.delivered_at_ms, Some(1700000001000));

        // Duplicate delivery receipt -> should not modify or error (returns false)
        assert!(!dispatcher.process_incoming_delivery_receipt(&delivery_rcpt).await.unwrap());
        assert_eq!(store.get_message("msg-100").await.unwrap().delivery_state, MessageDeliveryState::Delivered);

        // Out-of-order Sending update -> cannot regress Delivered
        assert!(!store.update_message_delivery_state("msg-100", MessageDeliveryState::Sending).await.unwrap());
        assert_eq!(store.get_message("msg-100").await.unwrap().delivery_state, MessageDeliveryState::Delivered);

        // 3. Incoming ReadReceiptDto arrives from Bob
        let read_rcpt = ReadReceiptDto {
            receipt_id: "rcpt-read-100".to_string(),
            message_id: "msg-100".to_string(),
            conversation_id: bob_fp.to_string(),
            sender_fp: local_fp.to_string(),
            reader_fp: bob_fp.to_string(),
            read_at_ms: 1700000002000,
            signature: None,
        };

        assert!(dispatcher.process_incoming_read_receipt(&read_rcpt).await.unwrap());
        let read_msg = store.get_message("msg-100").await.unwrap();
        assert_eq!(read_msg.delivery_state, MessageDeliveryState::Read);
        assert_eq!(read_msg.read_at_ms, Some(1700000002000));

        // Delayed DeliveryReceipt arriving after Read -> cannot regress Read to Delivered
        assert!(!dispatcher.process_incoming_delivery_receipt(&delivery_rcpt).await.unwrap());
        assert_eq!(store.get_message("msg-100").await.unwrap().delivery_state, MessageDeliveryState::Read);

        // Failed state transition attempt -> cannot fail already read message
        assert!(!store.update_message_delivery_state("msg-100", MessageDeliveryState::Failed).await.unwrap());
        assert_eq!(store.get_message("msg-100").await.unwrap().delivery_state, MessageDeliveryState::Read);

        // 4. Test offline receipt queuing: local user generates a read receipt for offline peer Charlie
        let charlie_fp = "CCCC111122223333444455556666777788889999AAAABBBBCCCCDDDDEEEEFFFF0003";
        let charlie_rcpt = ReadReceiptDto {
            receipt_id: "rcpt-read-200".to_string(),
            message_id: "msg-200".to_string(),
            conversation_id: charlie_fp.to_string(),
            sender_fp: charlie_fp.to_string(),
            reader_fp: local_fp.to_string(),
            read_at_ms: 1700000003000,
            signature: None,
        };

        dispatcher.enqueue_read_receipt(charlie_rcpt).await.expect("enqueue charlie receipt");
        assert_eq!(dispatcher.queue_len().await, 1);
        assert_eq!(dispatcher.pending_items_for_peer(charlie_fp).await.len(), 1);

        // Flush Charlie once online
        let flush_res = dispatcher.flush_peer(charlie_fp).await;
        assert_eq!(flush_res.len(), 1);
        assert!(flush_res[0].is_ok());
        assert_eq!(dispatcher.queue_len().await, 0);

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
