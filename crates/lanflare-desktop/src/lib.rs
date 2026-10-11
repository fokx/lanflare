//! LanFlare Desktop Application library crate.

pub mod commands;
pub mod state;
pub mod tray;

use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub use commands::*;
pub use state::DesktopState;

/// Entry point to configure and run the Tauri application.
pub fn run() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lanflare=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = Arc::new(DesktopState::new().expect("Failed to initialize LanFlare desktop state"));

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state.clone())
        .setup({
            let state_tray = state.clone();
            move |app| {
                tray::setup_tray(app.handle(), state_tray)?;
                Ok(())
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_node_status,
            commands::update_config,
            commands::get_discovered_peers,
            commands::start_discovery,
            commands::announce_presence,
            commands::stop_discovery,
            commands::start_receiver_server,
            commands::respond_incoming_transfer,
            commands::stop_receiver_server,
            commands::send_files,
            commands::cancel_outgoing_transfer,
        ])
        .run(tauri::generate_context!())
        .expect("Error while running LanFlare Desktop application");
}
