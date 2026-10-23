//! 1:1 rich-media chat orchestrator and HTTP endpoint handlers.
//!
//! Sub-phase 8.3 implementation:
//! - Wires `TrustGate`, `ChatStore`, `OutboxDispatcher`, and `ResumeManager` into a unified `ChatEngine`.
//! - Mounts and serves 1:1 chat & media endpoints:
//!   - `POST /api/lanflare/v1/chat/request` (Stranger introduction handshake with Hashcash PoW)
//!   - `POST /api/lanflare/v1/chat/message` (1:1 message exchange with cryptographic delivery receipt)
//!   - `POST /api/lanflare/v1/chat/receipt` (Delivery and read receipt state transitions)
//!   - `POST /api/lanflare/v1/chat/media` (Direct chunked resumable media upload stream)
//!   - `POST /api/lanflare/v1/transfer/resume` (Resumable byte-offset transfer negotiation)
//! - Outbox transport integrating 1-hop direct LAN HTTP and multi-hop mesh packet forwarding.
//! - Reactive event emitter for GUI and CLI notification.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};
use lanflare_proto::chat::{
    ChatContentDto, ChatMessageDto, DeliveryReceiptDto, MessageDeliveryState, ReadReceiptDto,
    ResumeTransferDto, TrustRequestDto, DEFAULT_POW_DIFFICULTY_BITS, DEFAULT_TIME_BUCKET_SECS,
};
use lanflare_proto::mesh::{RoutedEnvelope, MAX_MESH_TTL};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::{broadcast, RwLock};

use crate::chat::outbox::{
    OutboxConfig, OutboxDispatchError, OutboxDispatcher, OutboxError, OutboxItem, OutboxPayload,
    OutboxTransport,
};
use crate::chat::store::{ChatStore, ChatStoreError, Contact};
use crate::chat::trust::{
    compute_time_bucket, generate_hashcash_pow, ContactTrustState, TrustError, TrustGate,
    TrustGateConfig,
};
use crate::crypto::cert::TlsIdentity;
use crate::crypto::e2ee::{open_envelope, seal_envelope, sign_with_identity, E2eeError};
use crate::discovery::registry::PeerRegistry;
use crate::mesh::router::MeshRouter;
use crate::server::dufs::{empty_box_response, full_box_body, json_response, ServerResponseBody};
use crate::server::handlers::percent_decode;
use crate::server::router::ClientConnectionInfo;
use crate::storage::resume::{ResumeError, ResumeManager};

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn text_box_response(status: StatusCode, text: impl Into<String>) -> Response<ServerResponseBody> {
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_box_body(text.into()))
        .unwrap()
}

