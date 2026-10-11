use lanflare_desktop::state::DesktopState;
use std::sync::Arc;
use std::path::PathBuf;

#[tokio::test]
async fn test_desktop_state_initialization() {
    let state = DesktopState::new().expect("State init must succeed");
    assert_eq!(state.identity.fingerprint.len(), 64);
    assert_eq!(state.registry.count(), 0);

    let cfg = state.config.read().await;
    assert_eq!(cfg.port, 53317);
    assert!(!cfg.auto_accept);
    assert!(cfg.pin.is_none());
}

#[tokio::test]
async fn test_desktop_config_update() {
    let state = Arc::new(DesktopState::new().expect("State init must succeed"));
    
    // Simulate updating config via payload
    {
        let mut cfg = state.config.write().await;
        cfg.alias = "Custom Device".to_string();
        cfg.port = 54321;
        cfg.auto_accept = true;
        cfg.pin = Some("1234".to_string());
        cfg.download_dir = PathBuf::from("/tmp/lanflare_downloads");
    }

    let cfg = state.config.read().await;
    assert_eq!(cfg.alias, "Custom Device");
    assert_eq!(cfg.port, 54321);
    assert!(cfg.auto_accept);
    assert_eq!(cfg.pin.as_deref(), Some("1234"));
    assert_eq!(cfg.download_dir, PathBuf::from("/tmp/lanflare_downloads"));
}
