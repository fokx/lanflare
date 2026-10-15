//! Hyper router dispatching incoming HTTP requests to LocalSend v2.2 handlers.

use std::net::IpAddr;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use crate::server::handlers::{
    empty_response, handle_cancel, handle_download, handle_info, handle_prepare_download,
    handle_prepare_upload, handle_register, handle_upload, handle_web_portal, ServerState,
};

/// Client connection metadata extracted from the underlying TLS / TCP stream.
#[derive(Clone, Debug)]
pub struct ClientConnectionInfo {
    /// Remote IP address.
    pub remote_ip: IpAddr,
    /// Client certificate SHA-256 fingerprint (if presented in mTLS).
    pub cert_fingerprint: Option<String>,
}

/// Routes incoming HTTP requests to the appropriate handler according to LocalSend v2.2 specification.
pub async fn route_request(
    req: Request<Incoming>,
    client: ClientConnectionInfo,
    state: &ServerState,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method();
    let path = req.uri().path();

    let response = match (method, path) {
        // Web portal for browser download (Share via link)
        (&Method::GET, "/") | (&Method::GET, "/index.html") => {
            handle_web_portal(state).await
        }

        // v1/v2 Device Information: GET /api/localsend/v2/info or GET /api/localsend/v1/info
        (&Method::GET, "/api/localsend/v2/info") | (&Method::GET, "/api/localsend/v1/info") => {
            handle_info(state).await
        }

        // Two-way discovery registration: POST /api/localsend/v2/register
        (&Method::POST, "/api/localsend/v2/register") => {
            handle_register(req, client.remote_ip, client.cert_fingerprint, state).await
        }

        // Transfer session preparation: POST /api/localsend/v2/prepare-upload
        (&Method::POST, "/api/localsend/v2/prepare-upload") => {
            handle_prepare_upload(req, client.remote_ip, client.cert_fingerprint, state).await
        }

        // Binary file upload stream: POST /api/localsend/v2/upload
        (&Method::POST, "/api/localsend/v2/upload") => {
            handle_upload(req, client.remote_ip, state).await
        }

        // Reverse file transfer (Browser download metadata): POST /api/localsend/v2/prepare-download
        (&Method::POST, "/api/localsend/v2/prepare-download") => {
            handle_prepare_download(req, client.remote_ip, state).await
        }

        // Reverse file transfer binary stream: GET /api/localsend/v2/download or GET /download
        (&Method::GET, "/api/localsend/v2/download") | (&Method::GET, "/download") => {
            handle_download(req, client.remote_ip, state).await
        }

        // Transfer cancellation: POST /api/localsend/v2/cancel
        (&Method::POST, "/api/localsend/v2/cancel") => {
            handle_cancel(req, client.remote_ip, state).await
        }

        // Fallback: 404 Not Found
        _ => empty_response(StatusCode::NOT_FOUND),
    };

    Ok(response)
}
