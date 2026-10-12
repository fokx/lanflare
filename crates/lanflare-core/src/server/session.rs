//! Transfer session state and PIN protection for the LocalSend HTTP server.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Instant;
use lanflare_proto::transfer::FileDto;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Maximum failed PIN attempts per IP before temporary lockout (HTTP 429).
pub const MAX_PIN_ATTEMPTS: u32 = 3;

/// Status of an individual file in an active upload session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileTransferStatus {
    /// File is pending upload.
    Pending,
    /// File upload is currently in progress.
    InProgress,
    /// File upload completed and verified successfully.
    Completed,
    /// File upload failed or was rejected.
    Failed,
}

/// Metadata and state for an individual file in an active upload session.
#[derive(Clone, Debug)]
pub struct SessionFile {
    /// File metadata DTO from prepare-upload.
    pub dto: FileDto,
    /// Unique upload token assigned for this file.
    pub token: String,
    /// Current transfer status.
    pub status: FileTransferStatus,
    /// Number of upload attempts.
    pub attempts: u32,
    /// Resolved target path on disk.
    pub target_path: PathBuf,
}

/// A pending session awaiting user decision (or auto-accept).
#[derive(Debug)]
pub struct PendingSession {
    /// Unique session identifier.
    pub session_id: String,
    /// IP address of the sender.
    pub sender_ip: IpAddr,
    /// Certificate fingerprint of sender (if mTLS used).
    pub cert_fingerprint: Option<String>,
    /// Cancellation token if sender disconnects or cancels before decision.
    pub cancel: CancellationToken,
    /// When the pending session was initiated.
    pub created_at: Instant,
}

/// An active upload session with accepted files.
#[derive(Debug)]
pub struct ActiveUploadSession {
    /// Unique session identifier.
    pub session_id: String,
    /// IP address of the sender.
    pub sender_ip: IpAddr,
    /// Certificate fingerprint of sender (if mTLS used).
    pub cert_fingerprint: Option<String>,
    /// Files accepted for upload keyed by file ID.
    pub files: HashMap<String, SessionFile>,
    /// When the session became active.
    pub created_at: Instant,
}

impl ActiveUploadSession {
    /// Checks if all accepted files have reached a terminal state (Completed or Failed).
    pub fn is_finished(&self) -> bool {
        self.files.values().all(|f| {
            matches!(
                f.status,
                FileTransferStatus::Completed | FileTransferStatus::Failed
            )
        })
    }

    /// Checks if all accepted files were successfully completed.
    pub fn all_succeeded(&self) -> bool {
        self.files
            .values()
            .all(|f| f.status == FileTransferStatus::Completed)
    }
}

/// Lifecycle state machine for the server's transfer session slot.
#[derive(Debug)]
pub enum SessionSlotState {
    /// Pending decision from receiver/user.
    Pending(PendingSession),
    /// Active upload session in progress.
    Active(ActiveUploadSession),
}

/// Decision made on an incoming prepare-upload request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareUploadDecision {
    /// Accept specific file IDs (can be a subset or all).
    Accept(Vec<String>),
    /// Reject the upload request.
    Decline,
}

/// Reason why an upload session was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndReason {
    /// All accepted files finished transferring.
    Finished,
    /// Cancelled by the remote sender.
    CancelledBySender,
    /// Cancelled locally by the receiver.
    CancelledLocally,
    /// Session timed out due to inactivity.
    TimedOut,
}

/// Thread-safe PIN attempt tracker with lockout capability.
#[derive(Debug, Default)]
pub struct PinTracker {
    attempts: Mutex<HashMap<IpAddr, u32>>,
}

impl PinTracker {
    pub fn new() -> Self {
        Self {
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// Validates the provided query PIN against the server's configured PIN.
    ///
    /// - If no PIN is configured on the server, Ok(()) is returned immediately.
    /// - If too many failed attempts have occurred from this IP, returns Err(429).
    /// - If the provided PIN matches, resets attempt counter and returns Ok(()).
    /// - If PIN is missing or invalid, increments attempt counter and returns Err(401).
    pub async fn check_pin(
        &self,
        configured_pin: Option<&str>,
        provided_pin: Option<&str>,
        remote_ip: IpAddr,
    ) -> Result<(), hyper::StatusCode> {
        let Some(required) = configured_pin else {
            return Ok(());
        };

        let mut lock = self.attempts.lock().await;
        let count = lock.get(&remote_ip).copied().unwrap_or(0);

        if count >= MAX_PIN_ATTEMPTS {
            return Err(hyper::StatusCode::TOO_MANY_REQUESTS);
        }

        match provided_pin {
            Some(pin) if pin == required => {
                lock.remove(&remote_ip);
                Ok(())
            }
            Some(_) => {
                lock.insert(remote_ip, count + 1);
                Err(hyper::StatusCode::UNAUTHORIZED)
            }
            None => {
                // LocalSend spec: If PIN is required and not provided in query param, return 401
                Err(hyper::StatusCode::UNAUTHORIZED)
            }
        }
    }

    /// Resets the failed attempts for an IP.
    pub async fn reset(&self, ip: IpAddr) {
        self.attempts.lock().await.remove(&ip);
    }
}

/// Manager maintaining session slots, concurrency bounds, and token authentication.
#[derive(Debug)]
pub struct SessionManager {
    /// The current upload session slot (single session active at a time as per LocalSend v2 spec).
    slot: Mutex<Option<SessionSlotState>>,
    /// PIN verification tracker.
    pin_tracker: PinTracker,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            pin_tracker: PinTracker::new(),
        }
    }

