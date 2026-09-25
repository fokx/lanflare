//! File transfer DTOs matching LocalSend v2.2 specification.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::discovery::RegisterDtoV2;

/// File metadata sent during transfer preparation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    pub id: String,
    pub file_name: String,
    pub size: u64,
    pub file_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// Request body for POST /api/localsend/v2/prepare-upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadRequestDtoV2 {
    pub info: RegisterDtoV2,
    pub files: HashMap<String, FileDto>,
}

/// Response body for POST /api/localsend/v2/prepare-upload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponseDtoV2 {
    pub session_id: String,
    pub files: HashMap<String, String>,
}
