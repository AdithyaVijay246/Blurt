//! Error types for `blurt-schema`.

use thiserror::Error;

/// Everything that can go wrong in the storage/encryption layer.
///
/// `WrongSecret` is deliberately its own variant and carries no detail about
/// *why* the unwrap failed — callers (and the UI) must not be able to
/// distinguish "wrong passphrase" from "corrupt slot" in a way that could be
/// used as an oracle.
#[derive(Debug, Error)]
pub enum SchemaError {
    /// The supplied passphrase or recovery key did not unwrap the slot.
    #[error("incorrect passphrase or recovery key")]
    WrongSecret,

    /// The requested slot is not present in the keyring.
    #[error("no {0} keyslot present")]
    NoSuchSlot(&'static str),

    /// A slot with this kind already exists; replace it rather than adding.
    #[error("a {0} keyslot already exists")]
    SlotExists(&'static str),

    /// The recovery key string could not be parsed.
    #[error("invalid recovery key: {0}")]
    InvalidRecoveryKey(&'static str),

    /// Keyring file is a version this build does not understand.
    #[error("unsupported keyring format version {0}")]
    UnsupportedKeyringVersion(u32),

    /// The database opened but could not be read with the supplied key.
    #[error("database could not be decrypted with the supplied key")]
    DatabaseLocked,

    /// Key derivation failed (bad Argon2 parameters, bad output length).
    #[error("key derivation failed: {0}")]
    Kdf(String),

    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("keyring serialization error: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, SchemaError>;
