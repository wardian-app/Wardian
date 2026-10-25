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

    // RFC 6979 Appendix A.2.5, P-256/SHA-256, message "sample".
    const RFC6979_P256_SAMPLE_SPKI_DER_BASE64: &str =
        "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEYP7UuiVanTHJYet0xjVtaMBJuJI7Yfps5mliLmDyn7Z5A/4QCLi8maQa6elWKLxk8vGyDC1+n1F3o8KU1EYimQ==";
    const RFC6979_P256_SAMPLE_SIGNATURE_DER_BASE64: &str =
        "MEYCIQDv1IsqrLao/RFA3ZzUXoHWnSyHe1aq+ZHDTQ6oTq83FgIhAPfLHJQtZXxB1DbHobbin2Xz6QDbua/0Bk3Eqy+EOs2o";
    const RFC6979_P256_SAMPLE_SIGNATURE_RAW_BASE64: &str =
        "79SLKqy2qP0RQN2c1F6B1p0sh3tWqvmRw00OqE6vNxb3yxyULWV8QdQ2x6G24p9l8+kA27mv9AZNxKsvhDrNqA==";

    // NIST P-384 generator SPKI from FIPS 186-4 Appendix D.1.2.4.
    const P384_GENERATOR_SPKI_DER_BASE64: &str =
        "MHYwEAYHKoZIzj0CAQYFK4EEACIDYgAEqofKIr6LBTeOscce8yCtdG4dO2KLp5uYWfdB4IJUKjhVAvJdv1UpbDpUXjhydgq3NhfeSpYmLG9dnpi/kpLcKfj0Hb0omhR86doxE7XwuMAKYLHOHX6BnXpDHXyQ6g5f";

    fn decode_static_base64_fixture(value: &str) -> Vec<u8> {
        use base64::Engine as _;

        base64::engine::general_purpose::STANDARD
            .decode(value)
            .expect("valid static test fixture")
    }

    #[test]
    fn p256_signature_verifies_rfc6979_sha256_sample_vector() {
        let public_key_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SPKI_DER_BASE64);
        let signature_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SIGNATURE_DER_BASE64);

        assert!(verify_p256_sha256_signature(&public_key_der, b"sample", &signature_der).is_ok());
    }

    #[test]
    fn p256_signature_rejects_malformed_truncated_and_raw_signature_encodings() {
        let public_key_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SPKI_DER_BASE64);
        let signature_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SIGNATURE_DER_BASE64);
        let raw_signature = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SIGNATURE_RAW_BASE64);
        let truncated_signature = &signature_der[..signature_der.len() - 1];
        let malformed_der = [0x30, 0x00];

        for invalid_signature in [
            malformed_der.as_slice(),
            truncated_signature,
            raw_signature.as_slice(),
        ] {
            assert!(
                verify_p256_sha256_signature(&public_key_der, b"sample", invalid_signature)
                    .is_err()
            );
        }
    }

    #[test]
    fn p256_signature_rejects_signature_and_payload_tampering() {
        let public_key_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SPKI_DER_BASE64);
        let signature_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SIGNATURE_DER_BASE64);
        let mut tampered_signature_der = signature_der.clone();
        *tampered_signature_der
            .last_mut()
            .expect("nonempty RFC signature DER") ^= 0x01;

        assert!(
            verify_p256_sha256_signature(&public_key_der, b"sample", &tampered_signature_der)
                .is_err()
        );
        assert!(verify_p256_sha256_signature(&public_key_der, b"samplf", &signature_der).is_err());
    }

    #[test]
    fn p256_signature_rejects_invalid_points_and_other_curve_spki() {
        let public_key_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SPKI_DER_BASE64);
        let signature_der = decode_static_base64_fixture(RFC6979_P256_SAMPLE_SIGNATURE_DER_BASE64);
        let mut invalid_point_spki = public_key_der.clone();
        // In this fixed P-256 SPKI, byte 26 starts the uncompressed SEC1 point.
        invalid_point_spki[27..].fill(0);
        assert!(
            verify_p256_sha256_signature(&invalid_point_spki, b"sample", &signature_der).is_err()
        );

        let p384_spki = decode_static_base64_fixture(P384_GENERATOR_SPKI_DER_BASE64);
        assert!(verify_p256_sha256_signature(&p384_spki, b"sample", &signature_der).is_err());
    }
}
