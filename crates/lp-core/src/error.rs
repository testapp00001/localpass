#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("random number generator failure")]
    Rng,
    #[error("decryption failed (wrong key or corrupted data)")]
    Decrypt,
    #[error("encryption failed")]
    Encrypt,
    #[error("invalid key derivation parameters: {0}")]
    Kdf(String),
    #[error("invalid signature")]
    BadSignature,
    #[error("malformed data: {0}")]
    Malformed(String),
    #[error("encoding error: {0}")]
    Encode(String),
    #[error("invalid TOTP: {0}")]
    Totp(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
