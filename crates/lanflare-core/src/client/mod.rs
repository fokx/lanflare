//! HTTP/HTTPS Client & Transfer Orchestrator implementing LocalSend v2.2 specification.

pub mod transfer;

use std::sync::Arc;
use std::time::Duration;
use reqwest::Client;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use thiserror::Error;

use lanflare_proto::discovery::{InfoResponseDtoV2, RegisterDtoV2, RegisterResponseDtoV2};
use lanflare_proto::transfer::{
    PrepareDownloadResponseDtoV2, PrepareUploadRequestDtoV2, PrepareUploadResponseDtoV2,
};
use crate::crypto::cert::TlsIdentity;
use crate::crypto::verifier::PinnedServerCertVerifier;

/// Errors arising from LocalSend client operations.
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("Network / HTTP request error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("TLS configuration error: {0}")]
    Tls(String),

    #[error("Server returned rejection or error HTTP status: {status} ({body})")]
    ServerError {
        status: reqwest::StatusCode,
        body: String,
    },

    #[error("Session preparation was rejected by the receiver")]
    SessionRejected,

    #[error("Recipient required a PIN or provided PIN was invalid (HTTP 401 Unauthorized)")]
    UnauthorizedPin,

    #[error("Recipient PIN attempts exceeded; rate limited (HTTP 429 Too Many Requests)")]
    PinRateLimited,

    #[error("Recipient transfer slot is busy with another session (HTTP 409 Conflict)")]
    SessionConflict,

    #[error("File upload failed: HTTP {status} ({reason})")]
    UploadFailed {
        status: reqwest::StatusCode,
        reason: String,
    },

    #[error("I/O error reading file: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
}

/// Configuration options for building an [`LsClient`].
#[derive(Clone, Debug)]
pub struct LsClientConfig {
    /// Client timeout for requests (default: 30s).
    pub timeout: Duration,
    /// Connect timeout (default: 5s).
    pub connect_timeout: Duration,
    /// Local TLS identity used for mTLS client authentication.
    pub tls_identity: Option<TlsIdentity>,
    /// Pinned SHA-256 server certificate fingerprint (expected from receiver).
    pub target_fingerprint: Option<String>,
}

impl Default for LsClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            tls_identity: None,
            target_fingerprint: None,
        }
    }
}

/// HTTP/HTTPS client tailored for the LocalSend v2.2 wire protocol.
#[derive(Clone)]
pub struct LsClient {
    client: Client,
    target_fingerprint: Option<String>,
}

impl LsClient {
    /// Constructs a new [`LsClient`] with the provided configuration.
    pub fn new(config: LsClientConfig) -> Result<Self, ClientError> {
        let mut builder = Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .tcp_nodelay(true);

        if config.tls_identity.is_some() || config.target_fingerprint.is_some() {
            // Build custom rustls ClientConfig with PinnedServerCertVerifier
            let dummy_cert_pem = match &config.tls_identity {
                Some(ident) => ident.cert_pem.clone(),
                None => {
                    // Generate minimal ephemeral self-signed cert to populate root store
                    let ephemeral = crate::crypto::cert::generate_self_signed()
                        .map_err(|e| ClientError::Tls(e.to_string()))?;
                    ephemeral.cert_pem
                }
            };

            let server_verifier = Arc::new(
                PinnedServerCertVerifier::new(&dummy_cert_pem, config.target_fingerprint.clone())
                    .map_err(|e| ClientError::Tls(e.to_string()))?,
            );

            let rustls_builder = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(server_verifier);

            let rustls_config = match &config.tls_identity {
                Some(ident) => {
                    let cert_der = CertificateDer::from_pem_slice(ident.cert_pem.as_bytes())
                        .map_err(|e| ClientError::Tls(format!("Failed to parse client cert DER: {e}")))?;
                    let key_der = PrivateKeyDer::from_pem_slice(ident.key_pem.as_bytes())
                        .map_err(|e| ClientError::Tls(format!("Failed to parse client key DER: {e}")))?;

                    rustls_builder
                        .with_client_auth_cert(vec![cert_der], key_der)
                        .map_err(|e| ClientError::Tls(format!("Failed to set client auth cert: {e}")))?
                }
                None => rustls_builder.with_no_client_auth(),
            };

            builder = builder.use_preconfigured_tls(rustls_config);
        }

        let client = builder.build().map_err(ClientError::Reqwest)?;
        Ok(Self {
            client,
            target_fingerprint: config.target_fingerprint,
        })
    }

    /// Access the underlying `reqwest::Client`.
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// The pinned target fingerprint, if any.
    pub fn target_fingerprint(&self) -> Option<&str> {
        self.target_fingerprint.as_deref()
    }

