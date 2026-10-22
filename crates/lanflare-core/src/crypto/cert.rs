//! Self-signed certificate and key generation.

use std::io::Cursor;
use sha2::{Digest, Sha256};
use x509_parser::asn1_rs::FromDer;
use x509_parser::certificate::X509Certificate;
use x509_parser::pem::Pem;
use x509_parser::x509::SubjectPublicKeyInfo;

/// Generated self-signed TLS identity.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TlsIdentity {
    /// PEM-encoded self-signed X.509 certificate.
    pub cert_pem: String,
    /// PEM-encoded private key (PKCS#8).
    pub key_pem: String,
    /// Uppercase hex SHA-256 fingerprint of the DER certificate bytes.
    pub fingerprint: String,
}

/// Loads a persisted TLS identity from a JSON file, or generates and saves a new one if not found or corrupted.
pub fn load_or_generate_identity(path: &std::path::Path) -> anyhow::Result<TlsIdentity> {
    if let Ok(content) = std::fs::read_to_string(path)
        && let Ok(ident) = serde_json::from_str::<TlsIdentity>(&content)
    {
        // Verify identity validity
        if !ident.cert_pem.trim().is_empty()
            && !ident.key_pem.trim().is_empty()
            && !ident.fingerprint.trim().is_empty()
        {
            return Ok(ident);
        }
    }

    let identity = generate_self_signed()?;
    let _ = save_identity(path, &identity);
    Ok(identity)
}

/// Saves a TLS identity to the specified JSON path.
pub fn save_identity(path: &std::path::Path, identity: &TlsIdentity) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let data = serde_json::to_string_pretty(identity)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, data)
}

/// Generates a new self-signed RSA-2048 device identity.
///
/// LocalSend v2.2 specification standard:
/// - RSA-2048 key pair
/// - `CN=LocalSend User`
/// - Valid from 1975 to 4096 (rcgen default, effectively non-expiring)
/// - Device identity is uniquely identified by the uppercase SHA-256 hex fingerprint
pub fn generate_self_signed() -> anyhow::Result<TlsIdentity> {
    generate_self_signed_rsa(2048)
}

/// Generates a self-signed RSA TLS identity with the specified bit size (e.g. 2048).
pub fn generate_self_signed_rsa(bits: usize) -> anyhow::Result<TlsIdentity> {
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};

    let mut rng = rsa::rand_core::OsRng;
    let private_key = rsa::RsaPrivateKey::new(&mut rng, bits)?;
    let private_key_pem = private_key.to_pkcs8_pem(LineEnding::LF)?;
    let key_pair = rcgen::KeyPair::try_from(private_key.to_pkcs8_der()?.as_bytes())?;

    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "LocalSend User");

    let cert = params.self_signed(&key_pair)?;
    let cert_pem = cert.pem();
    let fingerprint = fingerprint_from_cert_der(cert.der());

    Ok(TlsIdentity {
        cert_pem,
        key_pem: private_key_pem.to_string(),
        fingerprint,
    })
}

/// Generates a self-signed Ed25519 TLS identity.
///
/// Supported for high-performance LanFlare-native peer connections.
pub fn generate_self_signed_ed25519() -> anyhow::Result<TlsIdentity> {
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ED25519)?;
    let private_key_pem = key_pair.serialize_pem();

    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "LocalSend User");

    let cert = params.self_signed(&key_pair)?;
    let cert_pem = cert.pem();
    let fingerprint = fingerprint_from_cert_der(cert.der());

    Ok(TlsIdentity {
        cert_pem,
        key_pem: private_key_pem,
        fingerprint,
    })
}

/// Computes the SHA-256 fingerprint of a certificate in DER format.
///
/// Formatted as an uppercase hex string (64 chars), adhering strictly to
/// LocalSend v2.2 wire specification format.
pub fn fingerprint_from_cert_der(cert_der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    let hash = hasher.finalize();
    hash.iter().map(|b| format!("{b:02X}")).collect()
}

/// Computes the SHA-256 fingerprint of a PEM-encoded certificate.
pub fn fingerprint_from_cert_pem(cert_pem: &str) -> anyhow::Result<String> {
    let (pem, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))?;
    Ok(fingerprint_from_cert_der(&pem.contents))
}

