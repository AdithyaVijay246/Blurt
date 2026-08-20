//! Module 2 — SQLCipher schema and key-wrapping encryption.
//!
//! See `docs/MODULE_02_SCHEMA.md`.
//!
//! This crate owns storage primitives only. The *policy* around them — when a
//! fresh, uncached auth check is required for a sensitive destination, the
//! app-level idle timer, biometric integration — belongs to `blurt-app` and
//! Module 6. This crate can tell you whether a passphrase unwraps a slot; it
//! does not decide when to ask for one.

pub mod db;
pub mod error;
pub mod keyring;
pub mod migrations;
pub mod recovery;

pub use db::Database;
pub use error::{Result, SchemaError};
pub use keyring::{KeySlot, Keyring, MasterKey, SlotKind, MASTER_KEY_LEN};
pub use migrations::LATEST_VERSION;
pub use recovery::RecoveryKey;
