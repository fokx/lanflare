//! Discovery DTOs matching LocalSend v2.2 specification.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The protocol version (major.minor) implemented for the v2 protocol.
pub const PROTOCOL_VERSION_V2: &str = "2.0";

/// Protocol type for file transfers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolType {
    Http,
    Https,
}

impl ProtocolType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProtocolType::Http => "http",
            ProtocolType::Https => "https",
        }
    }
}

/// Device classification according to LocalSend specification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceType {
    Mobile,
    Desktop,
    Web,
    Headless,
    Server,
}

/// Serde helper for `Option<DeviceType>` in the v2 protocol.
///
/// Serializes to lowercase string or null.
/// Deserializes case-insensitively, falling back to `Some(DeviceType::Desktop)`
/// on unknown values as mandated by the LocalSend specification (section 7.1).
pub mod device_type_serde {
    use super::*;

    pub fn serialize<S: Serializer>(
        value: &Option<DeviceType>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(dt) => {
                let s = match dt {
                    DeviceType::Mobile => "mobile",
                    DeviceType::Desktop => "desktop",
                    DeviceType::Web => "web",
                    DeviceType::Headless => "headless",
                    DeviceType::Server => "server",
                };
                serializer.serialize_str(s)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<DeviceType>, D::Error> {
        let opt = Option::<String>::deserialize(deserializer)?;
        Ok(opt.map(|s| match s.to_ascii_lowercase().as_str() {
            "mobile" => DeviceType::Mobile,
            "desktop" => DeviceType::Desktop,
            "web" => DeviceType::Web,
            "headless" => DeviceType::Headless,
            "server" => DeviceType::Server,
            _ => DeviceType::Desktop, // specification fallback requirement
        }))
    }
}

/// Announcement or response message broadcast over UDP multicast (v2.2).
///
/// Default multicast address: 224.0.0.167:53317.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MulticastMessageV2 {
    /// The display name of the device.
    pub alias: String,

    /// Protocol version (e.g. "2.0", "2.2").
    pub version: String,

    /// Device model (e.g. "Samsung", "Windows"). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,

    /// Device type category. Optional, defaults to Desktop on unknown value.
    #[serde(
        default,
        with = "device_type_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,

    /// Fingerprint for device identification.
    pub fingerprint: String,

    /// Port number the device is listening on.
    pub port: u16,

    /// Protocol type (http or https).
    pub protocol: ProtocolType,

    /// Whether download API (sections 5.2, 5.3) is active. Defaults to false.
    #[serde(default)]
    pub download: bool,

    /// True if an announcement requesting responses; false if a multicast reply.
    #[serde(default)]
    pub announce: bool,
}

/// Device registration request DTO sent via POST `/api/localsend/v2/register`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterDtoV2 {
    /// The display name of the device.
    pub alias: String,

    /// Protocol version (e.g. "2.0", "2.2").
    pub version: String,

    /// Device model. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,

    /// Device type category. Optional.
    #[serde(
        default,
        with = "device_type_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,

    /// Fingerprint for device identification (ignored in HTTPS mode).
    pub fingerprint: String,

    /// Port number the device is listening on.
    pub port: u16,

    /// Protocol type (http or https).
    pub protocol: ProtocolType,

    /// Whether download API is active. Defaults to false.
    #[serde(default)]
    pub download: bool,
}

/// Device registration response DTO returned from POST `/api/localsend/v2/register`.
/// Note: Response body does not include port and protocol as the HTTP connection is already established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterResponseDtoV2 {
    /// The display name of the device.
    pub alias: String,

    /// Protocol version (e.g. "2.0", "2.2").
    pub version: String,

    /// Device model. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,

    /// Device type category. Optional.
    #[serde(
        default,
        with = "device_type_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,

    /// Fingerprint for device identification.
    #[serde(default)]
    pub fingerprint: String,

    /// Whether download API is active. Defaults to false.
    #[serde(default)]
    pub download: bool,
}

