//! End-to-End Encryption (E2EE) envelopes and signed node descriptors for multi-hop mesh relays.
//!
//! Intermediate relay nodes only inspect outer routing metadata (`RoutedEnvelope`). Inner payloads
//! are sealed end-to-end between origin sender and ultimate recipient using RSA-OAEP key encapsulation,
//! ChaCha20-Poly1305 authenticated symmetric encryption, and sender digital signatures.

use std::io::Cursor;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use lanflare_proto::mesh::{E2eeEnvelope, MeshNodeAnnouncement};
use ring::aead::{
    Aad, BoundKey, Nonce, NonceSequence, OpeningKey, SealingKey, UnboundKey, CHACHA20_POLY1305,
};
use ring::rand::SecureRandom;
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;
use x509_parser::pem::Pem;

use crate::crypto::cert::{fingerprint_from_cert_pem, public_key_from_cert_der, TlsIdentity};

/// Errors encountered during E2EE sealing, opening, or announcement verification.
#[derive(Debug, thiserror::Error)]
pub enum E2eeError {
    #[error("Certificate parsing or validation failed: {0}")]
    Certificate(String),

    #[error("Fingerprint mismatch: expected {expected}, computed {computed}")]
    FingerprintMismatch { expected: String, computed: String },

    #[error("Recipient fingerprint mismatch: envelope recipient {envelope_recipient} does not match own fingerprint {own_fingerprint}")]
    RecipientMismatch {
        envelope_recipient: String,
        own_fingerprint: String,
    },

    #[error("Key error: {0}")]
    Key(String),

    #[error("Digital signature verification failed: {0}")]
    InvalidSignature(String),

    #[error("Key decapsulation / RSA decryption failed: {0}")]
    KeyDecapsulationFailed(String),

    #[error("AEAD decryption or integrity authentication failed: {0}")]
    AeadAuthenticationFailed(String),

    #[error("Base64 decoding failed: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("Unsupported key algorithm: {0}")]
    UnsupportedAlgorithm(String),
}

/// Helper struct providing single-nonce consumption for `ring::aead`.
struct SingleNonce(Option<[u8; 12]>);

impl NonceSequence for SingleNonce {
    fn advance(&mut self) -> Result<Nonce, ring::error::Unspecified> {
        self.0
            .take()
            .map(Nonce::assume_unique_for_key)
            .ok_or(ring::error::Unspecified)
    }
}

/// Signs arbitrary canonical bytes using a private key (supporting RSA PKCS#8 or Ed25519 PKCS#8).
pub fn sign_with_identity(key_pem: &str, data: &[u8]) -> Result<Vec<u8>, E2eeError> {
    let (pem_block, _) = Pem::read(Cursor::new(key_pem.as_bytes()))
        .map_err(|e| E2eeError::Key(format!("Failed to parse private key PEM: {e}")))?;

    // 1. Try Ed25519 PKCS#8
    if let Ok(ed_keypair) = ring::signature::Ed25519KeyPair::from_pkcs8(&pem_block.contents) {
        let sig = ed_keypair.sign(data);
        return Ok(sig.as_ref().to_vec());
    }

    // 2. Try RSA PKCS#8
    if let Ok(rsa_keypair) = ring::signature::RsaKeyPair::from_pkcs8(&pem_block.contents) {
        let mut sig_buf = vec![0u8; rsa_keypair.public().modulus_len()];
        let rng = ring::rand::SystemRandom::new();
        rsa_keypair
            .sign(&ring::signature::RSA_PKCS1_SHA256, &rng, data, &mut sig_buf)
            .map_err(|e| E2eeError::Key(format!("RSA signing failed: {e:?}")))?;
        return Ok(sig_buf);
    }

    Err(E2eeError::UnsupportedAlgorithm(
        "Private key is neither a valid Ed25519 nor RSA PKCS#8 key".into(),
    ))
}

