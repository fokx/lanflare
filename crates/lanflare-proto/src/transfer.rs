//! File transfer DTOs matching LocalSend v2.2 specification.

use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use crate::discovery::{InfoResponseDtoV2, RegisterDtoV2};

/// File metadata sent during transfer preparation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    /// Unique identifier for the file in the transfer session.
    pub id: String,

    /// Name of the file including extension.
    pub file_name: String,

    /// Size of the file in bytes.
    pub size: u64,

    /// MIME type or general classification (e.g., "image/jpeg", "application/pdf").
    pub file_type: String,

    /// Optional SHA-256 hash of the file payload for data integrity verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,

    /// Optional thumbnail or preview data string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,

    /// Optional file timestamps or additional metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<FileMetadataDto>,
}

/// Additional file metadata (timestamps in RFC 3339 format).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadataDto {
    /// Last modified timestamp (e.g., "2021-01-01T12:34:56Z").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,

    /// Last accessed timestamp (e.g., "2021-01-01T12:34:56Z").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessed: Option<String>,
}

/// Request body for POST `/api/localsend/v2/prepare-upload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadRequestDtoV2 {
    /// Sender's device information.
    pub info: RegisterDtoV2,

    /// Map of file ID to file metadata.
    pub files: HashMap<String, FileDto>,
}

/// Response body for POST `/api/localsend/v2/prepare-upload`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareUploadResponseDtoV2 {
    /// Session ID assigned by the receiver for this upload session.
    pub session_id: String,

    /// Map of accepted file ID to upload authentication token.
    /// Only contains files accepted by the receiver.
    pub files: HashMap<String, String>,
}

/// Response body for POST `/api/localsend/v2/prepare-download` (Reverse File Transfer / Download API).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareDownloadResponseDtoV2 {
    /// Sender's device info.
    pub info: InfoResponseDtoV2,

    /// Session ID for the download session.
    pub session_id: String,

    /// Map of file ID to file metadata available for download.
    pub files: HashMap<String, FileDto>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::{DeviceType, ProtocolType};

    #[test]
    fn test_prepare_upload_request_serialization_spec_vector() {
        let mut files = HashMap::new();
        files.insert(
            "file-1".to_string(),
            FileDto {
                id: "file-1".to_string(),
                file_name: "my image.png".to_string(),
                size: 324242,
                file_type: "image/jpeg".to_string(),
                sha256: Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string()),
                preview: Some("preview_base64_data".to_string()),
                metadata: Some(FileMetadataDto {
                    modified: Some("2021-01-01T12:34:56Z".to_string()),
                    accessed: Some("2021-01-01T12:34:56Z".to_string()),
                }),
            },
        );

        let req = PrepareUploadRequestDtoV2 {
            info: RegisterDtoV2 {
                alias: "Nice Orange".to_string(),
                version: "2.0".to_string(),
                device_model: Some("Samsung".to_string()),
                device_type: Some(DeviceType::Mobile),
                fingerprint: "random string".to_string(),
                port: 53317,
                protocol: ProtocolType::Https,
                download: true,
            },
            files,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""fileName":"my image.png""#));
        assert!(json.contains(r#""fileType":"image/jpeg""#));
        assert!(json.contains(r#""size":324242"#));
        assert!(json.contains(r#""modified":"2021-01-01T12:34:56Z""#));

        let decoded: PrepareUploadRequestDtoV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(req, decoded);
    }

    #[test]
    fn test_prepare_upload_response_deserialization_spec_vector() {
        let json = r#"{
            "sessionId": "mySessionId",
            "files": {
                "someFileId": "someFileToken",
                "someOtherFileId": "someOtherFileToken"
            }
        }"#;

        let resp: PrepareUploadResponseDtoV2 = serde_json::from_str(json).unwrap();
        assert_eq!(resp.session_id, "mySessionId");
        assert_eq!(resp.files.len(), 2);
        assert_eq!(resp.files.get("someFileId").unwrap(), "someFileToken");
        assert_eq!(resp.files.get("someOtherFileId").unwrap(), "someOtherFileToken");
    }

    #[test]
    fn test_prepare_download_response_roundtrip() {
        let mut files = HashMap::new();
        files.insert(
            "dl-1".to_string(),
            FileDto {
                id: "dl-1".to_string(),
                file_name: "document.pdf".to_string(),
                size: 1048576,
                file_type: "application/pdf".to_string(),
                sha256: None,
                preview: None,
                metadata: None,
            },
        );

        let resp = PrepareDownloadResponseDtoV2 {
            info: InfoResponseDtoV2 {
                alias: "Sender Laptop".to_string(),
                version: "2.0".to_string(),
                device_model: Some("MacBook".to_string()),
                device_type: Some(DeviceType::Desktop),
                fingerprint: "cert-hash-123".to_string(),
                port: Some(53317),
                protocol: Some(ProtocolType::Http),
                download: true,
            },
            session_id: "dl-session-99".to_string(),
            files,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let decoded: PrepareDownloadResponseDtoV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }
}
