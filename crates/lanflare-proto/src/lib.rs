//! Protocol definitions and data transfer objects for LanFlare,
//! fully compatible with LocalSend v2.2 specification.

pub mod chat;
pub mod discovery;
pub mod error;
pub mod mesh;
pub mod toolbox;
pub mod transfer;

// Convenience re-exports
pub use discovery::{
    DeviceType, InfoResponseDtoV2, MulticastMessageV2, ProtocolType, RegisterDtoV2,
    RegisterResponseDtoV2, PROTOCOL_VERSION_V2,
};
pub use error::ProtocolError;
pub use transfer::{
    FileDto, FileMetadataDto, PrepareDownloadResponseDtoV2, PrepareUploadRequestDtoV2,
    PrepareUploadResponseDtoV2,
};
