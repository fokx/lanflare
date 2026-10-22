use std::path::PathBuf;
use serde::{Deserialize, Serialize};

fn default_save_to_history() -> bool {
    true
}

fn default_multicast_address() -> String {
    "224.0.0.167".to_string()
}

fn default_share_link_auto_accept() -> bool {
    false
}

fn default_share_link_require_pin() -> bool {
    true
}

/// Generates a strong random 10-character password containing no '@', ':', or '/'
pub fn generate_share_link_pin() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut rng = rand::rng();
    (0..10)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn default_share_link_pin() -> String {
    generate_share_link_pin()
}

/// Global runtime configuration for LanFlare.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub alias: String,
    pub port: u16,
    pub download_dir: PathBuf,
    pub auto_accept: bool,
    pub pin: Option<String>,
    #[serde(default = "default_save_to_history")]
    pub save_to_history: bool,
    #[serde(default = "default_multicast_address")]
    pub multicast_address: String,
    #[serde(default = "default_share_link_auto_accept")]
    pub share_link_auto_accept: bool,
    #[serde(default = "default_share_link_require_pin")]
    pub share_link_require_pin: bool,
    #[serde(default = "default_share_link_pin")]
    pub share_link_pin: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        #[cfg(target_os = "android")]
        let download_dir = PathBuf::from("/storage/emulated/0/Download");
        #[cfg(not(target_os = "android"))]
        let download_dir = dirs::download_dir().unwrap_or_else(|| PathBuf::from("."));

        Self {

            alias: "LanFlare Node".to_string(),
            port: 53317,
            download_dir,
            auto_accept: false,

            pin: None,
            save_to_history: true,
            multicast_address: default_multicast_address(),
            share_link_auto_accept: false,
            share_link_require_pin: true,
            share_link_pin: generate_share_link_pin(),
        }
    }
}
