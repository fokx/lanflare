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

/// Generates a friendly, two-word random device alias (e.g. "Golden Mango", "Sweet Apple").
/// Avoids awkward, funny, or ambiguous words.
pub fn generate_random_alias() -> String {
    use rand::Rng;
    const ADJECTIVES: &[&str] = &[
        "Amber", "Azure", "Brave", "Bright", "Brisk", "Calm", "Clever", "Cool",
        "Cosmic", "Crispy", "Crystal", "Dapper", "Eager", "Emerald", "Frosty", "Gentle",
        "Golden", "Grand", "Green", "Happy", "Jolly", "Kind", "Lucky", "Lunar",
        "Mellow", "Misty", "Noble", "Pacific", "Peaceful", "Placid", "Proud", "Quiet",
        "Radiant", "Rosy", "Ruby", "Sapphire", "Serene", "Silent", "Silver", "Simple",
        "Solar", "Starry", "Sturdy", "Sunny", "Sweet", "Swift", "Tender", "Tranquil",
        "Valiant", "Velvet", "Vibrant", "Warm", "Wild", "Wise",
    ];

    const NOUNS: &[&str] = &[
        "Apple", "Aurora", "Autumn", "Bamboo", "Beacon", "Berry", "Blossom", "Breeze",
        "Brook", "Canyon", "Cedar", "Cherry", "Clover", "Comet", "Coral", "Cottage",
        "Creek", "Cypress", "Dawn", "Dolphin", "Echo", "Falcon", "Fern", "Finch",
        "Forest", "Garden", "Glade", "Grove", "Harbor", "Haven", "Hazel", "Island",
        "Lagoon", "Lark", "Leaf", "Lemon", "Lotus", "Mango", "Maple", "Meadow",
        "Melon", "Moon", "Mountain", "Oasis", "Ocean", "Orchard", "Orchid", "Otter",
        "Peach", "Peak", "Pebble", "Penguin", "Pine", "Planet", "Plum", "Prairie",
        "Rainbow", "Raven", "Ray", "Reef", "River", "Robin", "Rose", "Sail",
        "Sequoia", "Sky", "Sparrow", "Spring", "Star", "Stream", "Summit", "Sunrise",
        "Sunset", "Swallow", "Tea", "Tulip", "Valley", "Willow", "Wind",
    ];

    let mut rng = rand::rng();
    let adj = ADJECTIVES[rng.random_range(0..ADJECTIVES.len())];
    let noun = NOUNS[rng.random_range(0..NOUNS.len())];
    format!("{adj} {noun}")
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
            alias: generate_random_alias(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_random_alias_structure() {
        for _ in 0..20 {
            let alias = generate_random_alias();
            let parts: Vec<&str> = alias.split_whitespace().collect();
            assert_eq!(parts.len(), 2, "Alias must consist of exactly two words: {alias}");
            assert!(!parts[0].is_empty());
            assert!(!parts[1].is_empty());
        }
    }
}
