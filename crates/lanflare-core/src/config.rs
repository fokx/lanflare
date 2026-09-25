use std::path::PathBuf;

/// Global runtime configuration for LanFlare.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub alias: String,
    pub port: u16,
    pub download_dir: PathBuf,
    pub auto_accept: bool,
    pub pin: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            alias: "LanFlare Node".to_string(),
            port: 53317,
            download_dir: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")),
            auto_accept: false,
            pin: None,
        }
    }
}