/// Verifies whether an X.509 certificate parsed from PEM is valid:
/// - Cryptographic signature check (self-signed or against `expected_public_key`)
/// - Validity timeframe (not expired and already active)
/// - Optional public key matching (SPKI equality)
pub fn verify_cert_from_pem(
    cert_pem: &str,
    expected_public_key_pem: Option<&str>,
) -> anyhow::Result<()> {
    let (pem, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))?;
    let parsed_cert = pem.parse_x509()?;
    verify_cert_from_cert(&parsed_cert, expected_public_key_pem)
}

/// Verifies whether an X.509 certificate parsed from DER is valid:
/// - Cryptographic signature check (self-signed or against `expected_public_key`)
/// - Validity timeframe (not expired and already active)
/// - Optional public key matching (SPKI equality)
pub fn verify_cert_from_der(
    cert_der: &[u8],
    expected_public_key_pem: Option<&str>,
) -> anyhow::Result<()> {
    let (_, parsed_cert) = X509Certificate::from_der(cert_der)?;
    verify_cert_from_cert(&parsed_cert, expected_public_key_pem)
}

/// Internal helper to verify an `X509Certificate`.
fn verify_cert_from_cert(
    cert: &X509Certificate<'_>,
    expected_public_key_pem: Option<&str>,
) -> anyhow::Result<()> {
    if !cert.validity.is_valid() {
        anyhow::bail!("Certificate validity period check failed");
    }

    if let Some(pub_key_pem) = expected_public_key_pem {
        let cert_spki_raw = cert.tbs_certificate.subject_pki.raw;
        let (expected_pem, _) = Pem::read(Cursor::new(pub_key_pem.as_bytes()))?;
        if cert_spki_raw != expected_pem.contents.as_slice() {
            // Also attempt SubjectPublicKeyInfo parsed comparison for formatting differences
            let (_, expected_spki) = SubjectPublicKeyInfo::from_der(&expected_pem.contents)?;
            let cert_spki = cert.tbs_certificate.subject_pki.parsed()?;
            let expected_parsed = expected_spki.parsed()?;
            if cert_spki != expected_parsed {
                anyhow::bail!("Certificate public key mismatch");
            }
        }
    }

    cert.verify_signature(None)
        .map_err(|e| anyhow::anyhow!("Signature verification error: {e:?}"))?;

    Ok(())
}

/// Extracts the public key from DER certificate bytes and encodes it as PEM.
pub fn public_key_from_cert_der(cert_der: &[u8]) -> anyhow::Result<String> {
    let (_, cert) = X509Certificate::from_der(cert_der)?;
    public_key_from_cert(&cert)
}