/// Errors arising during chat engine operations and endpoint dispatch.
#[derive(Debug, Error)]
pub enum ChatEngineError {
    #[error("Store error: {0}")]
    Store(#[from] ChatStoreError),

    #[error("Trust gate error: {0}")]
    Trust(#[from] TrustError),

    #[error("Outbox error: {0}")]
    Outbox(#[from] OutboxError),

    #[error("Resume storage error: {0}")]
    Resume(#[from] ResumeError),

    #[error("E2EE error: {0}")]
    E2ee(#[from] E2eeError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Contact not found: {0}")]
    ContactNotFound(String),

    #[error("Conversation not found: {0}")]
    ConversationNotFound(String),

    #[error("Message not found: {0}")]
    MessageNotFound(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),
}

/// Events emitted by `ChatEngine` for UI notifications and test observers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ChatEngineEvent {
    /// Received a trust introduction handshake request from a stranger.
    TrustRequestReceived(TrustRequestDto),

    /// A contact's trust state transitioned to TrustedContact.
    ContactApproved { peer_fp: String },

    /// A contact's trust state transitioned to Blocked.
    ContactBlocked { peer_fp: String },

    /// Received an incoming 1:1 or group chat message.
    MessageReceived(ChatMessageDto),

    /// Received a delivery receipt acknowledgment.
    DeliveryReceiptReceived(DeliveryReceiptDto),

    /// Received a read receipt acknowledgment.
    ReadReceiptReceived(ReadReceiptDto),

    /// Streaming media transfer progress update.
    MediaProgress {
        session_id: String,
        file_id: String,
        received_bytes: u64,
        total_bytes: u64,
    },

    /// Streaming media transfer successfully completed and verified.
    MediaCompleted {
        session_id: String,
        file_id: String,
        file_name: String,
        target_path: PathBuf,
        sha256: String,
    },
}

/// Configuration options for initializing `ChatEngine`.
#[derive(Clone, Debug)]
pub struct ChatEngineConfig {
    pub alias: String,
    pub data_dir: PathBuf,
    pub media_dir: PathBuf,
    pub trust_config: TrustGateConfig,
    pub outbox_config: OutboxConfig,
}

impl ChatEngineConfig {
    /// Creates a configuration using base data and media directories.
    pub fn new(alias: impl Into<String>, base_dir: impl Into<PathBuf>) -> Self {
        let base = base_dir.into();
        Self {
            alias: alias.into(),
            data_dir: base.join("chat"),
            media_dir: base.join("media"),
            trust_config: TrustGateConfig::default(),
            outbox_config: OutboxConfig::default(),
        }
    }
}

/// Internal transport bridge decoupling `OutboxDispatcher` construction from `ChatEngine`.
struct EngineOutboxTransport(std::sync::RwLock<Option<std::sync::Weak<ChatEngine>>>);

impl OutboxTransport for EngineOutboxTransport {
    fn dispatch<'a>(
        &'a self,
        item: &'a OutboxItem,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), OutboxDispatchError>> + Send + 'a>> {
        Box::pin(async move {
            let weak_opt = self.0.read().unwrap().clone();
            if let Some(weak) = weak_opt
                && let Some(engine) = weak.upgrade()
            {
                return engine.dispatch_outbox_item(item).await;
            }
            Err(OutboxDispatchError::NetworkError("ChatEngine unavailable".into()))
        })
    }
}

/// Unified 1:1 chat engine and rich-media transfer orchestrator.
pub struct ChatEngine {
    identity: TlsIdentity,
    alias: RwLock<String>,
    store: Arc<ChatStore>,
    trust_gate: Arc<TrustGate>,
    outbox: Arc<OutboxDispatcher>,
    resume_manager: Arc<ResumeManager>,
    media_dir: PathBuf,
    registry: Arc<PeerRegistry>,
    mesh_router: RwLock<Option<Arc<MeshRouter>>>,
    event_tx: broadcast::Sender<ChatEngineEvent>,
    http_client: reqwest::Client,
}

impl ChatEngine {
    /// Initializes a new `ChatEngine` instance with persistent store, trust gate, and outbox.
    pub async fn new(
        identity: TlsIdentity,
        config: ChatEngineConfig,
        registry: Arc<PeerRegistry>,
    ) -> Result<Arc<Self>, ChatEngineError> {
        tokio::fs::create_dir_all(&config.data_dir).await?;
        tokio::fs::create_dir_all(&config.media_dir).await?;

        let store = ChatStore::open(&config.data_dir).await?;
        let trust_gate = Arc::new(TrustGate::new(identity.fingerprint.clone(), config.trust_config));
        let transport_bridge = Arc::new(EngineOutboxTransport(std::sync::RwLock::new(None)));

        let outbox = OutboxDispatcher::new(
            &config.data_dir,
            store.clone(),
            transport_bridge.clone(),
            config.outbox_config,
        )
        .await?;

        let resume_manager = Arc::new(ResumeManager::new(&config.media_dir));
        let (event_tx, _) = broadcast::channel(128);

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .tcp_nodelay(true)
            .build()
            .unwrap_or_default();

        let engine = Arc::new(Self {
            identity,
            alias: RwLock::new(config.alias),
            store,
            trust_gate,
            outbox: outbox.clone(),
            resume_manager,
            media_dir: config.media_dir,
            registry: registry.clone(),
            mesh_router: RwLock::new(None),
            event_tx,
            http_client,
        });

        // Wire weak reference into transport bridge
        *transport_bridge.0.write().unwrap() = Some(Arc::downgrade(&engine));

        // Note: OutboxDispatcher automatically handles flushes when items are enqueued or peers update

        Ok(engine)
    }

    /// Attaches an optional `MeshRouter` for multi-hop packet and stream routing.
    pub async fn attach_mesh_router(self: &Arc<Self>, mesh_router: Arc<MeshRouter>) {
        {
            let mut router_guard = self.mesh_router.write().await;
            *router_guard = Some(mesh_router.clone());
        }

        // Register packet sink on MeshRouter for locally delivered routed envelopes
        let engine_weak = Arc::downgrade(self);
        mesh_router
            .set_packet_sink(move |routed: RoutedEnvelope| {
                if let Some(engine) = engine_weak.upgrade() {
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.handle_routed_envelope(routed).await;
                    });
                }
            })
            .await;
    }

    /// Returns a reference to the local TLS identity.
    pub fn identity(&self) -> &TlsIdentity {
        &self.identity
    }

    /// Returns the canonical local fingerprint.
    pub fn local_fingerprint(&self) -> &str {
        &self.identity.fingerprint
    }

    /// Returns a reference to the persistent chat store.
    pub fn store(&self) -> &Arc<ChatStore> {
        &self.store
    }

    /// Returns a reference to the anti-harassment trust gate.
    pub fn trust_gate(&self) -> &Arc<TrustGate> {
        &self.trust_gate
    }

    /// Returns a reference to the delay-tolerant outbox dispatcher.
    pub fn outbox(&self) -> &Arc<OutboxDispatcher> {
        &self.outbox
    }

    /// Returns a reference to the resumable transfer manager.
    pub fn resume_manager(&self) -> &Arc<ResumeManager> {
        &self.resume_manager
    }

    /// Returns the media download directory.
    pub fn media_dir(&self) -> &std::path::Path {
        &self.media_dir
    }

    /// Subscribes to the live chat engine event stream.
    pub fn subscribe_events(&self) -> broadcast::Receiver<ChatEngineEvent> {
        self.event_tx.subscribe()
    }

    // --- Outbound Actions ---

    /// Sends a plain text message to a trusted peer.
    pub async fn send_text_message(
        &self,
        recipient_fp: &str,
        text: &str,
    ) -> Result<ChatMessageDto, ChatEngineError> {
        let recipient_fp = recipient_fp.to_ascii_uppercase();
        let conv_id = format!("conv_{}", recipient_fp);

        let msg_id = uuid::Uuid::new_v4().to_string();
        let timestamp_ms = current_time_ms();

        let mut msg = ChatMessageDto {
            id: msg_id,
            conversation_id: conv_id,
            sender_fp: self.identity.fingerprint.clone(),
            recipient_fp: recipient_fp.clone(),
            timestamp_ms,
            content: ChatContentDto::Text {
                text: text.to_string(),
            },
            reply_to_id: None,
            signature: None,
        };

        // Sign message with local identity
        let sig_bytes = sign_with_identity(&self.identity.key_pem, &msg.canonical_signing_bytes())?;
        msg.signature = Some(STANDARD.encode(sig_bytes));

        // Enqueue into DTN outbox and persist to ChatStore in Queued state
        self.outbox.enqueue_message(msg.clone()).await?;

        // Trigger flush attempt
        let _ = self.outbox.flush_all().await;

        Ok(msg)
    }

    /// Sends a rich media message descriptor (Image, File, Voice, or Video) to a trusted peer.
    pub async fn send_media_message(
        &self,
        recipient_fp: &str,
        content: ChatContentDto,
    ) -> Result<ChatMessageDto, ChatEngineError> {
        let recipient_fp = recipient_fp.to_ascii_uppercase();
        let conv_id = format!("conv_{}", recipient_fp);

        let msg_id = uuid::Uuid::new_v4().to_string();
        let timestamp_ms = current_time_ms();

        let mut msg = ChatMessageDto {
            id: msg_id,
            conversation_id: conv_id,
            sender_fp: self.identity.fingerprint.clone(),
            recipient_fp: recipient_fp.clone(),
            timestamp_ms,
            content,
            reply_to_id: None,
            signature: None,
        };

        let sig_bytes = sign_with_identity(&self.identity.key_pem, &msg.canonical_signing_bytes())?;
        msg.signature = Some(STANDARD.encode(sig_bytes));

        self.outbox.enqueue_message(msg.clone()).await?;
        let _ = self.outbox.flush_all().await;

        Ok(msg)
    }

    /// Sends an introductory trust handshake request with Hashcash PoW to a stranger.
    pub async fn send_trust_request(
        &self,
        target_fp: &str,
        intro_message: &str,
    ) -> Result<TrustRequestDto, ChatEngineError> {
        let target_fp = target_fp.to_ascii_uppercase();
        let alias = self.alias.read().await.clone();
        let now_ms = current_time_ms();
        let now_secs = now_ms / 1000;
        let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);

        // Generate Hashcash stamp satisfying recipient difficulty
        let pow_stamp = generate_hashcash_pow(
            &self.identity.fingerprint,
            &target_fp,
            DEFAULT_POW_DIFFICULTY_BITS,
            time_bucket,
            1_000_000,
        )?;

        let mut req = TrustRequestDto {
            request_id: uuid::Uuid::new_v4().to_string(),
            sender_fp: self.identity.fingerprint.clone(),
            sender_alias: alias,
            target_fp: target_fp.clone(),
            message: intro_message.to_string(),
            pow_stamp,
            timestamp_ms: now_ms,
            signature: None,
        };

        let sig_bytes = sign_with_identity(&self.identity.key_pem, &req.canonical_signing_bytes())?;
        req.signature = Some(STANDARD.encode(sig_bytes));

        // Directly post or forward trust request
        if let Some(peer) = self.registry.get(&target_fp).await {
            if let Some(base_url) = peer.primary_base_url() {
                let url = format!("{base_url}/api/lanflare/v1/chat/request");
                let _ = self.http_client.post(&url).json(&req).send().await;
            }
        } else if let Some(mesh) = self.mesh_router.read().await.as_ref() {
            let json_bytes = serde_json::to_vec(&req)?;
            let cert_pem_opt = mesh.graph().read().await.get_node(&target_fp).and_then(|n| n.cert_pem.clone());
            if let Some(cert_pem) = cert_pem_opt
                && let Ok(e2ee) = seal_envelope(&self.identity, &target_fp, &cert_pem, &json_bytes)
            {
                let routed = RoutedEnvelope {
                    packet_id: uuid::Uuid::new_v4().to_string(),
                    src_fp: self.identity.fingerprint.clone(),
                    dst_fp: target_fp,
                    ttl: MAX_MESH_TTL,
                    route_path: vec![self.identity.fingerprint.clone()],
                    payload: e2ee,
                };
                let _ = mesh.forward_envelope(routed).await;
            }
        }

        Ok(req)
    }

    /// Explicitly approves a pending contact, elevating them to `TrustedContact`.
    pub async fn approve_contact(&self, peer_fp: &str) -> Result<(), ChatEngineError> {
        let peer_fp = peer_fp.to_ascii_uppercase();
        self.trust_gate.approve_contact(&peer_fp);

        if let Some(mut contact) = self.store.get_contact(&peer_fp).await {
            contact.trust_state = ContactTrustState::TrustedContact;
            self.store.upsert_contact(contact).await?;
        }

        let _ = self.event_tx.send(ChatEngineEvent::ContactApproved {
            peer_fp: peer_fp.clone(),
        });
        Ok(())
    }

    /// Blocks a peer, rejecting any future messages or requests in O(1) time.
    pub async fn block_contact(&self, peer_fp: &str) -> Result<(), ChatEngineError> {
        let peer_fp = peer_fp.to_ascii_uppercase();
        self.trust_gate.block_contact(&peer_fp);

        if let Some(mut contact) = self.store.get_contact(&peer_fp).await {
            contact.trust_state = ContactTrustState::Blocked;
            self.store.upsert_contact(contact).await?;
        }

        let _ = self.event_tx.send(ChatEngineEvent::ContactBlocked {
            peer_fp: peer_fp.clone(),
        });
        Ok(())
    }

    /// Marks a received message as opened/read by the local user, generating and queuing a ReadReceipt.
    pub async fn mark_message_as_read(
        &self,
        message_id: &str,
    ) -> Result<ReadReceiptDto, ChatEngineError> {
        let mut msg = self
            .store
            .get_message(message_id)
            .await
            .ok_or_else(|| ChatEngineError::MessageNotFound(message_id.to_string()))?;

        let now_ms = current_time_ms();
        msg.delivery_state.apply_monotonic_transition(MessageDeliveryState::Read);
        msg.read_at_ms = Some(now_ms);

        let conv_id = msg.message.conversation_id.clone();
        let sender_fp = msg.message.sender_fp.clone();

        // Update in chat store
        self.store
            .insert_message(msg.message.clone(), msg.delivery_state, msg.is_outgoing)
            .await?;

        let mut receipt = ReadReceiptDto {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            message_id: message_id.to_string(),
            conversation_id: conv_id,
            sender_fp,
            reader_fp: self.identity.fingerprint.clone(),
            read_at_ms: now_ms,
            signature: None,
        };

        let sig_bytes = sign_with_identity(&self.identity.key_pem, &receipt.canonical_signing_bytes())?;
        receipt.signature = Some(STANDARD.encode(sig_bytes));

        // Queue read receipt in DTN outbox back to author
        self.outbox
            .enqueue_read_receipt(receipt.clone())
            .await?;
        let _ = self.outbox.flush_all().await;

        let _ = self
            .event_tx
            .send(ChatEngineEvent::ReadReceiptReceived(receipt.clone()));

        Ok(receipt)
    }

    // --- Inbound Handlers ---

    /// Evaluates and handles an incoming `TrustRequestDto` from a stranger.
    pub async fn handle_trust_request(
        &self,
        req: TrustRequestDto,
        remote_ip: Option<IpAddr>,
    ) -> Result<TrustRequestDto, ChatEngineError> {
        let sender_fp = req.sender_fp.to_ascii_uppercase();
        let client_ip = remote_ip.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let now_secs = current_time_ms() / 1000;

        // 5-layer trust gate evaluation (PoW, rate limiting, stranger gate)
        self.trust_gate
            .evaluate_trust_request(&req, client_ip, now_secs)?;

        // Record contact in persistent store as PendingApproval
        let contact = Contact::new(
            sender_fp.clone(),
            req.sender_alias.clone(),
            ContactTrustState::PendingApproval,
            req.timestamp_ms,
        );
        self.store.upsert_contact(contact).await?;

        // If an introductory message is present, store it as delivered
        if !req.message.trim().is_empty() {
            let conv_id = format!("conv_{}", sender_fp);
            let intro_msg = ChatMessageDto {
                id: req.request_id.clone(),
                conversation_id: conv_id,
                sender_fp: sender_fp.clone(),
                recipient_fp: self.identity.fingerprint.clone(),
                timestamp_ms: req.timestamp_ms,
                content: ChatContentDto::Text {
                    text: req.message.clone(),
                },
                reply_to_id: None,
                signature: req.signature.clone(),
            };
            self.store
                .insert_message(intro_msg, MessageDeliveryState::Delivered, false)
                .await?;
        }

        let _ = self
            .event_tx
            .send(ChatEngineEvent::TrustRequestReceived(req.clone()));

        Ok(req)
    }

    /// Evaluates, stores, and acknowledges an incoming 1:1 `ChatMessageDto`.
    pub async fn handle_chat_message(
        &self,
        msg: ChatMessageDto,
        _remote_ip: Option<IpAddr>,
    ) -> Result<DeliveryReceiptDto, ChatEngineError> {
        let sender_fp = msg.sender_fp.to_ascii_uppercase();

        // Evaluate sender trust state
        let trust_state = self.trust_gate.get_trust_state(&sender_fp);
        match trust_state {
            ContactTrustState::Stranger | ContactTrustState::Blocked => {
                if msg.content.is_media() {
                    return Err(ChatEngineError::Trust(
                        TrustError::MediaNotPermittedForStranger,
                    ));
                } else {
                    return Err(ChatEngineError::Trust(TrustError::PendingApprovalLocked));
                }
            }
            ContactTrustState::PendingApproval => {
                if msg.content.is_media() {
                    return Err(ChatEngineError::Trust(
                        TrustError::MediaNotPermittedForStranger,
                    ));
                } else {
                    return Err(ChatEngineError::Trust(TrustError::PendingApprovalLocked));
                }
            }
            ContactTrustState::TrustedContact => {
                // Trusted peer, proceed
            }
        }

        let delivered_at_ms = current_time_ms();
        let conv_id = format!("conv_{}", sender_fp);

        self.store
            .insert_message(msg.clone(), MessageDeliveryState::Delivered, false)
            .await?;

        // Update contact last seen
        let _ = self
            .store
            .update_contact_last_seen(&sender_fp, delivered_at_ms)
            .await;

        // Generate cryptographic DeliveryReceipt
        let mut receipt = DeliveryReceiptDto {
            receipt_id: uuid::Uuid::new_v4().to_string(),
            message_id: msg.id.clone(),
            conversation_id: conv_id,
            sender_fp: sender_fp.clone(),
            recipient_fp: self.identity.fingerprint.clone(),
            delivered_at_ms,
            signature: None,
        };

        let sig_bytes = sign_with_identity(&self.identity.key_pem, &receipt.canonical_signing_bytes())?;
        receipt.signature = Some(STANDARD.encode(sig_bytes));

        let _ = self
            .event_tx
            .send(ChatEngineEvent::MessageReceived(msg.clone()));

        Ok(receipt)
    }

    /// Handles an incoming delivery or read receipt, advancing message progression monotonically.
    pub async fn handle_receipt_json(&self, json_str: &str) -> Result<(), ChatEngineError> {
        if let Ok(delivery) = serde_json::from_str::<DeliveryReceiptDto>(json_str) {
            self.store.apply_delivery_receipt(&delivery).await?;
            let _ = self
                .event_tx
                .send(ChatEngineEvent::DeliveryReceiptReceived(delivery));
            return Ok(());
        }

        if let Ok(read) = serde_json::from_str::<ReadReceiptDto>(json_str) {
            self.store.apply_read_receipt(&read).await?;
            let _ = self
                .event_tx
                .send(ChatEngineEvent::ReadReceiptReceived(read));
            return Ok(());
        }

        Err(ChatEngineError::BadRequest(
            "Payload matches neither DeliveryReceiptDto nor ReadReceiptDto".into(),
        ))
    }

    /// Negotiates an incoming resumable transfer probe via `ResumeManager`.
    pub async fn handle_resume_request(
        &self,
        req: ResumeTransferDto,
    ) -> Result<ResumeTransferDto, ChatEngineError> {
        let sender_fp = req.sender_fp.to_ascii_uppercase();
        let trust = self.trust_gate.get_trust_state(&sender_fp);
        if trust != ContactTrustState::TrustedContact {
            return Err(ChatEngineError::Trust(
                TrustError::MediaNotPermittedForStranger,
            ));
        }

        let resp = self.resume_manager.handle_resume_request(&req).await?;
        Ok(resp)
    }

    // --- Outbox Dispatch Execution ---

    async fn dispatch_outbox_item(&self, item: &OutboxItem) -> Result<(), OutboxDispatchError> {
        let target_fp = item.recipient_fp.to_ascii_uppercase();

        // 1. Direct 1-hop LAN delivery if peer is discovered in PeerRegistry
        if let Some(peer) = self.registry.get(&target_fp).await
            && let Some(base_url) = peer.primary_base_url()
        {
            let client = &self.http_client;

                match &item.payload {
                    OutboxPayload::Message(m) => {
                        let url = format!("{base_url}/api/lanflare/v1/chat/message");
                        match client.post(&url).json(m).send().await {
                            Ok(res) if res.status().is_success() => {
                                if let Ok(receipt) = res.json::<DeliveryReceiptDto>().await {
                                    let _ = self.store.apply_delivery_receipt(&receipt).await;
                                }
                                return Ok(());
                            }
                            Ok(res) => {
                                return Err(OutboxDispatchError::Rejected(format!(
                                    "HTTP status {}",
                                    res.status()
                                )));
                            }
                            Err(e) => {
                                return Err(OutboxDispatchError::NetworkError(e.to_string()));
                            }
                        }
                    }
                    OutboxPayload::DeliveryReceipt(r) => {
                        let url = format!("{base_url}/api/lanflare/v1/chat/receipt");
                        match client.post(&url).json(r).send().await {
                            Ok(res) if res.status().is_success() => return Ok(()),
                            Ok(res) => {
                                return Err(OutboxDispatchError::Rejected(format!(
                                    "HTTP status {}",
                                    res.status()
                                )));
                            }
                            Err(e) => return Err(OutboxDispatchError::NetworkError(e.to_string())),
                        }
                    }
                    OutboxPayload::ReadReceipt(r) => {
                        let url = format!("{base_url}/api/lanflare/v1/chat/receipt");
                        match client.post(&url).json(r).send().await {
                            Ok(res) if res.status().is_success() => return Ok(()),
                            Ok(res) => {
                                return Err(OutboxDispatchError::Rejected(format!(
                                    "HTTP status {}",
                                    res.status()
                                )));
                            }
                            Err(e) => return Err(OutboxDispatchError::NetworkError(e.to_string())),
                        }
                    }
                }
        }

        // 2. Multi-hop mesh relay routing if MeshRouter is attached and route exists
        if let Some(mesh) = self.mesh_router.read().await.as_ref() {
            let (has_route, cert_pem_opt) = {
                let graph = mesh.graph().read().await;
                let route = graph.resolve_route(&target_fp);
                let cert = graph.get_node(&target_fp).and_then(|n| n.cert_pem.clone());
                (route.is_some(), cert)
            };

            if has_route
                && let Some(cert_pem) = cert_pem_opt
            {
                let payload_bytes = match &item.payload {
                        OutboxPayload::Message(m) => serde_json::to_vec(m),
                        OutboxPayload::DeliveryReceipt(r) => serde_json::to_vec(r),
                        OutboxPayload::ReadReceipt(r) => serde_json::to_vec(r),
                    }
                    .map_err(|e| OutboxDispatchError::NetworkError(e.to_string()))?;

                    let e2ee = seal_envelope(&self.identity, &target_fp, &cert_pem, &payload_bytes)
                        .map_err(|e| OutboxDispatchError::NetworkError(e.to_string()))?;

                    let routed = RoutedEnvelope {
                        packet_id: uuid::Uuid::new_v4().to_string(),
                        src_fp: self.identity.fingerprint.clone(),
                        dst_fp: target_fp.clone(),
                        ttl: MAX_MESH_TTL,
                        route_path: vec![self.identity.fingerprint.clone()],
                        payload: e2ee,
                    };

                    match mesh.forward_envelope(routed).await {
                        Ok(_) => return Ok(()),
                        Err(e) => return Err(OutboxDispatchError::NetworkError(e.to_string())),
                    }
                }
            }

        // 3. Peer currently unreachable on both direct LAN and mesh
        Err(OutboxDispatchError::PeerUnreachable(format!(
            "Recipient {target_fp} is currently offline"
        )))
    }

    /// Handles an incoming routed envelope delivered locally from the overlay mesh router.
    async fn handle_routed_envelope(&self, routed: RoutedEnvelope) {
        let sender_cert_opt = if let Some(mesh) = self.mesh_router.read().await.as_ref() {
            mesh.graph().read().await.get_node(&routed.src_fp).and_then(|n| n.cert_pem.clone())
        } else {
            None
        };

        if let Ok(decrypted) = open_envelope(&self.identity, sender_cert_opt.as_deref(), &routed.payload) {
            // 1. Try ChatMessageDto
            if let Ok(msg) = serde_json::from_slice::<ChatMessageDto>(&decrypted) {
                if let Ok(delivery) = self.handle_chat_message(msg, None).await {
                    // Return delivery receipt back across mesh
                    let _ = self.outbox.enqueue_delivery_receipt(delivery).await;
                    let _ = self.outbox.flush_all().await;
                }
                return;
            }

            // 2. Try TrustRequestDto
            if let Ok(req) = serde_json::from_slice::<TrustRequestDto>(&decrypted) {
                let _ = self.handle_trust_request(req, None).await;
                return;
            }

            // 3. Try DeliveryReceiptDto
            if let Ok(delivery) = serde_json::from_slice::<DeliveryReceiptDto>(&decrypted) {
                let _ = self.store.apply_delivery_receipt(&delivery).await;
                let _ = self
                    .event_tx
                    .send(ChatEngineEvent::DeliveryReceiptReceived(delivery));
                return;
            }

            // 4. Try ReadReceiptDto
            if let Ok(read) = serde_json::from_slice::<ReadReceiptDto>(&decrypted) {
                let _ = self.store.apply_read_receipt(&read).await;
                let _ = self
                    .event_tx
                    .send(ChatEngineEvent::ReadReceiptReceived(read));
            }
        }
    }

    // --- Hyper Request Dispatcher ---

    /// Dispatches an incoming Hyper request to the appropriate chat or media handler.
    pub async fn handle_hyper_request(
        self: &Arc<Self>,
        mut req: Request<Incoming>,
        client: Option<ClientConnectionInfo>,
    ) -> Response<ServerResponseBody> {
        let path = req.uri().path().to_string();
        let method = req.method().clone();
        let remote_ip = client.map(|c| c.remote_ip);

        match (method, path.as_str()) {
            (Method::POST, "/api/lanflare/v1/chat/request") => {
                let body_bytes = match req.body_mut().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read request body: {e}"),
                        );
                    }
                };

                let trust_req = match serde_json::from_slice::<TrustRequestDto>(&body_bytes) {
                    Ok(tr) => tr,
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Invalid TrustRequestDto JSON: {e}"),
                        );
                    }
                };

                match self.handle_trust_request(trust_req, remote_ip).await {
                    Ok(resp) => json_response(StatusCode::OK, &resp),
                    Err(ChatEngineError::Trust(TrustError::RateLimited { .. })) => {
                        text_box_response(StatusCode::TOO_MANY_REQUESTS, "Rate limited")
                    }
                    Err(ChatEngineError::Trust(TrustError::ContactsOnlyMode)) => {
                        text_box_response(StatusCode::FORBIDDEN, "Contacts only mode active")
                    }
                    Err(ChatEngineError::Trust(TrustError::InsufficientPoW { .. }))
                    | Err(ChatEngineError::Trust(TrustError::PoWExpired { .. }))
                    | Err(ChatEngineError::Trust(TrustError::PoWTargetMismatch { .. })) => {
                        text_box_response(StatusCode::UNPROCESSABLE_ENTITY, "Invalid Hashcash PoW")
                    }
                    Err(e) => text_box_response(StatusCode::FORBIDDEN, e.to_string()),
                }
            }

            (Method::POST, "/api/lanflare/v1/chat/message") => {
                let body_bytes = match req.body_mut().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read request body: {e}"),
                        );
                    }
                };

                let msg_dto = match serde_json::from_slice::<ChatMessageDto>(&body_bytes) {
                    Ok(m) => m,
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Invalid ChatMessageDto JSON: {e}"),
                        );
                    }
                };

                match self.handle_chat_message(msg_dto, remote_ip).await {
                    Ok(receipt) => json_response(StatusCode::OK, &receipt),
                    Err(ChatEngineError::Trust(TrustError::MediaNotPermittedForStranger)) => {
                        text_box_response(
                            StatusCode::FORBIDDEN,
                            "Media not permitted for stranger or pending contact",
                        )
                    }
                    Err(ChatEngineError::Trust(TrustError::PendingApprovalLocked)) => {
                        text_box_response(StatusCode::FORBIDDEN, "Contact pending approval")
                    }
                    Err(e) => text_box_response(StatusCode::FORBIDDEN, e.to_string()),
                }
            }

            (Method::POST, "/api/lanflare/v1/chat/receipt") => {
                let body_bytes = match req.body_mut().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read request body: {e}"),
                        );
                    }
                };

                let body_str = String::from_utf8_lossy(&body_bytes);
                match self.handle_receipt_json(&body_str).await {
                    Ok(_) => json_response(StatusCode::OK, &serde_json::json!({"status": "ok"})),
                    Err(e) => text_box_response(StatusCode::BAD_REQUEST, e.to_string()),
                }
            }

            (Method::POST, "/api/lanflare/v1/transfer/resume") => {
                let body_bytes = match req.body_mut().collect().await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Failed to read request body: {e}"),
                        );
                    }
                };

                let resume_dto = match serde_json::from_slice::<ResumeTransferDto>(&body_bytes) {
                    Ok(dto) => dto,
                    Err(e) => {
                        return text_box_response(
                            StatusCode::BAD_REQUEST,
                            format!("Invalid ResumeTransferDto JSON: {e}"),
                        );
                    }
                };

                match self.handle_resume_request(resume_dto).await {
                    Ok(resp) => json_response(StatusCode::OK, &resp),
                    Err(e) => text_box_response(StatusCode::FORBIDDEN, e.to_string()),
                }
            }

            (Method::POST, "/api/lanflare/v1/chat/media") => {
                let mut params = HashMap::new();
                if let Some(query) = req.uri().query() {
                    for pair in query.split('&') {
                        if let Some((k, v)) = pair.split_once('=') {
                            params.insert(k.to_string(), percent_decode(v));
                        }
                    }
                }

                let session_id = params.get("sessionId").cloned().unwrap_or_default();
                let file_id = params.get("fileId").cloned().unwrap_or_default();
                let file_name = params.get("fileName").cloned().unwrap_or_default();
                let sender_fp = params.get("senderFp").cloned().unwrap_or_default();
                let total_bytes = params
                    .get("totalBytes")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or_default();
                let expected_sha256 = params.get("sha256").cloned().unwrap_or_default();
                let offset = params
                    .get("offset")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or_default();

                if session_id.is_empty()
                    || file_id.is_empty()
                    || file_name.is_empty()
                    || sender_fp.is_empty()
                {
                    return text_box_response(
                        StatusCode::BAD_REQUEST,
                        "Missing required query params (sessionId, fileId, fileName, senderFp)",
                    );
                }

                // Verify contact trust
                let trust = self.trust_gate.get_trust_state(&sender_fp);
                if trust != ContactTrustState::TrustedContact {
                    return text_box_response(
                        StatusCode::FORBIDDEN,
                        "Rich media streaming not permitted for non-trusted contact",
                    );
                }

                let writer_res = self
                    .resume_manager
                    .create_writer(
                        &session_id,
                        &file_id,
                        &file_name,
                        &sender_fp,
                        &self.identity.fingerprint,
                        total_bytes,
                        &expected_sha256,
                        128 * 1024,
                    )
                    .await;

                let mut writer = match writer_res {
                    Ok(w) => w,
                    Err(e) => {
                        return text_box_response(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to initialize writer: {e}"),
                        );
                    }
                };

                if writer.received_bytes() != offset {
                    return text_box_response(
                        StatusCode::CONFLICT,
                        format!(
                            "Offset mismatch: client offered {offset}, but server verified {}",
                            writer.received_bytes()
                        ),
                    );
                }

                // Stream incoming HTTP body chunks
                let mut body = req.into_body();
                while let Some(frame_res) = body.frame().await {
                    match frame_res {
                        Ok(frame) => {
                            if let Ok(data) = frame.into_data() {
                                if let Err(e) = writer.write_chunk(&data).await {
                                    return text_box_response(
                                        StatusCode::BAD_REQUEST,
                                        format!("Stream write error: {e}"),
                                    );
                                }
                                let _ = self.event_tx.send(ChatEngineEvent::MediaProgress {
                                    session_id: session_id.clone(),
                                    file_id: file_id.clone(),
                                    received_bytes: writer.received_bytes(),
                                    total_bytes,
                                });
                            }
                        }
                        Err(e) => {
                            // Interruption: pause writer and preserve disk checkpoint
                            let _ = writer.pause().await;
                            return text_box_response(
                                StatusCode::BAD_REQUEST,
                                format!("Inbound stream error: {e}"),
                            );
                        }
                    }
                }

                if writer.is_complete() {
                    match writer.commit().await {
                        Ok(commit_result) => {
                            let _ = self.event_tx.send(ChatEngineEvent::MediaCompleted {
                                session_id: session_id.clone(),
                                file_id: file_id.clone(),
                                file_name: file_name.clone(),
                                target_path: commit_result.target_path.clone(),
                                sha256: commit_result.sha256.clone(),
                            });
                            json_response(
                                StatusCode::OK,
                                &serde_json::json!({
                                    "status": "completed",
                                    "bytesWritten": commit_result.bytes_written,
                                    "sha256": commit_result.sha256,
                                }),
                            )
                        }
                        Err(e) => text_box_response(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            format!("Commit verification failed: {e}"),
                        ),
                    }
                } else {
                    let paused = match writer.pause().await {
                        Ok(p) => p,
                        Err(e) => {
                            return text_box_response(
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("Pause failed: {e}"),
                            );
                        }
                    };
                    json_response(
                        StatusCode::OK,
                        &serde_json::json!({
                            "status": "interrupted",
                            "receivedBytes": paused.received_bytes,
                            "totalBytes": paused.total_bytes,
                        }),
                    )
                }
            }

            _ => empty_box_response(StatusCode::NOT_FOUND),
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[tokio::test]
    pub async fn test_chat_engine_stranger_handshake_and_message_delivery() {
        let temp_dir = std::env::temp_dir().join(format!("lanflare_chat_engine_{}", uuid::Uuid::new_v4()));
        let ident_a = crate::crypto::cert::generate_self_signed().expect("ident A");
        let ident_b = crate::crypto::cert::generate_self_signed().expect("ident B");

        let registry_b = PeerRegistry::new();
        let cfg_b = ChatEngineConfig::new("Bob-Node", &temp_dir);
        let engine_b = ChatEngine::new(ident_b.clone(), cfg_b, registry_b)
            .await
            .expect("engine B");

        // 1. Stranger Alice sends introductory trust handshake with valid Hashcash
        let now_secs = current_time_ms() / 1000;
        let time_bucket = compute_time_bucket(now_secs, DEFAULT_TIME_BUCKET_SECS);
        let pow_stamp = generate_hashcash_pow(
            &ident_a.fingerprint,
            &ident_b.fingerprint,
            DEFAULT_POW_DIFFICULTY_BITS,
            time_bucket,
            1_000_000,
        )
        .expect("pow stamp");

        let trust_req = TrustRequestDto {
            request_id: "req-101".to_string(),
            sender_fp: ident_a.fingerprint.clone(),
            sender_alias: "Alice".to_string(),
            target_fp: ident_b.fingerprint.clone(),
            message: "Hello Bob, please accept my contact request!".to_string(),
            pow_stamp,
            timestamp_ms: current_time_ms(),
            signature: None,
        };

        let handled_req = engine_b
            .handle_trust_request(trust_req.clone(), None)
            .await
            .expect("handle trust request");
        assert_eq!(handled_req.request_id, "req-101");

        // Verify Bob's trust state for Alice is PendingApproval
        assert_eq!(
            engine_b.trust_gate.get_trust_state(&ident_a.fingerprint),
            ContactTrustState::PendingApproval
        );

        // 2. Stranger Alice attempts sending media before approval -> must be rejected
        let media_msg = ChatMessageDto {
            id: "msg-media-1".to_string(),
            conversation_id: format!("conv_{}", ident_a.fingerprint),
            sender_fp: ident_a.fingerprint.clone(),
            recipient_fp: ident_b.fingerprint.clone(),
            timestamp_ms: current_time_ms(),
            content: ChatContentDto::Image {
                file_name: "photo.jpg".into(),
                size_bytes: 1024,
                mime_type: "image/jpeg".into(),
                sha256: "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855".into(),
                thumbnail_base64: None,
            },
            reply_to_id: None,
            signature: None,
        };
        let rej_res = engine_b.handle_chat_message(media_msg, None).await;
        assert!(
            matches!(rej_res, Err(ChatEngineError::Trust(TrustError::MediaNotPermittedForStranger))),
            "Media from unapproved stranger must be rejected"
        );

        // 3. Bob explicitly approves Alice
        engine_b
            .approve_contact(&ident_a.fingerprint)
            .await
            .expect("approve Alice");
        assert_eq!(
            engine_b.trust_gate.get_trust_state(&ident_a.fingerprint),
            ContactTrustState::TrustedContact
        );

        // 4. Alice sends text message -> succeeds and receives cryptographic delivery receipt
        let text_msg = ChatMessageDto {
            id: "msg-text-1".to_string(),
            conversation_id: format!("conv_{}", ident_a.fingerprint),
            sender_fp: ident_a.fingerprint.clone(),
            recipient_fp: ident_b.fingerprint.clone(),
            timestamp_ms: current_time_ms(),
            content: ChatContentDto::Text {
                text: "Glad to connect Bob!".into(),
            },
            reply_to_id: None,
            signature: None,
        };
        let receipt = engine_b
            .handle_chat_message(text_msg, None)
            .await
            .expect("handle text message");
        assert_eq!(receipt.message_id, "msg-text-1");
        assert_eq!(receipt.recipient_fp, ident_b.fingerprint);
        assert!(receipt.signature.is_some());

        // 5. Bob marks message as read -> generates read receipt
        let read_receipt = engine_b
            .mark_message_as_read("msg-text-1")
            .await
            .expect("mark message as read");
        assert_eq!(read_receipt.message_id, "msg-text-1");

        // Verify message delivery state progressed to Read
        let stored = engine_b
            .store
            .get_message("msg-text-1")
            .await
            .expect("get stored message");
        assert_eq!(stored.delivery_state, MessageDeliveryState::Read);

        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }
}
