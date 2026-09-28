//! Custom certificate verifier for fingerprint pinning without CA trust roots.

use crate::crypto::cert::{fingerprint_from_cert_der, verify_cert_from_der};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, OtherError, RootCertStore,
    SignatureScheme,
};
use std::fmt::{Debug, Display, Formatter};
use std::sync::Arc;

/// The reason a peer certificate was rejected, preserving a descriptive error chain.
#[derive(Clone, Eq, PartialEq)]
pub struct RejectedCert(pub String);

impl Display for RejectedCert {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Debug for RejectedCert {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RejectedCert {}

fn rejected(reason: String) -> Error {
    tracing::warn!("{reason}");
    Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(RejectedCert(
        reason,
    )))))
}

/// Verifies server certificates against an optional pinned SHA-256 fingerprint.
///
/// LocalSend peers use self-signed certificates without PKI CAs. Peer identity is defined
/// by the uppercase-hex SHA-256 fingerprint of the DER certificate.
///
/// When `expected_fingerprint` is `Some`, any certificate whose fingerprint differs is
/// immediately rejected with a descriptive error.
/// When `expected_fingerprint` is `None`, any cryptographically valid self-signed / peer
/// certificate is accepted (Trust On First Use, for discovery probes).
#[derive(Debug)]
pub struct PinnedServerCertVerifier {
    inner: Arc<dyn ServerCertVerifier>,
    expected_fingerprint: Option<String>,
}

impl PinnedServerCertVerifier {
    /// Creates a new pinned server certificate verifier.
    ///
    /// `own_cert_pem` is added to a local dummy root store so that the underlying signature
    /// validator initializes properly without an empty-root error. It is never used as a trust CA.
    pub fn new(own_cert_pem: &str, expected_fingerprint: Option<String>) -> anyhow::Result<Self> {
        let mut root_store = RootCertStore::empty();
        root_store.add(CertificateDer::from_pem_slice(own_cert_pem.as_bytes())?)?;

        let inner = WebPkiServerVerifier::builder(Arc::new(root_store)).build()?;
        Ok(Self {
            inner,
            expected_fingerprint: expected_fingerprint.map(|f| f.to_ascii_uppercase()),
        })
    }

    /// Access the pinned fingerprint filter (if any).
    pub fn expected_fingerprint(&self) -> Option<&str> {
        self.expected_fingerprint.as_deref()
    }
}

impl ServerCertVerifier for PinnedServerCertVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        // LocalSend peers are identified by IP without matching SANs; check crypto & validity
        verify_cert_from_der(end_entity.as_ref(), None)
            .map_err(|e| rejected(format!("Server certificate is not valid: {e:#}")))?;

        if let Some(expected) = &self.expected_fingerprint {
            let actual = fingerprint_from_cert_der(end_entity.as_ref());
            if actual != *expected {
                return Err(rejected(format!(
                    "Server certificate fingerprint mismatch: expected {expected}, got {actual}"
                )));
            }
        }

        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Verifies client certificates on incoming TLS connections.
///
/// If `mandatory` is true, connections without client certificates are rejected.
/// If `expected_fingerprint` is `Some`, the client certificate fingerprint must match it.
#[derive(Debug)]
pub struct PinnedClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    mandatory: bool,
    expected_fingerprint: Option<String>,
}

impl PinnedClientCertVerifier {
    /// Creates a new client certificate verifier.
    ///
    /// `own_cert_pem` is used to initialize the inner verifier's signature schemes.
    pub fn new(
        own_cert_pem: &str,
        mandatory: bool,
        expected_fingerprint: Option<String>,
    ) -> anyhow::Result<Self> {
        let mut root_store = RootCertStore::empty();
        root_store.add(CertificateDer::from_pem_slice(own_cert_pem.as_bytes())?)?;

        let inner = WebPkiClientVerifier::builder(Arc::new(root_store)).build()?;
        Ok(Self {
            inner,
            mandatory,
            expected_fingerprint: expected_fingerprint.map(|f| f.to_ascii_uppercase()),
        })
    }

    /// Access whether client auth is strictly required.
    pub fn is_mandatory(&self) -> bool {
        self.mandatory
    }

    /// Access the pinned client fingerprint filter (if any).
    pub fn expected_fingerprint(&self) -> Option<&str> {
        self.expected_fingerprint.as_deref()
    }
}

impl ClientCertVerifier for PinnedClientCertVerifier {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        self.mandatory
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        verify_cert_from_der(end_entity.as_ref(), None)
            .map_err(|e| rejected(format!("Client certificate is not valid: {e:#}")))?;

        if let Some(expected) = &self.expected_fingerprint {
            let actual = fingerprint_from_cert_der(end_entity.as_ref());
            if actual != *expected {
                return Err(rejected(format!(
                    "Client certificate fingerprint mismatch: expected {expected}, got {actual}"
                )));
            }
        }

        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::cert::generate_self_signed;