    /// Returns a reference to the PIN attempt tracker.
    pub fn pin_tracker(&self) -> &PinTracker {
        &self.pin_tracker
    }

    /// Checks if a session is currently pending or active.
    pub async fn has_active_session(&self) -> bool {
        self.slot.lock().await.is_some()
    }

    /// Claims the session slot for a pending prepare-upload request.
    /// Returns hyper::StatusCode::CONFLICT (409) if a session is already active or pending.
    pub async fn claim_pending(
        &self,
        session_id: String,
        sender_ip: IpAddr,
        cert_fingerprint: Option<String>,
        cancel: CancellationToken,
    ) -> Result<(), hyper::StatusCode> {
        let mut slot = self.slot.lock().await;
        if slot.is_some() {
            return Err(hyper::StatusCode::CONFLICT);
        }

        *slot = Some(SessionSlotState::Pending(PendingSession {
            session_id,
            sender_ip,
            cert_fingerprint,
            cancel,
            created_at: Instant::now(),
        }));
        Ok(())
    }

    /// Releases the pending session if it still matches `session_id`.
    pub async fn clear_pending(&self, session_id: &str) {
        let mut slot = self.slot.lock().await;
        if matches!(slot.as_ref(), Some(SessionSlotState::Pending(p)) if p.session_id == session_id) {
            *slot = None;
        }
    }

    /// Promotes a pending session into an active upload session with accepted files.
    pub async fn activate_session(
        &self,
        session_id: &str,
        sender_ip: IpAddr,
        cert_fingerprint: Option<String>,
        files: HashMap<String, SessionFile>,
    ) -> Result<(), hyper::StatusCode> {
        let mut slot = self.slot.lock().await;
        // Verify slot state is still pending for this session
        match slot.as_ref() {
            Some(SessionSlotState::Pending(p)) if p.session_id == session_id => {}
            _ => return Err(hyper::StatusCode::CONFLICT),
        }

        *slot = Some(SessionSlotState::Active(ActiveUploadSession {
            session_id: session_id.to_string(),
            sender_ip,
            cert_fingerprint,
            files,
            created_at: Instant::now(),
        }));
        Ok(())
    }

    /// Validates upload authorization parameters for `/api/localsend/v2/upload`.
    ///
    /// Verifies:
    /// 1. An active session exists matching `session_id`.
    /// 2. Sender IP matches the session's recorded IP.
    /// 3. The requested `file_id` was accepted and has not completed.
    /// 4. The upload `token` matches the token issued during prepare-upload.
    ///
    /// Returns the corresponding `SessionFile` and sets its status to `InProgress`.
    pub async fn validate_and_start_upload(
        &self,
        session_id: &str,
        file_id: &str,
        token: &str,
        sender_ip: IpAddr,
    ) -> Result<SessionFile, hyper::StatusCode> {
        let mut slot = self.slot.lock().await;
        let Some(SessionSlotState::Active(session)) = slot.as_mut() else {
            return Err(hyper::StatusCode::FORBIDDEN);
        };

        if session.session_id != session_id || session.sender_ip != sender_ip {
            return Err(hyper::StatusCode::FORBIDDEN);
        }

        let Some(file) = session.files.get_mut(file_id) else {
            return Err(hyper::StatusCode::FORBIDDEN);
        };

        if file.token != token {
            return Err(hyper::StatusCode::FORBIDDEN);
        }

        if file.status == FileTransferStatus::Completed {
            // Already finished
            return Err(hyper::StatusCode::BAD_REQUEST);
        }

        file.status = FileTransferStatus::InProgress;
        file.attempts = file.attempts.saturating_add(1);
        Ok(file.clone())
    }