/// Device information DTO returned from GET `/api/localsend/v2/info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InfoResponseDtoV2 {
    /// The display name of the device.
    pub alias: String,

    /// Protocol version (e.g. "2.0", "2.2").
    pub version: String,

    /// Device model. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_model: Option<String>,

    /// Device type category. Optional.
    #[serde(
        default,
        with = "device_type_serde",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_type: Option<DeviceType>,

    /// Fingerprint for device identification.
    pub fingerprint: String,

    /// Port number the device is listening on (optional in older versions, present in info).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,

    /// Protocol type (http or https) (optional in older versions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProtocolType>,

    /// Whether download API is active. Defaults to false.
    #[serde(default)]
    pub download: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multicast_announcement_serialization() {
        let msg = MulticastMessageV2 {
            alias: "Nice Orange".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Samsung".to_string()),
            device_type: Some(DeviceType::Mobile),
            fingerprint: "random string".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
            announce: true,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""alias":"Nice Orange""#));
        assert!(json.contains(r#""version":"2.0""#));
        assert!(json.contains(r#""deviceModel":"Samsung""#));
        assert!(json.contains(r#""deviceType":"mobile""#));
        assert!(json.contains(r#""fingerprint":"random string""#));
        assert!(json.contains(r#""port":53317"#));
        assert!(json.contains(r#""protocol":"https""#));
        assert!(json.contains(r#""download":true"#));
        assert!(json.contains(r#""announce":true"#));
    }

    #[test]
    fn test_multicast_deserialization_from_spec_vector() {
        let json = r#"{
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

        let msg: MulticastMessageV2 = serde_json::from_str(json).unwrap();
        assert_eq!(msg.alias, "Nice Orange");
        assert_eq!(msg.version, "2.0");
        assert_eq!(msg.device_model.as_deref(), Some("Samsung"));
        assert_eq!(msg.device_type, Some(DeviceType::Mobile));
        assert_eq!(msg.fingerprint, "random string");
        assert_eq!(msg.port, 53317);
        assert_eq!(msg.protocol, ProtocolType::Https);
        assert!(msg.download);
        assert!(msg.announce);
    }

    #[test]
    fn test_multicast_minimal_defaults() {
        let json = r#"{
            "alias": "Headless Server",
            "version": "2.0",
            "fingerprint": "fp123",
            "port": 53317,
            "protocol": "http"
        }"#;

        let msg: MulticastMessageV2 = serde_json::from_str(json).unwrap();
        assert_eq!(msg.alias, "Headless Server");
        assert_eq!(msg.device_model, None);
        assert_eq!(msg.device_type, None);
        assert!(!msg.download);
        assert!(!msg.announce);
    }

    #[test]
    fn test_unknown_device_type_fallback_to_desktop() {
        let json = r#"{
            "alias": "Smart Fridge",
            "version": "2.0",
            "deviceType": "smart_fridge",
            "fingerprint": "xyz",
            "port": 53317,
            "protocol": "http"
        }"#;

        let msg: MulticastMessageV2 = serde_json::from_str(json).unwrap();
        assert_eq!(msg.device_type, Some(DeviceType::Desktop));
    }

    #[test]
    fn test_register_dto_roundtrip() {
        let reg = RegisterDtoV2 {
            alias: "Secret Banana".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Windows".to_string()),
            device_type: Some(DeviceType::Desktop),
            fingerprint: "random string".to_string(),
            port: 53317,
            protocol: ProtocolType::Https,
            download: true,
        };

        let json = serde_json::to_string(&reg).unwrap();
        let decoded: RegisterDtoV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(reg, decoded);
    }

    #[test]
    fn test_register_response_dto_roundtrip() {
        let resp = RegisterResponseDtoV2 {
            alias: "Nice Orange".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Samsung".to_string()),
            device_type: Some(DeviceType::Mobile),
            fingerprint: "random string".to_string(),
            download: true,
        };

        let json = serde_json::to_string(&resp).unwrap();
        let decoded: RegisterResponseDtoV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(resp, decoded);
    }

    #[test]
    fn test_info_response_dto_roundtrip() {
        let info = InfoResponseDtoV2 {
            alias: "Nice Orange".to_string(),
            version: "2.0".to_string(),
            device_model: Some("Samsung".to_string()),
            device_type: Some(DeviceType::Mobile),
            fingerprint: "random string".to_string(),
            port: Some(53317),
            protocol: Some(ProtocolType::Https),
            download: true,
        };

        let json = serde_json::to_string(&info).unwrap();
        let decoded: InfoResponseDtoV2 = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }
}
