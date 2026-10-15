use std::path::PathBuf;
use serde::{Deserialize, Serialize};

fn default_save_to_history() -> bool {
    true
}

fn default_multicast_address() -> String {
    "224.0.0.167".to_string()
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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            alias: "LanFlare Node".to_string(),
            port: 53317,
            download_dir: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            auto_accept: false,
            pin: None,
            save_to_history: true,
            multicast_address: default_multicast_address(),
        }
    }
}