/// Verifies a digital signature against an X.509 certificate (supporting RSA or Ed25519).
pub fn verify_signature_with_cert(
    cert_pem: &str,
    data: &[u8],
    signature: &[u8],
) -> Result<(), E2eeError> {
    let (cert_pem_block, _) = Pem::read(Cursor::new(cert_pem.as_bytes()))
        .map_err(|e| E2eeError::Certificate(format!("Failed to read cert PEM: {e}")))?;
    let parsed_cert = cert_pem_block
        .parse_x509()
        .map_err(|e| E2eeError::Certificate(format!("Failed to parse X.509 cert: {e}")))?;

    if !parsed_cert.validity.is_valid() {
        return Err(E2eeError::Certificate(
            "Certificate validity period expired or not yet active".into(),
        ));
    }

    let pub_key_bytes = parsed_cert.tbs_certificate.subject_pki.subject_public_key.data.as_ref();
    let oid_str = parsed_cert
        .tbs_certificate
        .subject_pki
        .algorithm
        .algorithm
        .to_string();

    // Ed25519 OID: 1.3.101.112
    if oid_str == "1.3.101.112" {
        let verifier =
            ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pub_key_bytes);
        return verifier.verify(data, signature).map_err(|e| {
            E2eeError::InvalidSignature(format!("Ed25519 signature verification failed: {e:?}"))
        });
    }

    // RSA encryption OID: 1.2.840.113549.1.1.1
    if oid_str == "1.2.840.113549.1.1.1" {
        let verifier = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            pub_key_bytes,
        );
        return verifier.verify(data, signature).map_err(|e| {
            E2eeError::InvalidSignature(format!("RSA signature verification failed: {e:?}"))
        });
    }

    // Fallback detection
    let ed_verifier =
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pub_key_bytes);
    if ed_verifier.verify(data, signature).is_ok() {
        return Ok(());
    }

    let rsa_verifier = ring::signature::UnparsedPublicKey::new(
        &ring::signature::RSA_PKCS1_2048_8192_SHA256,
        pub_key_bytes,
    );
    if rsa_verifier.verify(data, signature).is_ok() {
        return Ok(());
    }

    Err(E2eeError::InvalidSignature(
        "Digital signature did not match certificate public key".into(),
    ))
}

/// Signs and creates a cryptographically authenticated `MeshNodeAnnouncement`.
pub fn sign_node_announcement(
    identity: &TlsIdentity,
    alias: &str,
    capabilities: Vec<String>,
    relay_enabled: bool,
    seq_no: u64,
    timestamp_ms: u64,
) -> Result<MeshNodeAnnouncement, E2eeError> {
    let mut announcement = MeshNodeAnnouncement {
        fingerprint: identity.fingerprint.to_ascii_uppercase(),
        alias: alias.to_string(),
        cert_pem: identity.cert_pem.clone(),
        capabilities,
        relay_enabled,
        seq_no,
        timestamp_ms,
        signature: String::new(),
    };

    let canonical_bytes = announcement.canonical_signing_bytes();
    let sig_bytes = sign_with_identity(&identity.key_pem, &canonical_bytes)?;
    announcement.signature = STANDARD.encode(&sig_bytes);

    Ok(announcement)
}

/// Verifies that a `MeshNodeAnnouncement`:
/// 1. Has a canonical fingerprint matching `SHA256(DER(cert_pem))`.
/// 2. Has a valid X.509 certificate within its validity period.
/// 3. Has a valid digital signature over its canonical bytes matching the certificate's public key.
pub fn verify_node_announcement(announcement: &MeshNodeAnnouncement) -> Result<(), E2eeError> {
    // 1. Verify fingerprint binding
    let computed_fp = fingerprint_from_cert_pem(&announcement.cert_pem)
        .map_err(|e| E2eeError::Certificate(format!("Failed to calculate fingerprint: {e:#}")))?;

    if !announcement.fingerprint.eq_ignore_ascii_case(&computed_fp) {
        return Err(E2eeError::FingerprintMismatch {
            expected: announcement.fingerprint.to_ascii_uppercase(),
            computed: computed_fp.to_ascii_uppercase(),
        });
    }

    // 2. Decode signature from base64
    let sig_bytes = STANDARD.decode(&announcement.signature)?;

    // 3. Verify digital signature over canonical announcement bytes
    let canonical_bytes = announcement.canonical_signing_bytes();
    verify_signature_with_cert(&announcement.cert_pem, &canonical_bytes, &sig_bytes)
}

