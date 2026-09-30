use base64::Engine;
use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn random_url_token(byte_len: usize) -> String {
    let mut bytes = vec![0_u8; byte_len];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn sha256_fingerprint(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn verify_p256_sha256_signature(
    public_key_spki_der: &[u8],
    message: &[u8],
    signature_der: &[u8],
) -> Result<(), String> {
    let verifying_key = VerifyingKey::from_public_key_der(public_key_spki_der)
        .map_err(|error| error.to_string())?;
    let signature = Signature::from_der(signature_der).map_err(|error| error.to_string())?;
    verifying_key
        .verify(message, &signature)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_url_safe() {
        let token = random_url_token(32);
        assert!(token.len() >= 43);
        assert!(token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
    }

    #[test]
    fn sha256_fingerprint_is_colon_separated_hex() {
        let fp = sha256_fingerprint(b"wardian");
        assert_eq!(
            fp,
            "fe:bc:b9:48:9c:75:26:5f:ed:92:0d:98:eb:bb:38:2a:e5:b1:da:62:84:62:ab:ae:7e:38:ff:89:9f:a1:2b:ee"
        );
    }

    #[test]
    fn p256_signature_rejects_malformed_public_key() {
        assert!(verify_p256_sha256_signature(b"not-spki", b"message", b"signature").is_err());
    }

    #[test]
    fn p256_signature_verifies_spki_public_key_and_der_signature() {
        use p256::ecdsa::{signature::Signer, SigningKey};
        use p256::pkcs8::EncodePublicKey;

        let signing_key = SigningKey::from_bytes((&[7_u8; 32]).into()).expect("signing key");
        let public_key_der = signing_key
            .verifying_key()
            .to_public_key_der()
            .expect("public key der");
        let signature: Signature = signing_key.sign(b"message");

        assert!(verify_p256_sha256_signature(
            public_key_der.as_bytes(),
            b"message",
            signature.to_der().as_bytes()
        )
        .is_ok());
        assert!(verify_p256_sha256_signature(
            public_key_der.as_bytes(),
            b"tampered",
            signature.to_der().as_bytes()
        )
        .is_err());
    }
}
