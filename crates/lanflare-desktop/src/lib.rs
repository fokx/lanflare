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

                // Chat Engine event pump: emit chat messages, receipts, and trust requests
                let state_chat = state_setup.clone();
                let app_chat = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    if let Ok(chat) = state_chat.get_or_init_chat_engine().await {
                        let mut rx = chat.subscribe_events();
                        while let Ok(event) = rx.recv().await {
                            match event {
                                lanflare_core::chat::ChatEngineEvent::MessageReceived(msg) => {
                                    let _ = app_chat.emit("lanflare://chat-message", &msg);
                                }
                                lanflare_core::chat::ChatEngineEvent::DeliveryReceiptReceived(r) => {
                                    let _ = app_chat.emit("lanflare://chat-receipt", &r);
                                }
                                lanflare_core::chat::ChatEngineEvent::ReadReceiptReceived(r) => {
                                    let _ = app_chat.emit("lanflare://chat-receipt", &r);
                                }
                                lanflare_core::chat::ChatEngineEvent::TrustRequestReceived(req) => {
                                    let _ = app_chat.emit("lanflare://trust-request", &req);
                                }
                                lanflare_core::chat::ChatEngineEvent::ContactApproved { peer_fp } => {
                                    let _ = app_chat.emit("lanflare://trust-request", serde_json::json!({ "approved": peer_fp }));
                                }
                                lanflare_core::chat::ChatEngineEvent::ContactBlocked { peer_fp } => {
                                    let _ = app_chat.emit("lanflare://trust-request", serde_json::json!({ "blocked": peer_fp }));
                                }
                                lanflare_core::chat::ChatEngineEvent::GroupSynced { group_id, .. } => {
                                    let _ = app_chat.emit("lanflare://chat-message", serde_json::json!({ "groupSynced": group_id }));
                                }
                                _ => {}
                            }
                        }
                    }
                });

                // Personal Device Ring Universal Clipboard event pump
                let state_ring = state_setup.clone();
                let app_ring = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    use tauri::Emitter;
                    let mut rx = state_ring.device_ring.subscribe();
                    while let Ok(event) = rx.recv().await {
                        let _ = app_ring.emit("lanflare://clipboard-synced", &event);
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
            commands::regenerate_identity,
            commands::generate_random_alias,
            commands::get_discovered_peers,
            commands::remove_discovered_peer,
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
            // Sub-phase 11.2: Multi-Hop Mesh & Simulation
            commands::get_mesh_topology,
            commands::start_mesh_simulation,
            commands::toggle_mesh_link,
            commands::run_mesh_traceroute,
            commands::run_mesh_benchmark,
            // Sub-phase 11.2: Encrypted 1:1 & Group Chat
            commands::send_chat_message,
            commands::respond_trust_request,
            commands::list_chat_conversations,
            commands::get_chat_history,
            commands::create_chat_group,
            commands::sync_chat_group,
            // Sub-phase 11.2: LAN Toolbox
            commands::get_clipboard_state,
            commands::broadcast_clipboard,
            commands::search_federated_files,
            commands::get_collab_doc,
            commands::apply_collab_edit,
        ])
        .run(tauri::generate_context!())
        .expect("Error while running LanFlare Desktop application");
}