    /// Updates the status of a file in the active session.
    /// If all files reach terminal status, the session is completed and the slot is freed.
    /// Returns whether the entire session finished.
    pub async fn mark_file_status(
        &self,
        session_id: &str,
        file_id: &str,
        status: FileTransferStatus,
    ) -> bool {
        let mut slot = self.slot.lock().await;
        let Some(SessionSlotState::Active(session)) = slot.as_mut() else {
            return false;
        };

        if session.session_id != session_id {
            return false;
        }

        if let Some(file) = session.files.get_mut(file_id) {
            file.status = status;
        }

        if session.is_finished() {
            *slot = None;
            true
        } else {
            false
        }
    }

    /// Cancels the session if the request matches the current pending or active session.
    /// Returns `Some(SessionEndReason)` if an active or pending session was cancelled.
    pub async fn cancel_session(
        &self,
        session_id: Option<&str>,
        remote_ip: IpAddr,
    ) -> Option<SessionEndReason> {
        let mut slot = self.slot.lock().await;
        match slot.as_ref() {
            Some(SessionSlotState::Pending(p))
                if p.sender_ip == remote_ip && session_id.is_none_or(|id| id == p.session_id) =>
            {
                p.cancel.cancel();
                *slot = None;
                Some(SessionEndReason::CancelledBySender)
            }
            Some(SessionSlotState::Active(a))
                if a.sender_ip == remote_ip && session_id.is_some_and(|id| id == a.session_id) =>
            {
                *slot = None;
                Some(SessionEndReason::CancelledBySender)
            }
            _ => None,
        }
    }

    /// Forcibly aborts and clears the current session locally.
    pub async fn abort_locally(&self) -> Option<SessionEndReason> {
        let mut slot = self.slot.lock().await;
        if slot.is_some() {
            *slot = None;
            Some(SessionEndReason::CancelledLocally)
        } else {
            None
        }
    }
}

/// A download file registered with the download manager.
#[derive(Clone, Debug)]
pub struct DownloadFile {
    /// File metadata DTO.
    pub dto: FileDto,
    /// Absolute path to the source file on disk.
    pub file_path: PathBuf,
}

/// An active or accepted reverse transfer download session for a client IP.
#[derive(Debug, Clone)]
pub struct DownloadSession {
    /// Unique session identifier (often client IP string or UUID).
    pub session_id: String,
    /// Client remote IP.
    pub client_ip: IpAddr,
    /// Whether this session has been approved / accepted.
    pub accepted: bool,
    /// When this session was created.
    pub created_at: Instant,
}

/// Manager maintaining registered files and sessions for the reverse transfer / browser download API.
#[derive(Debug, Default)]
pub struct DownloadManager {
    /// Registered files available for downloading, keyed by fileId.
    files: Mutex<HashMap<String, DownloadFile>>,
    /// Active/accepted download sessions keyed by sessionId.
    sessions: Mutex<HashMap<String, DownloadSession>>,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Registers a set of files available for downloading.
    pub async fn set_files(&self, files: Vec<DownloadFile>) {
        let mut map = self.files.lock().await;
        map.clear();
        for file in files {
            map.insert(file.dto.id.clone(), file);
        }
    }

    /// Clears all offered download files and resets download sessions.
    pub async fn clear_files(&self) {
        self.files.lock().await.clear();
        self.sessions.lock().await.clear();
    }

    /// Returns a map of all offered file DTOs.
    pub async fn get_file_dtos(&self) -> HashMap<String, FileDto> {
        let map = self.files.lock().await;
        map.iter()
            .map(|(k, v)| (k.clone(), v.dto.clone()))
            .collect()
    }

    /// Checks if a session with given ID is currently valid and accepted for the given remote IP.
    pub async fn is_session_valid(&self, session_id: &str, client_ip: IpAddr) -> bool {
        let sessions = self.sessions.lock().await;
        sessions
            .get(session_id)
            .is_some_and(|s| s.accepted && s.client_ip == client_ip)
    }

    /// Prepares or accepts a download session for a client.
    /// Returns the session ID.
    pub async fn prepare_session(
        &self,
        requested_session_id: Option<&str>,
        client_ip: IpAddr,
        accepted: bool,
    ) -> String {
        let session_id = requested_session_id
            .map(str::to_string)
            .unwrap_or_else(|| client_ip.to_string());

        let mut sessions = self.sessions.lock().await;
        sessions.insert(
            session_id.clone(),
            DownloadSession {
                session_id: session_id.clone(),
                client_ip,
                accepted,
                created_at: Instant::now(),
            },
        );
        session_id
    }

