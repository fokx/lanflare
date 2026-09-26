use lanflare_proto::{
    DeviceType, InfoResponseDtoV2, MulticastMessageV2, PrepareDownloadResponseDtoV2,
    PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2, ProtocolType, RegisterDtoV2,
    RegisterResponseDtoV2,
};

#[test]
fn test_multicast_official_spec_vectors() {
    // 3.1 Multicast UDP (Default) Announcement from README.md
    let announcement_raw = r#"{
      "alias": "Nice Orange",
      "version": "2.0",
      "deviceModel": "Samsung",
      "deviceType": "mobile",
      "fingerprint": "random string",
      "port": 53317,
      "protocol": "https",
      "download": true,
      "announce": true
    }"#;

    let parsed: MulticastMessageV2 = serde_json::from_str(announcement_raw).unwrap();
    assert_eq!(parsed.alias, "Nice Orange");
    assert_eq!(parsed.version, "2.0");
    assert_eq!(parsed.device_model.as_deref(), Some("Samsung"));
    assert_eq!(parsed.device_type, Some(DeviceType::Mobile));
    assert_eq!(parsed.fingerprint, "random string");
    assert_eq!(parsed.port, 53317);
    assert_eq!(parsed.protocol, ProtocolType::Https);
    assert!(parsed.download);
    assert!(parsed.announce);

    // Multicast UDP Fallback Response from README.md (announce = false)
    let reply_raw = r#"{
      "alias": "Secret Banana",
      "version": "2.0",
      "deviceModel": "Windows",
      "deviceType": "desktop",
      "fingerprint": "random string",
      "port": 53317,
      "protocol": "https",
      "download": true,
      "announce": false
    }"#;

    let parsed_reply: MulticastMessageV2 = serde_json::from_str(reply_raw).unwrap();
    assert_eq!(parsed_reply.alias, "Secret Banana");
    assert!(!parsed_reply.announce);
}

#[test]
fn test_register_official_spec_vectors() {
    // POST /api/localsend/v2/register Request
    let req_raw = r#"{
      "alias": "Secret Banana",
      "version": "2.0",
      "deviceModel": "Windows",
      "deviceType": "desktop",
      "fingerprint": "random string",
      "port": 53317,
      "protocol": "https",
      "download": true
    }"#;

    let req: RegisterDtoV2 = serde_json::from_str(req_raw).unwrap();
    assert_eq!(req.alias, "Secret Banana");
    assert_eq!(req.port, 53317);
    assert_eq!(req.protocol, ProtocolType::Https);

    // POST /api/localsend/v2/register Response
    let resp_raw = r#"{
      "alias": "Nice Orange",
      "version": "2.0",
      "deviceModel": "Samsung",
      "deviceType": "mobile",
      "fingerprint": "random string",
      "download": true
    }"#;

    let resp: RegisterResponseDtoV2 = serde_json::from_str(resp_raw).unwrap();
    assert_eq!(resp.alias, "Nice Orange");
    assert_eq!(resp.device_type, Some(DeviceType::Mobile));
    assert!(resp.download);
}

#[test]
fn test_prepare_upload_official_spec_vectors() {
    // 4.1 Preparation (Metadata Only) POST /api/localsend/v2/prepare-upload Request
    let req_raw = r#"{
      "info": {
        "alias": "Nice Orange",
        "version": "2.0",
        "deviceModel": "Samsung",
        "deviceType": "mobile",
        "fingerprint": "random string",
        "port": 53317,
        "protocol": "https",
        "download": true
      },
      "files": {
        "some file id": {
          "id": "some file id",
          "fileName": "my image.png",
          "size": 324242,
          "fileType": "image/jpeg",
          "sha256": "*sha256 hash*",
          "preview": "*preview data*",
          "metadata": {
            "modified": "2021-01-01T12:34:56Z",
            "accessed": "2021-01-01T12:34:56Z"
          }
        },
        "another file id": {
          "id": "another file id",
          "fileName": "another image.jpg",
          "size": 1234,
          "fileType": "image/jpeg",
          "sha256": "*sha256 hash*",
          "preview": "*preview data*"
        }
      }
    }"#;

    let req: PrepareUploadRequestDtoV2 = serde_json::from_str(req_raw).unwrap();
    assert_eq!(req.info.alias, "Nice Orange");
    assert_eq!(req.files.len(), 2);

    let file1 = req.files.get("some file id").unwrap();
    assert_eq!(file1.file_name, "my image.png");
    assert_eq!(file1.size, 324242);
    let meta = file1.metadata.as_ref().unwrap();
    assert_eq!(meta.modified.as_deref(), Some("2021-01-01T12:34:56Z"));

    // POST /api/localsend/v2/prepare-upload Response
    let resp_raw = r#"{
      "sessionId": "mySessionId",
      "files": {
        "someFileId": "someFileToken",
        "someOtherFileId": "someOtherFileToken"
      }
    }"#;

    let resp: PrepareUploadResponseDtoV2 = serde_json::from_str(resp_raw).unwrap();
    assert_eq!(resp.session_id, "mySessionId");
    assert_eq!(resp.files.get("someFileId").unwrap(), "someFileToken");
}

#[test]
fn test_reverse_file_transfer_spec_vectors() {
    // 5.2 Receive Request POST /api/localsend/v2/prepare-download Response
    let resp_raw = r#"{
      "info": {
        "alias": "Nice Orange",
        "version": "2.0",
        "deviceModel": "Samsung",
        "deviceType": "mobile",
        "fingerprint": "random string",
        "download": true
      },
      "sessionId": "mySessionId",
      "files": {
        "some file id": {
          "id": "some file id",
          "fileName": "my image.png",
          "size": 324242,
          "fileType": "image/jpeg",
          "sha256": "*sha256 hash*",
          "preview": "*preview data*"
        }
      }
    }"#;

    let resp: PrepareDownloadResponseDtoV2 = serde_json::from_str(resp_raw).unwrap();
    assert_eq!(resp.info.alias, "Nice Orange");
    assert_eq!(resp.session_id, "mySessionId");
    assert_eq!(resp.files.len(), 1);
}

#[test]
fn test_info_spec_vector() {
    // 6.1 Info GET /api/localsend/v2/info Response
    let resp_raw = r#"{
      "alias": "Nice Orange",
      "version": "2.0",
      "deviceModel": "Samsung",
      "deviceType": "mobile",
      "fingerprint": "random string",
      "download": true
    }"#;

    let resp: InfoResponseDtoV2 = serde_json::from_str(resp_raw).unwrap();
    assert_eq!(resp.alias, "Nice Orange");
    assert_eq!(resp.fingerprint, "random string");
    assert!(resp.download);
}
