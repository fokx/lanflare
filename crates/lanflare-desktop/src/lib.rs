//! LanFlare Desktop Application library crate.

pub mod commands;
pub mod state;
#[cfg(desktop)]
pub mod tray;

use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

pub use commands::*;
pub use state::DesktopState;

/// Entry point to configure and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lanflare=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let state = Arc::new(DesktopState::new().expect("Failed to initialize LanFlare desktop state"));

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init());

    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_mobile_sharetarget::init());
    }

    builder
        .manage(state.clone())

        .setup({
            let state_setup = state.clone();
            move |app| {
                use tauri::Manager;
                let app_h = app.handle();
                let config_dir = app_h.path().app_config_dir().ok();
                let data_dir = app_h.path().app_data_dir().ok();
                #[cfg(target_os = "android")]
                let download_dir = Some(std::path::PathBuf::from("/storage/emulated/0/Download"));
                #[cfg(not(target_os = "android"))]
                let download_dir = app_h.path().download_dir().ok();
                let state_paths = state_setup.clone();

                tauri::async_runtime::spawn(async move {
                    state_paths.init_paths(config_dir, data_dir, download_dir).await;
                });

                let registry = state_setup.registry.clone();
                tauri::async_runtime::spawn(async move {
                    registry.start_eviction_task();
                });

                // Listen to registry events (added, updated, expired, removed) and emit to GUI
                let mut reg_rx = state_setup.registry.subscribe();
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    while let Ok(_event) = reg_rx.recv().await {
                        let _ = app_handle.emit("lanflare://peers-updated", ());
                    }
                });

                #[cfg(desktop)]
                tray::setup_tray(app.handle(), state_setup)?;
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
            commands::create_temp_text_file,
            commands::offer_download_files,
            commands::clear_download_files,
            commands::get_download_files,
            commands::get_transfer_history,
            commands::save_transfer_history,
            commands::clear_transfer_history,
            commands::read_clipboard,
            commands::save_clipboard_image,
            commands::regenerate_share_link_pin,
            commands::respond_browser_download,
            commands::open_path_in_system,
            commands::reveal_in_folder,
            commands::get_common_directories,
            commands::list_directory,
            commands::start_dufs_share,
            commands::stop_dufs_share,
            commands::get_dufs_share_status,
        ])
        .run(tauri::generate_context!())
        .expect("Error while running LanFlare Desktop application");
}
