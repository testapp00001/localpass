#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("no vault on this device yet")]
    NoVault,
    #[error("this device already has a vault")]
    AlreadyHasVault,
    #[error("the vault is locked")]
    Locked,
    #[error("wrong master password")]
    WrongPassword,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// A request the vault can't serve (for example a passkey algorithm we don't implement).
    #[error("{0}")]
    Unsupported(String),
    /// The site already has a passkey from this vault that it asked us not to duplicate.
    #[error("a passkey for this account already exists")]
    PasskeyExists,
    #[error("sync is not running")]
    SyncNotRunning,
    #[error("no device inviting on this network; enter its address or scan its QR code")]
    NoInviterFound,
    #[error("device secret storage failed: {0}")]
    Secret(String),
    #[error(transparent)]
    Core(#[from] lp_core::CoreError),
    #[error(transparent)]
    Store(#[from] lp_store::StoreError),
    #[error(transparent)]
    Sync(#[from] lp_sync::SyncError),
    #[error("file error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = EngineError> = std::result::Result<T, E>;
