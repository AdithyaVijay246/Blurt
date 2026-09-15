//! Command-layer error type — the only shape of error that crosses the IPC
//! boundary. Never bubbles `blurt_schema::SchemaError`'s `Display` text
//! straight through without a deliberate decision to: raw SQLite error text
//! is an implementation detail the frontend shouldn't depend on.

use serde::Serialize;

#[derive(Debug, thiserror::Error, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "message")]
pub enum CommandError {
    #[error("database is locked")]
    Locked,

    #[error("invalid id: {0}")]
    InvalidId(String),

    /// No vault exists yet. Distinct from a wrong passphrase because the
    /// frontend shows a different screen: onboarding, not the unlock screen.
    #[error("vault has not been set up")]
    NotInitialized,

    /// A vault already exists. Re-initializing would generate a new master key
    /// and orphan everything encrypted under the old one.
    #[error("vault already exists")]
    AlreadyInitialized,

    /// The passphrase or recovery key did not unwrap a slot. Carries no detail
    /// on purpose — the same reasoning as `SchemaError::WrongSecret`, which is
    /// not an oracle.
    #[error("incorrect passphrase")]
    WrongSecret,

    /// A filesystem or platform-path failure.
    #[error("{0}")]
    Io(String),

    #[error("{0}")]
    Schema(String),
}

impl From<blurt_schema::SchemaError> for CommandError {
    fn from(err: blurt_schema::SchemaError) -> Self {
        match err {
            // Mapped to its own variant so the unlock screen can say "wrong
            // passphrase" without string-matching on an error message.
            blurt_schema::SchemaError::WrongSecret => CommandError::WrongSecret,
            other => CommandError::Schema(other.to_string()),
        }
    }
}

pub type CommandResult<T> = Result<T, CommandError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_error_converts_to_a_schema_variant_carrying_its_message() {
        let err: CommandError = blurt_schema::SchemaError::DatabaseLocked.into();
        assert_eq!(
            err,
            CommandError::Schema("database could not be decrypted with the supplied key".to_string())
        );
    }

    #[test]
    fn locked_serializes_with_no_content_field() {
        let json = serde_json::to_value(CommandError::Locked).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "Locked" }));
    }

    #[test]
    fn invalid_id_serializes_with_its_message() {
        let json = serde_json::to_value(CommandError::InvalidId("not-a-uuid".to_string())).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "InvalidId", "message": "not-a-uuid" }));
    }
}