/// Seals an arbitrary payload into an `E2eeEnvelope` for an intended recipient.
///
/// Encryption protocol:
/// - Validates `recipient_cert_pem` matches `recipient_fp`.
/// - Encapsulates a fresh 32-byte symmetric key with recipient's RSA public key (RSA-OAEP-SHA256).
/// - Encrypts `plaintext` using `ChaCha20-Poly1305` with a random 12-byte nonce and AAD binding sender and recipient fingerprints.
/// - Signs envelope metadata and ciphertext using sender's private key (RSA or Ed25519).
pub fn seal_envelope(
    sender: &TlsIdentity,
    recipient_fp: &str,
    recipient_cert_pem: &str,
    plaintext: &[u8],
) -> Result<E2eeEnvelope, E2eeError> {
    // 1. Validate recipient certificate fingerprint
    let computed_recipient_fp = fingerprint_from_cert_pem(recipient_cert_pem)
        .map_err(|e| E2eeError::Certificate(format!("Failed to read recipient cert: {e:#}")))?;

    if !recipient_fp.eq_ignore_ascii_case(&computed_recipient_fp) {
        return Err(E2eeError::FingerprintMismatch {
            expected: recipient_fp.to_ascii_uppercase(),
            computed: computed_recipient_fp.to_ascii_uppercase(),
        });
    }

    // 2. Extract recipient RSA public key from certificate
    let (cert_pem_block, _) = Pem::read(Cursor::new(recipient_cert_pem.as_bytes()))
        .map_err(|e| E2eeError::Certificate(format!("Failed to read recipient cert PEM: {e}")))?;
    let pub_key_pem = public_key_from_cert_der(&cert_pem_block.contents)
        .map_err(|e| E2eeError::Certificate(format!("Failed to extract public key: {e}")))?;
    let rsa_pub_key = RsaPublicKey::from_public_key_pem(&pub_key_pem).map_err(|e| {
        E2eeError::UnsupportedAlgorithm(format!("Recipient certificate is not RSA: {e}"))
    })?;

    // 3. Generate random 32-byte symmetric key
    let mut symmetric_key = [0u8; 32];
    let sys_rng = ring::rand::SystemRandom::new();
    sys_rng
        .fill(&mut symmetric_key)
        .map_err(|_| E2eeError::Key("System RNG error generating symmetric key".into()))?;

    // 4. Encapsulate symmetric key using RSA-OAEP-SHA256
    let mut rsa_rng = rsa::rand_core::OsRng;
    let padding = Oaep::new::<Sha256>();
    let encrypted_key_bytes = rsa_pub_key
        .encrypt(&mut rsa_rng, padding, &symmetric_key)
        .map_err(|e| {
            E2eeError::KeyDecapsulationFailed(format!("RSA-OAEP encryption failed: {e}"))
        })?;

    // 5. Generate random 12-byte nonce
    let mut nonce_bytes = [0u8; 12];
    sys_rng
        .fill(&mut nonce_bytes)
        .map_err(|_| E2eeError::Key("System RNG error generating nonce".into()))?;

    // 6. Encrypt plaintext via ChaCha20-Poly1305 with AAD binding sender and recipient
    let sender_fp_canonical = sender.fingerprint.to_ascii_uppercase();
    let recipient_fp_canonical = recipient_fp.to_ascii_uppercase();
    let aad_string = format!("{}:{}", sender_fp_canonical, recipient_fp_canonical);

    let unbound_key = UnboundKey::new(&CHACHA20_POLY1305, &symmetric_key)
        .map_err(|_| E2eeError::Key("Invalid symmetric key for ChaCha20".into()))?;
    let mut sealing_key = SealingKey::new(unbound_key, SingleNonce(Some(nonce_bytes)));

    let mut ciphertext_buf = plaintext.to_vec();
    sealing_key
        .seal_in_place_append_tag(Aad::from(aad_string.as_bytes()), &mut ciphertext_buf)
        .map_err(|_| E2eeError::AeadAuthenticationFailed("ChaCha20-Poly1305 seal failed".into()))?;

    // 7. Assemble envelope and compute sender digital signature
    let timestamp_ms = chrono::Utc::now().timestamp_millis() as u64;
    let mut envelope = E2eeEnvelope {
        sender_fp: sender_fp_canonical,
        recipient_fp: recipient_fp_canonical,
        sender_cert_pem: Some(sender.cert_pem.clone()),
        encrypted_key: STANDARD.encode(&encrypted_key_bytes),
        nonce: STANDARD.encode(nonce_bytes),
        ciphertext: STANDARD.encode(&ciphertext_buf),
        timestamp_ms,
        signature: String::new(),
    };

    let canonical_bytes = envelope.canonical_signing_bytes();
    let sig_bytes = sign_with_identity(&sender.key_pem, &canonical_bytes)?;
    envelope.signature = STANDARD.encode(&sig_bytes);

    Ok(envelope)
}