    /// Retrieves the local filesystem path for a valid file in an authorized session.
    pub async fn get_file_path(
        &self,
        session_id: &str,
        file_id: &str,
        client_ip: IpAddr,
    ) -> Result<PathBuf, hyper::StatusCode> {
        let sessions = self.sessions.lock().await;
        let valid = sessions
            .get(session_id)
            .is_some_and(|s| s.accepted && s.client_ip == client_ip);
        if !valid {
            return Err(hyper::StatusCode::FORBIDDEN);
        }

        let files = self.files.lock().await;
        match files.get(file_id) {
            Some(f) => Ok(f.file_path.clone()),
            None => Err(hyper::StatusCode::NOT_FOUND),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn test_pin_tracker_flow() {
        let tracker = PinTracker::new();
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        // No PIN configured -> always Ok
        assert!(tracker.check_pin(None, None, ip).await.is_ok());
        assert!(tracker.check_pin(None, Some("123"), ip).await.is_ok());

        // PIN configured -> missing -> 401
        assert_eq!(
            tracker.check_pin(Some("1234"), None, ip).await,
            Err(hyper::StatusCode::UNAUTHORIZED)
        );

        // Wrong PIN 1st try -> 401 (count: 1)
        assert_eq!(
            tracker.check_pin(Some("1234"), Some("0000"), ip).await,
            Err(hyper::StatusCode::UNAUTHORIZED)
        );

        // Wrong PIN 2nd try -> 401 (count: 2)
        assert_eq!(
            tracker.check_pin(Some("1234"), Some("0001"), ip).await,
            Err(hyper::StatusCode::UNAUTHORIZED)
        );

        // Wrong PIN 3rd try -> 401 (count: 3)
        assert_eq!(
            tracker.check_pin(Some("1234"), Some("0002"), ip).await,
            Err(hyper::StatusCode::UNAUTHORIZED)
        );

        // 4th try -> 429 lockout triggered (count >= 3)
        assert_eq!(
            tracker.check_pin(Some("1234"), Some("0003"), ip).await,
            Err(hyper::StatusCode::TOO_MANY_REQUESTS)
        );

        // Even correct PIN is blocked once locked out
        assert_eq!(
            tracker.check_pin(Some("1234"), Some("1234"), ip).await,
            Err(hyper::StatusCode::TOO_MANY_REQUESTS)
        );

        // Reset clears lockout
        tracker.reset(ip).await;
        assert!(tracker.check_pin(Some("1234"), Some("1234"), ip).await.is_ok());
    }

    #[tokio::test]
    async fn test_session_slot_concurrency_and_lifecycle() {
        let manager = SessionManager::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        let cancel = CancellationToken::new();

        assert!(!manager.has_active_session().await);

        // Claim slot
        assert!(manager
            .claim_pending("sess-1".to_string(), ip, None, cancel.clone())
            .await
            .is_ok());
        assert!(manager.has_active_session().await);

        // Second claim while pending -> 409 Conflict
        assert_eq!(
            manager
                .claim_pending("sess-2".to_string(), ip, None, CancellationToken::new())
                .await,
            Err(hyper::StatusCode::CONFLICT)
        );

        // Activate session with file
        let mut files = HashMap::new();
        files.insert(
            "file-1".to_string(),
            SessionFile {
                dto: FileDto {
                    id: "file-1".to_string(),
                    file_name: "test.txt".to_string(),
                    size: 100,
                    file_type: "text/plain".to_string(),
                    sha256: None,
                    preview: None,
                    metadata: None,
                },
                token: "token-abc".to_string(),
                status: FileTransferStatus::Pending,
                attempts: 0,
                target_path: PathBuf::from("/tmp/test.txt"),
            },
        );

        assert!(manager
            .activate_session("sess-1", ip, None, files)
            .await
            .is_ok());

        // Validate upload with wrong token -> 403 Forbidden
        assert_eq!(
            manager
                .validate_and_start_upload("sess-1", "file-1", "wrong-tok", ip)
                .await
                .err(),
            Some(hyper::StatusCode::FORBIDDEN)
        );

        // Validate upload with wrong IP -> 403 Forbidden
        let other_ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        assert_eq!(
            manager
                .validate_and_start_upload("sess-1", "file-1", "token-abc", other_ip)
                .await
                .err(),
            Some(hyper::StatusCode::FORBIDDEN)
        );

        // Validate correct upload
        let file = manager
            .validate_and_start_upload("sess-1", "file-1", "token-abc", ip)
            .await
            .expect("should validate");
        assert_eq!(file.dto.file_name, "test.txt");

        // Mark file completed -> finishes session
        let finished = manager
            .mark_file_status("sess-1", "file-1", FileTransferStatus::Completed)
            .await;
        assert!(finished);
        assert!(!manager.has_active_session().await);
    }

    #[tokio::test]
    async fn test_cancel_pending_session() {
        let manager = SessionManager::new();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
        let cancel = CancellationToken::new();

        manager
            .claim_pending("sess-1".to_string(), ip, None, cancel.clone())
            .await
            .unwrap();

        let reason = manager.cancel_session(None, ip).await;
        assert_eq!(reason, Some(SessionEndReason::CancelledBySender));
        assert!(cancel.is_cancelled());
        assert!(!manager.has_active_session().await);
    }
}