/// Extracts the public key from an `X509Certificate` and encodes it as PEM (SPKI).
pub fn public_key_from_cert(cert: &X509Certificate<'_>) -> anyhow::Result<String> {
    let cert_public_key = cert.tbs_certificate.subject_pki.raw;
    let pem_block = pem::Pem::new("PUBLIC KEY", cert_public_key.to_vec());
    let config = pem::EncodeConfig::new().set_line_ending(pem::LineEnding::LF);
    Ok(pem::encode_config(&pem_block, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    static SPEC_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\n\
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAi9uDMYRn63SZtEPGRogZ\n\
Gdu5XXBAoQeMO60mycoinqLKDWyZdpMo+XWY3wYVhoAyxgzDOcPjIf+Uq1oEy/0K\n\
4WwfpbK8SCy851qgYkfMCT9D9mFvXwWoULJCUHFF7f947ArDE1nmuK1nNx2RodN2\n\
wJCXyzPjw0jn06bwGeg0EqfUC8wvW4FTZ6t1tErzmRqRdMUWuCJwsk1IMbDbFePh\n\
iK5jecOBG0RVVWLuw+TkuX8TUgrpIktH2+qEM1KdLyAMnL71hx2wMvE+lDKFKK9p\n\
37zXK8omjl+VgTC8ocjGeYDDsl43ZtW09V0pb7Vz2FM8b7BgM06kvJl48PIe5puY\n\
bQIDAQAB\n\
-----END PUBLIC KEY-----";

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

    #[test]
    fn test_spec_fingerprint_der_matches() {
        let (pem, _) = Pem::read(Cursor::new(SPEC_CERT.as_bytes())).unwrap();
        let computed = fingerprint_from_cert_der(&pem.contents);
        assert_eq!(computed, SPEC_FINGERPRINT);
    }

    #[test]
    fn test_spec_fingerprint_pem_matches() {
        let computed = fingerprint_from_cert_pem(SPEC_CERT).unwrap();
        assert_eq!(computed, SPEC_FINGERPRINT);
    }

    #[test]
    fn test_spec_cert_verification() {
        verify_cert_from_pem(SPEC_CERT, Some(SPEC_PUBLIC_KEY)).unwrap();
    }

    #[test]
    fn test_spec_cert_extract_public_key() {
        let (pem, _) = Pem::read(Cursor::new(SPEC_CERT.as_bytes())).unwrap();
        let extracted = public_key_from_cert_der(&pem.contents).unwrap();
        assert_eq!(
            extracted.trim(),
            SPEC_PUBLIC_KEY.trim()
        );
    }

    #[test]
    fn test_generate_self_signed_rsa_and_verify() {
        let identity = generate_self_signed().unwrap();
        assert!(identity.cert_pem.contains("-----BEGIN CERTIFICATE-----"));
        assert!(identity.key_pem.contains("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(identity.fingerprint.len(), 64);

        // Verify fingerprint matches re-extracted PEM
        let recomputed_fp = fingerprint_from_cert_pem(&identity.cert_pem).unwrap();
        assert_eq!(identity.fingerprint, recomputed_fp);

        // Verify self-signed certificate signature & validity
        verify_cert_from_pem(&identity.cert_pem, None).unwrap();

        // Parse with x509-parser to verify subject CN
        let (pem, _) = Pem::read(Cursor::new(identity.cert_pem.as_bytes())).unwrap();
        let (_, cert) = X509Certificate::from_der(&pem.contents).unwrap();
        let cn = cert.subject().iter_common_name().next().unwrap();
        assert_eq!(cn.as_str().unwrap(), "LocalSend User");
    }

    #[test]
    fn test_generate_self_signed_ed25519_and_verify() {
        let identity = generate_self_signed_ed25519().unwrap();
        assert!(identity.cert_pem.contains("-----BEGIN CERTIFICATE-----"));
        assert!(identity.key_pem.contains("-----BEGIN PRIVATE KEY-----"));
        assert_eq!(identity.fingerprint.len(), 64);

        // Verify self-signed certificate signature & validity
        verify_cert_from_pem(&identity.cert_pem, None).unwrap();

        let recomputed_fp = fingerprint_from_cert_pem(&identity.cert_pem).unwrap();
        assert_eq!(identity.fingerprint, recomputed_fp);
    }

    #[test]
    fn test_corrupted_cert_fails_verification() {
        // Corrupt signature bytes of the spec cert
        let mut corrupted = SPEC_CERT.to_string();
        corrupted = corrupted.replace("VRus1zGVD8IVpIdPMyz01WJyS7M0fWaHXKWo+Bo=", "VRus1zGVD8IVpIdPMyz01WJySwAAAAAAAAAAAAA=");
        let err = verify_cert_from_pem(&corrupted, Some(SPEC_PUBLIC_KEY));
        assert!(err.is_err(), "Corrupted signature must fail verification");
    }

    #[test]
    fn test_mismatched_public_key_fails_verification() {
        let identity1 = generate_self_signed().unwrap();
        let identity2 = generate_self_signed().unwrap();

        let (pem2, _) = Pem::read(Cursor::new(identity2.cert_pem.as_bytes())).unwrap();
        let pubkey2 = public_key_from_cert_der(&pem2.contents).unwrap();

        let err = verify_cert_from_pem(&identity1.cert_pem, Some(&pubkey2));
        assert!(err.is_err(), "Mismatched public key must fail verification");
    }

    #[test]
    fn test_identity_persistence_and_load() {
        let unique = format!("lanflare_test_ident_{}", rand::random::<u64>());
        let temp_dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&temp_dir).unwrap();
        let path = temp_dir.join("identity.json");

        // 1. Initial generation
        let id1 = load_or_generate_identity(&path).unwrap();
        assert_eq!(id1.fingerprint.len(), 64);
        assert!(path.exists());

        // 2. Load existing
        let id2 = load_or_generate_identity(&path).unwrap();
        assert_eq!(id1, id2, "Loaded identity must match previously saved identity");

        // 3. Corrupt file -> should regenerate cleanly
        std::fs::write(&path, "invalid json").unwrap();
        let id3 = load_or_generate_identity(&path).unwrap();
        assert_eq!(id3.fingerprint.len(), 64);
        assert_ne!(id1.fingerprint, id3.fingerprint);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