/// Opens an `E2eeEnvelope` addressed to the recipient.
///
/// Decryption protocol:
/// - Verifies `envelope.recipient_fp` matches recipient's fingerprint.
/// - Decapsulates the 32-byte symmetric key with recipient's RSA private key.
/// - Authenticates and decrypts `ciphertext` using ChaCha20-Poly1305 and AAD.
/// - If `sender_cert_pem` (or `envelope.sender_cert_pem`) is available, verifies sender's signature and fingerprint.
pub fn open_envelope(
    recipient: &TlsIdentity,
    sender_cert_pem: Option<&str>,
    envelope: &E2eeEnvelope,
) -> Result<Vec<u8>, E2eeError> {
    // 1. Verify recipient fingerprint
    if !envelope
        .recipient_fp
        .eq_ignore_ascii_case(&recipient.fingerprint)
    {
        return Err(E2eeError::RecipientMismatch {
            envelope_recipient: envelope.recipient_fp.to_ascii_uppercase(),
            own_fingerprint: recipient.fingerprint.to_ascii_uppercase(),
        });
    }

    // 2. Decapsulate symmetric key using RSA private key
    let encrypted_key_bytes = STANDARD.decode(&envelope.encrypted_key)?;
    let rsa_priv_key = RsaPrivateKey::from_pkcs8_pem(&recipient.key_pem)
        .map_err(|e| E2eeError::Key(format!("Failed to parse recipient private key: {e}")))?;

    let symmetric_key = rsa_priv_key
        .decrypt(Oaep::new::<Sha256>(), &encrypted_key_bytes)
        .map_err(|e| {
            E2eeError::KeyDecapsulationFailed(format!("RSA-OAEP decryption failed: {e}"))
        })?;

    if symmetric_key.len() != 32 {
        return Err(E2eeError::KeyDecapsulationFailed(
            "Decrypted symmetric key length is not 32 bytes".into(),
        ));
    }

    // 3. Decode nonce and ciphertext
    let nonce_bytes = STANDARD.decode(&envelope.nonce)?;
    if nonce_bytes.len() != 12 {
        return Err(E2eeError::AeadAuthenticationFailed(
            "Nonce length is not 12 bytes".into(),
        ));
    }
    let mut nonce_arr = [0u8; 12];
    nonce_arr.copy_from_slice(&nonce_bytes);

    let mut ciphertext_buf = STANDARD.decode(&envelope.ciphertext)?;
    if ciphertext_buf.len() < 16 {
        return Err(E2eeError::AeadAuthenticationFailed(
            "Ciphertext is shorter than 16-byte Poly1305 tag".into(),
        ));
    }

    // 4. Decrypt via ChaCha20-Poly1305 with AAD verification
    let aad_string = format!(
        "{}:{}",
        envelope.sender_fp.to_ascii_uppercase(),
        envelope.recipient_fp.to_ascii_uppercase()
    );

    let unbound_key = UnboundKey::new(&CHACHA20_POLY1305, &symmetric_key)
        .map_err(|_| E2eeError::Key("Invalid symmetric key for opening".into()))?;
    let mut opening_key = OpeningKey::new(unbound_key, SingleNonce(Some(nonce_arr)));

    let decrypted = opening_key
        .open_in_place(Aad::from(aad_string.as_bytes()), &mut ciphertext_buf)
        .map_err(|_| {
            E2eeError::AeadAuthenticationFailed(
                "ChaCha20-Poly1305 authentication failed (modified ciphertext, nonce, or recipient/sender AAD)".into(),
            )
        })?;
    let plaintext = decrypted.to_vec();

    // 5. Verify sender digital signature if sender certificate is present
    let cert_to_verify = sender_cert_pem.or(envelope.sender_cert_pem.as_deref());
    if let Some(cert_pem) = cert_to_verify {
        let computed_sender_fp = fingerprint_from_cert_pem(cert_pem)
            .map_err(|e| E2eeError::Certificate(format!("Invalid sender certificate: {e:#}")))?;

        if !envelope.sender_fp.eq_ignore_ascii_case(&computed_sender_fp) {
            return Err(E2eeError::FingerprintMismatch {
                expected: envelope.sender_fp.to_ascii_uppercase(),
                computed: computed_sender_fp.to_ascii_uppercase(),
            });
        }

        let sig_bytes = STANDARD.decode(&envelope.signature)?;
        let canonical_bytes = envelope.canonical_signing_bytes();
        verify_signature_with_cert(cert_pem, &canonical_bytes, &sig_bytes)?;
    }

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::cert::{generate_self_signed_ed25519, generate_self_signed_rsa};

    #[test]
    fn test_signed_node_announcement_verification_and_tamper_rejection() {
        // Test with RSA-2048 identity
        let rsa_ident = generate_self_signed_rsa(2048).unwrap();
        let rsa_announcement = sign_node_announcement(
            &rsa_ident,
            "Node-RSA",
            vec!["relay".into(), "chat".into()],
            true,
            1,
            1770000000000,
        )
        .unwrap();

        // Valid signature check must succeed
        assert!(verify_node_announcement(&rsa_announcement).is_ok());

        // Test with Ed25519 identity
        let ed_ident = generate_self_signed_ed25519().unwrap();
        let ed_announcement = sign_node_announcement(
            &ed_ident,
            "Node-Ed25519",
            vec!["stream".into()],
            false,
            2,
            1770000005000,
        )
        .unwrap();

        // Valid signature check must succeed
        assert!(verify_node_announcement(&ed_announcement).is_ok());

        // Tamper test 1: Modified alias must fail verification immediately
        let mut tampered_alias = rsa_announcement.clone();
        tampered_alias.alias = "Rogue-Alias".into();
        assert!(verify_node_announcement(&tampered_alias).is_err());

        // Tamper test 2: Modified seq_no must fail verification immediately
        let mut tampered_seq = rsa_announcement.clone();
        tampered_seq.seq_no += 1;
        assert!(verify_node_announcement(&tampered_seq).is_err());

        // Tamper test 3: Modified cert_pem must fail verification immediately
        let mut tampered_cert = rsa_announcement.clone();
        tampered_cert.cert_pem = ed_ident.cert_pem.clone();
        assert!(verify_node_announcement(&tampered_cert).is_err());

        // Tamper test 4: Modified relay_enabled must fail verification
        let mut tampered_relay = ed_announcement.clone();
        tampered_relay.relay_enabled = !tampered_relay.relay_enabled;
        assert!(verify_node_announcement(&tampered_relay).is_err());

        // Tamper test 5: Corrupted signature string must fail verification
        let mut tampered_sig = ed_announcement.clone();
        tampered_sig.signature = "AQIDBAU=".into();
        assert!(verify_node_announcement(&tampered_sig).is_err());
    }

    #[test]
    fn test_e2ee_envelope_seal_open_and_wrong_recipient_rejection() {
        let sender_ident = generate_self_signed_rsa(2048).unwrap();
        let recipient_ident = generate_self_signed_rsa(2048).unwrap();
        let wrong_recipient_ident = generate_self_signed_rsa(2048).unwrap();

        let secret_payload = b"Top secret multi-hop mesh message: hello LanFlare E2EE!";

        // 1. Seal envelope from sender to recipient
        let envelope = seal_envelope(
            &sender_ident,
            &recipient_ident.fingerprint,
            &recipient_ident.cert_pem,
            secret_payload,
        )
        .unwrap();

        // 2. Open envelope with intended recipient: must succeed bit-for-bit
        let opened_payload = open_envelope(&recipient_ident, None, &envelope).unwrap();
        assert_eq!(opened_payload, secret_payload);

        // 3. Opening with wrong recipient identity must fail immediately
        let wrong_open_res = open_envelope(&wrong_recipient_ident, None, &envelope);
        assert!(wrong_open_res.is_err());
        match wrong_open_res.unwrap_err() {
            E2eeError::RecipientMismatch { .. } => {}
            other => panic!("Expected RecipientMismatch error, got {other:?}"),
        }

        // 4. Even if recipient_fp is spoofed to match wrong recipient, RSA decapsulation must fail
        let mut spoofed_recipient_env = envelope.clone();
        spoofed_recipient_env.recipient_fp = wrong_recipient_ident.fingerprint.clone();
        let spoofed_open_res = open_envelope(&wrong_recipient_ident, None, &spoofed_recipient_env);
        assert!(spoofed_open_res.is_err());

        // 5. Tampered ciphertext must fail AEAD verification
        let mut tampered_ciphertext_env = envelope.clone();
        let mut raw_cipher = STANDARD.decode(&tampered_ciphertext_env.ciphertext).unwrap();
        raw_cipher[0] ^= 0xFF; // flip bits
        tampered_ciphertext_env.ciphertext = STANDARD.encode(&raw_cipher);
        let tampered_res = open_envelope(&recipient_ident, None, &tampered_ciphertext_env);
        assert!(tampered_res.is_err());

        // 6. Test with Ed25519 sender identity sealing for RSA recipient
        let ed_sender_ident = generate_self_signed_ed25519().unwrap();
        let ed_envelope = seal_envelope(
            &ed_sender_ident,
            &recipient_ident.fingerprint,
            &recipient_ident.cert_pem,
            b"Message signed by Ed25519 sender",
        )
        .unwrap();
        let ed_opened = open_envelope(&recipient_ident, None, &ed_envelope).unwrap();
        assert_eq!(ed_opened, b"Message signed by Ed25519 sender");
    }
}
