//! Self-signed certificate and key generation.

/// Generated self-signed TLS identity.
#[derive(Clone, Debug)]
pub struct TlsIdentity {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: String,
}
