//! Protocol definitions and data transfer objects for LanFlare,
//! fully compatible with LocalSend v2.2 specification.

pub mod chat;
pub mod discovery;
pub mod error;
pub mod mesh;
pub mod toolbox;
pub mod transfer;

// Convenience re-exports
pub use chat::{
    ChatContentDto, ChatMessageDto, DeliveryReceiptDto, GroupDagNodeDto, GroupKeyDistributionDto,
    GroupMember, GroupMembershipRoster, GroupRole, GroupSyncRequestDto, GroupSyncResponseDto,
    HashcashStamp, HlcTimestamp, MessageDeliveryState, ReadReceiptDto, ResumeTransferDto,
    TrustRequestDto, DEFAULT_POW_DIFFICULTY_BITS, DEFAULT_TIME_BUCKET_SECS, MAX_INTRO_MESSAGE_LEN,
};
pub use discovery::{
    DeviceType, InfoResponseDtoV2, MulticastMessageV2, ProtocolType, RegisterDtoV2,
    RegisterResponseDtoV2, PROTOCOL_VERSION_V2,
};
pub use error::ProtocolError;
pub use mesh::{
    DEFAULT_HOP_PENALTY_MS, DEFAULT_STREAM_CHUNK_SIZE, E2eeEnvelope, MAX_MESH_TTL,
    MailboxDepositRequest, MailboxDepositResponse, MailboxFetchRequest, MailboxFetchResponse,
    MailboxItemDto, MeshGossipPacket, MeshLinkMetric, MeshNodeAnnouncement,
    MeshRouteAdvertisement, MeshStreamHeader, RoutedEnvelope,
};
pub use transfer::{
    FileDto, FileMetadataDto, PrepareDownloadResponseDtoV2, PrepareUploadRequestDtoV2,
    PrepareUploadResponseDtoV2,
};