    /// Fetches remote peer info via `GET /api/localsend/v2/info` (or `/api/localsend/v1/info`).
    pub async fn get_info(&self, base_url: &str) -> Result<InfoResponseDtoV2, ClientError> {
        let url = format!("{}/api/localsend/v2/info", base_url.trim_end_matches('/'));
        let resp = self.client.get(&url).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::ServerError { status, body });
        }

        let info: InfoResponseDtoV2 = resp.json().await?;
        Ok(info)
    }

    /// Registers this device with the remote peer via `POST /api/localsend/v2/register`.
    pub async fn register(
        &self,
        base_url: &str,
        dto: &RegisterDtoV2,
    ) -> Result<RegisterResponseDtoV2, ClientError> {
        let url = format!("{}/api/localsend/v2/register", base_url.trim_end_matches('/'));
        let resp = self.client.post(&url).json(dto).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ClientError::ServerError { status, body });
        }

        let reg_resp: RegisterResponseDtoV2 = resp.json().await?;
        Ok(reg_resp)
    }

    /// Initiates a transfer session via `POST /api/localsend/v2/prepare-upload`.
    pub async fn prepare_upload(
        &self,
        base_url: &str,
        request: &PrepareUploadRequestDtoV2,
        pin: Option<&str>,
    ) -> Result<PrepareUploadResponseDtoV2, ClientError> {
        let mut url = format!("{}/api/localsend/v2/prepare-upload", base_url.trim_end_matches('/'));
        if let Some(p) = pin {
            url.push_str("?pin=");
            url.push_str(&urlencoding(p));
        }

        let resp = self.client.post(&url).json(request).send().await?;
        let status = resp.status();

        match status {
            reqwest::StatusCode::OK => {
                let prep_resp: PrepareUploadResponseDtoV2 = resp.json().await?;
                if prep_resp.files.is_empty() {
                    return Err(ClientError::SessionRejected);
                }
                Ok(prep_resp)
            }
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                Err(ClientError::UnauthorizedPin)
            }
            reqwest::StatusCode::TOO_MANY_REQUESTS => Err(ClientError::PinRateLimited),
            reqwest::StatusCode::CONFLICT => Err(ClientError::SessionConflict),
            _ => {
                let body = resp.text().await.unwrap_or_default();
                Err(ClientError::ServerError { status, body })
            }
        }
    }

    /// Uploads an individual file stream via `POST /api/localsend/v2/upload?sessionId=...&fileId=...&token=...`.
    pub async fn upload_file<B>(
        &self,
        base_url: &str,
        session_id: &str,
        file_id: &str,
        token: &str,
        body: B,
        content_length: u64,
    ) -> Result<(), ClientError>
    where
        B: Into<reqwest::Body>,
    {
        let url = format!(
            "{}/api/localsend/v2/upload?sessionId={}&fileId={}&token={}",
            base_url.trim_end_matches('/'),
            urlencoding(session_id),
            urlencoding(file_id),
            urlencoding(token)
        );

        let resp = self
            .client
            .post(&url)
            .header(reqwest::header::CONTENT_LENGTH, content_length)
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/octet-stream",
            )
            .body(body)
            .send()
            .await?;

        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let reason = resp.text().await.unwrap_or_default();
            Err(ClientError::UploadFailed { status, reason })
        }
    }

    /// Cancels an active or pending session via `POST /api/localsend/v2/cancel?sessionId=...`.
    pub async fn cancel_session(
        &self,
        base_url: &str,
        session_id: &str,
    ) -> Result<(), ClientError> {
        let url = format!(
            "{}/api/localsend/v2/cancel?sessionId={}",
            base_url.trim_end_matches('/'),
            urlencoding(session_id)
        );

        let resp = self.client.post(&url).send().await?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::ServerError { status, body })
        }
    }

    /// Prepares to download files from a sender hosting them (Browser Download / Reverse Transfer API).
    /// Calls `POST /api/localsend/v2/prepare-download`.
    pub async fn prepare_download(
        &self,
        base_url: &str,
        session_id: Option<&str>,
        pin: Option<&str>,
    ) -> Result<PrepareDownloadResponseDtoV2, ClientError> {
        let mut url = format!("{}/api/localsend/v2/prepare-download", base_url.trim_end_matches('/'));
        let mut params = Vec::new();
        if let Some(sid) = session_id {
            params.push(format!("sessionId={}", urlencoding(sid)));
        }
        if let Some(p) = pin {
            params.push(format!("pin={}", urlencoding(p)));
        }
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        let resp = self.client.post(&url).send().await?;
        let status = resp.status();

        match status {
            reqwest::StatusCode::OK => {
                let prep_resp: PrepareDownloadResponseDtoV2 = resp.json().await?;
                Ok(prep_resp)
            }
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
                Err(ClientError::UnauthorizedPin)
            }
            reqwest::StatusCode::TOO_MANY_REQUESTS => Err(ClientError::PinRateLimited),
            _ => {
                let body = resp.text().await.unwrap_or_default();
                Err(ClientError::ServerError { status, body })
            }
        }
    }

    /// Downloads a file binary stream from a sender hosting it (Reverse Transfer API).
    /// Calls `GET /api/localsend/v2/download?sessionId=...&fileId=...`.
    pub async fn download_file(
        &self,
        base_url: &str,
        session_id: &str,
        file_id: &str,
    ) -> Result<reqwest::Response, ClientError> {
        let url = format!(
            "{}/api/localsend/v2/download?sessionId={}&fileId={}",
            base_url.trim_end_matches('/'),
            urlencoding(session_id),
            urlencoding(file_id)
        );

        let resp = self.client.get(&url).send().await?;
        let status = resp.status();

        if status.is_success() {
            Ok(resp)
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(ClientError::ServerError { status, body })
        }
    }
}

/// Minimal percent-encoding helper for query params without pulling extra dependencies.
fn urlencoding(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ls_client_builder_defaults() {
        let config = LsClientConfig::default();
        let client = LsClient::new(config).expect("Default client should build cleanly");
        assert!(client.target_fingerprint().is_none());
    }

    #[tokio::test]
    async fn test_ls_client_with_tls_identity_and_pinned_fingerprint() {
        let ident = crate::crypto::cert::generate_self_signed().expect("Cert generation should succeed");
        let fp = ident.fingerprint.clone();

        let config = LsClientConfig {
            timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(3),
            tls_identity: Some(ident),
            target_fingerprint: Some(fp.clone()),
        };

        let client = LsClient::new(config).expect("Client with TLS and fingerprint should build cleanly");
        assert_eq!(client.target_fingerprint(), Some(fp.as_str()));
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("abc_123.-~"), "abc_123.-~");
        assert_eq!(urlencoding("session/123?x=1"), "session%2F123%3Fx%3D1");
    }
}