    static SPEC_CERT: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDGTCCAgGgAwIBAgIBATANBgkqhkiG9w0BAQsFADBQMRcwFQYDVQQDEw5Mb2Nh\n\
bFNlbmQgVXNlcjEJMAcGA1UEChMAMQkwBwYDVQQLEwAxCTAHBgNVBAcTADEJMAcG\n\
A1UECBMAMQkwBwYDVQQGEwAwHhcNMjUwMjA5MDAwMzE0WhcNMzUwMjA3MDAwMzE0\n\
WjBQMRcwFQYDVQQDEw5Mb2NhbFNlbmQgVXNlcjEJMAcGA1UEChMAMQkwBwYDVQQL\n\
EwAxCTAHBgNVBAcTADEJMAcGA1UECBMAMQkwBwYDVQQGEwAwggEiMA0GCSqGSIb3\n\
DQEBAQUAA4IBDwAwggEKAoIBAQCL24MxhGfrdJm0Q8ZGiBkZ27ldcEChB4w7rSbJ\n\
yiKeosoNbJl2kyj5dZjfBhWGgDLGDMM5w+Mh/5SrWgTL/QrhbB+lsrxILLznWqBi\n\
R8wJP0P2YW9fBahQskJQcUXt/3jsCsMTWea4rWc3HZGh03bAkJfLM+PDSOfTpvAZ\n\
6DQSp9QLzC9bgVNnq3W0SvOZGpF0xRa4InCyTUgxsNsV4+GIrmN5w4EbRFVVYu7D\n\
5OS5fxNSCukiS0fb6oQzUp0vIAycvvWHHbAy8T6UMoUor2nfvNcryiaOX5WBMLyh\n\
yMZ5gMOyXjdm1bT1XSlvtXPYUzxvsGAzTqS8mXjw8h7mm5htAgMBAAEwDQYJKoZI\n\
hvcNAQELBQADggEBABZ+I7D6wkeSrsi1NBLP2zoZ5oGh+INNcGTravfOQHs4Fbas\n\
/CysaUYjsD3fmaDh4MxgWEqAmWnnBiojfpGX2SGuFqRBKyT9DgitBt0L7Ezg1k3h\n\
bfSiFW4hXWp75grVO8xfML7ZcWMlhKrOsOMUGiy1qs3qsyJ3w7B2Tz78HhXGO5dd\n\
jyPmZarhixKO92UpEvKGxjO0E/3UUNUzxKTAAgFfhKpuwHUgIijM/EppZtA8OcSh\n\
fEztiV0xKfcPVx4d6dqRt/NMElK1Ivw2vUuxTymphZkkFOzht9m73/kyKaeFp8Ij\n\
VRus1zGVD8IVpIdPMyz01WJyS7M0fWaHXKWo+Bo=\n\
-----END CERTIFICATE-----";

    const SPEC_FINGERPRINT: &str =
        "4BADDE53A7F7CDEEED93189FD898E02BF6B4806CA4C05DE0ACE08319B86552FA";

    fn init_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn test_server_verifier_accepts_matching_fingerprint() {
        init_provider();
        let verifier =
            PinnedServerCertVerifier::new(SPEC_CERT, Some(SPEC_FINGERPRINT.to_string())).unwrap();

        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();
        let server_name = ServerName::try_from("192.168.1.50").unwrap();

        let res = verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            UnixTime::now(),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_server_verifier_accepts_case_insensitive_fingerprint() {
        init_provider();
        let lowercase_fp = SPEC_FINGERPRINT.to_ascii_lowercase();
        let verifier =
            PinnedServerCertVerifier::new(SPEC_CERT, Some(lowercase_fp)).unwrap();

        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();
        let server_name = ServerName::try_from("192.168.1.50").unwrap();

        let res = verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            UnixTime::now(),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_server_verifier_rejects_mismatching_fingerprint() {
        init_provider();
        let rogue_fp = "0".repeat(64);
        let verifier =
            PinnedServerCertVerifier::new(SPEC_CERT, Some(rogue_fp.clone())).unwrap();

        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();
        let server_name = ServerName::try_from("192.168.1.50").unwrap();

        let err = verifier
            .verify_server_cert(&cert_der, &[], &server_name, &[], UnixTime::now())
            .unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("fingerprint mismatch"), "{msg}");
        assert!(msg.contains(&rogue_fp), "{msg}");
        assert!(msg.contains(SPEC_FINGERPRINT), "{msg}");
    }

    #[test]
    fn test_server_verifier_accepts_any_valid_cert_when_unpinned() {
        init_provider();
        let verifier = PinnedServerCertVerifier::new(SPEC_CERT, None).unwrap();

        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();
        let server_name = ServerName::try_from("192.168.1.50").unwrap();

        let res = verifier.verify_server_cert(
            &cert_der,
            &[],
            &server_name,
            &[],
            UnixTime::now(),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_client_verifier_accepts_matching_fingerprint() {
        init_provider();
        let verifier =
            PinnedClientCertVerifier::new(SPEC_CERT, true, Some(SPEC_FINGERPRINT.to_string()))
                .unwrap();

        assert!(verifier.is_mandatory());
        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();

        let res = verifier.verify_client_cert(&cert_der, &[], UnixTime::now());
        assert!(res.is_ok());
    }

    #[test]
    fn test_client_verifier_rejects_mismatched_fingerprint() {
        init_provider();
        let verifier =
            PinnedClientCertVerifier::new(SPEC_CERT, true, Some("A".repeat(64))).unwrap();

        let cert_der = CertificateDer::from_pem_slice(SPEC_CERT.as_bytes()).unwrap();
        let err = verifier.verify_client_cert(&cert_der, &[], UnixTime::now()).unwrap_err();
        let msg = format!("{err:?}");
        assert!(msg.contains("fingerprint mismatch"), "{msg}");
    }

    #[test]
    fn test_client_verifier_with_newly_generated_identity() {
        init_provider();
        let identity = generate_self_signed().unwrap();
        let verifier = PinnedClientCertVerifier::new(
            &identity.cert_pem,
            false,
            Some(identity.fingerprint.clone()),
        )
        .unwrap();

        assert!(!verifier.is_mandatory());
        let cert_der = CertificateDer::from_pem_slice(identity.cert_pem.as_bytes()).unwrap();
        assert!(verifier.verify_client_cert(&cert_der, &[], UnixTime::now()).is_ok());
    }
}
