//! Hyper router dispatching incoming HTTP requests to LocalSend v2.2 and Dufs file server handlers.

use std::net::IpAddr;
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Method, Request, Response, StatusCode};

use crate::server::dufs::{full_to_box_body, handle_dufs_request, ServerResponseBody};
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

fn to_box_body(res: Response<Full<Bytes>>) -> Response<ServerResponseBody> {
    res.map(full_to_box_body)
}

/// Routes incoming HTTP requests to the appropriate handler according to LocalSend v2.2 and Dufs specifications.
pub async fn route_request(
    req: Request<Incoming>,
    client: ClientConnectionInfo,
    state: &ServerState,
) -> Result<Response<ServerResponseBody>, hyper::Error> {
    let method = req.method();
    let path = req.uri().path();

    // Check if Dufs folder sharing is active or if explicit /dufs path is requested
    let is_mesh_api = path.starts_with("/api/lanflare/v1/mesh/");
    let is_chat_api = path.starts_with("/api/lanflare/v1/chat/");
    let is_resume_api = path == "/api/lanflare/v1/transfer/resume";
    let is_toolbox_api = path.starts_with("/api/lanflare/v1/toolbox/");
    if let Some(dufs_cfg) = state.dufs_manager.get_config().await {
        let is_dufs_explicit = path == "/dufs" || path.starts_with("/dufs/");
        let is_localsend_api = path.starts_with("/api/localsend/");
        let is_download_endpoint = path == "/download" || path == "/api/localsend/v2/download";

        // Route to Dufs if explicit /dufs prefix, or if root/static file access while dufs is active
        if is_dufs_explicit || (!is_localsend_api && !is_download_endpoint && !is_mesh_api && !is_chat_api && !is_resume_api && !is_toolbox_api) {
            let mut cfg = dufs_cfg;
            if is_dufs_explicit {
                cfg.path_prefix = "/dufs".to_string();
            } else {
                cfg.path_prefix = String::new();
            }
            return handle_dufs_request(req, client.remote_ip, &cfg).await;
        }
    }

    if is_mesh_api {
        if let Some(mesh_router) = &state.mesh_router {
            return Ok(mesh_router.handle_hyper_request(req, Some(client)).await);
        } else {
            return Ok(to_box_body(empty_response(StatusCode::NOT_FOUND)));
        }
    }

    if is_chat_api || is_resume_api {
        if let Some(chat_engine) = &state.chat_engine {
            return Ok(chat_engine.handle_hyper_request(req, Some(client)).await);
        } else {
            return Ok(to_box_body(empty_response(StatusCode::NOT_FOUND)));
        }
    }

    if is_toolbox_api {
        if let Some(device_ring) = &state.device_ring {
            return Ok(device_ring.handle_hyper_request(req, Some(client)).await);
        } else {
            return Ok(to_box_body(empty_response(StatusCode::NOT_FOUND)));
        }
    }

    let response = match (method, path) {
        // Web portal for browser download (Share via link) when dufs is not active
        (&Method::GET, "/") | (&Method::GET, "/index.html") => {
            to_box_body(handle_web_portal(&req, state).await)
        }

        // v1/v2 Device Information: GET /api/localsend/v2/info or GET /api/localsend/v1/info
        (&Method::GET, "/api/localsend/v2/info") | (&Method::GET, "/api/localsend/v1/info") => {
            to_box_body(handle_info(client.remote_ip, state).await)
        }

        // Two-way discovery registration: POST /api/localsend/v2/register
        (&Method::POST, "/api/localsend/v2/register") => {
            to_box_body(handle_register(req, client.remote_ip, client.cert_fingerprint, state).await)
        }

        // Transfer session preparation: POST /api/localsend/v2/prepare-upload
        (&Method::POST, "/api/localsend/v2/prepare-upload") => {
            to_box_body(handle_prepare_upload(req, client.remote_ip, client.cert_fingerprint, state).await)
        }

        // Binary file upload stream: POST /api/localsend/v2/upload
        (&Method::POST, "/api/localsend/v2/upload") => {
            to_box_body(handle_upload(req, client.remote_ip, state).await)
        }

        // Reverse file transfer (Browser download metadata): POST /api/localsend/v2/prepare-download
        (&Method::POST, "/api/localsend/v2/prepare-download") => {
            to_box_body(handle_prepare_download(req, client.remote_ip, state).await)
        }

        // Reverse file transfer binary stream: GET /api/localsend/v2/download or GET /download
        (&Method::GET, "/api/localsend/v2/download") | (&Method::GET, "/download") => {
            to_box_body(handle_download(req, client.remote_ip, state).await)
        }

        // Transfer cancellation: POST /api/localsend/v2/cancel
        (&Method::POST, "/api/localsend/v2/cancel") => {
            to_box_body(handle_cancel(req, client.remote_ip, state).await)
        }

        // Fallback: 404 Not Found
        _ => to_box_body(empty_response(StatusCode::NOT_FOUND)),
    };

    Ok(response)
}
