//! SQLCipher connection and unlock — `MODULE_02_SCHEMA.md` §3.
//!
//! Whole-database encryption with a single AES-256 master key. No per-field or
//! per-table encryption for v1.
//!
//! ## Raw-key form
//!
//! The connection is keyed with SQLCipher's raw-key syntax,
//! `PRAGMA key = "x'<64 hex>'"`, rather than the passphrase form. Our master
//! key is already 256 bits of CSPRNG output, so running SQLCipher's own KDF
//! over it would add startup cost without adding entropy. The Argon2id work
//! happens once, in [`crate::keyring`], where the input actually is a
//! low-entropy human passphrase.
//!
//! ## Why opening probes the database
//!
//! SQLCipher does **not** fail at `PRAGMA key` time when the key is wrong — it
//! fails at the first read, with a generic "file is not a database" error. An
//! open path that doesn't probe would hand back a `Database` that looks
//! healthy and then explode somewhere unrelated, so [`Database::open`] reads
//! `sqlite_master` immediately and converts failure into
//! [`SchemaError::DatabaseLocked`].

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Result, SchemaError};
use crate::keyring::MasterKey;

/// An open, decrypted handle to the Blurt database.
pub struct Database {
    conn: Connection,
}

impl std::fmt::Debug for Database {
    /// Hand-written rather than derived: the connection handle carries the
    /// database path, and a derived `Debug` would put it into any panic
    /// message or log line that formats a `Result<Database, _>`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Database(<open>)")
    }
}

impl Database {
    /// Opens the encrypted database at `path`, creating it if absent.
    pub fn open(path: &Path, master: &MasterKey) -> Result<Self> {
        let _ = (path, master);
        todo!("open encrypted database")
    }

    /// Opens an encrypted in-memory database. Used by tests.
    pub fn open_in_memory(master: &MasterKey) -> Result<Self> {
        let _ = master;
        todo!("open in-memory encrypted database")
    }

    /// Borrows the underlying connection.
    ///
    /// The repository layer (a later pass) is built on this; it is exposed so
    /// sibling modules can issue queries without this crate wrapping every
    /// statement.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Applies the key and verifies it actually decrypts.
    fn key_and_verify(conn: &Connection, master: &MasterKey) -> Result<()> {
        let _ = (conn, master);
        todo!("apply key and verify")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyring::MasterKey;

    #[test]
    fn opens_and_reopens_with_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blurt.db");
        let master = MasterKey::generate();

        {
            let db = Database::open(&path, &master).expect("first open");
            db.conn()
                .execute_batch("CREATE TABLE probe (v TEXT); INSERT INTO probe VALUES ('hello');")
                .unwrap();
        }

        let db = Database::open(&path, &master).expect("reopen");
        let value: String = db
            .conn()
            .query_row("SELECT v FROM probe", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "hello");
    }

    #[test]
    fn wrong_key_is_rejected_at_open_not_later() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blurt.db");

        {
            let db = Database::open(&path, &MasterKey::generate()).unwrap();
            db.conn().execute_batch("CREATE TABLE probe (v TEXT);").unwrap();
        }

        let err = Database::open(&path, &MasterKey::generate())
            .expect_err("a different key must not open the database");
        assert!(
            matches!(err, SchemaError::DatabaseLocked),
            "expected DatabaseLocked, got {err:?}"
        );
    }

    #[test]
    fn database_file_is_actually_encrypted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blurt.db");
        let master = MasterKey::generate();

        {
            let db = Database::open(&path, &master).unwrap();
            db.conn()
                .execute_batch(
                    "CREATE TABLE probe (v TEXT); INSERT INTO probe VALUES ('sentinel-plaintext');",
                )
                .unwrap();
        }

        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.starts_with(b"SQLite format 3"),
            "file has a plaintext SQLite header — it is not encrypted"
        );
        let needle = b"sentinel-plaintext";
        assert!(
            !bytes.windows(needle.len()).any(|w| w == needle),
            "row content is readable on disk"
        );
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        let enabled: i64 = db
            .conn()
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(enabled, 1, "foreign key enforcement is off");
    }

    #[test]
    fn in_memory_database_works() {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        db.conn().execute_batch("CREATE TABLE t (a INTEGER);").unwrap();
        let count: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
