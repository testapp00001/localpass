#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),
    #[error("secure handshake failed: {0}")]
    Handshake(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("peer is not a member of this vault")]
    NotMember,
    #[error("peer device has been revoked")]
    Revoked,
    #[error("timed out")]
    Timeout,
    #[error("peer reported: {0}")]
    Remote(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
    #[error("wrong pairing code")]
    WrongCode,
    #[error("discovery error: {0}")]
    Discovery(String),
    #[error(transparent)]
    Store(#[from] lp_store::StoreError),
    #[error(transparent)]
    Core(#[from] lp_core::CoreError),
}

impl From<snow::Error> for SyncError {
    fn from(e: snow::Error) -> Self {
        SyncError::Handshake(e.to_string())
    }
}

impl From<tokio::time::error::Elapsed> for SyncError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        SyncError::Timeout
    }
}

pub type Result<T, E = SyncError> = std::result::Result<T, E>;
