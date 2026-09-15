//! `app_secrets` accessors — `MODULE_02_SCHEMA.md` §3.
//!
//! Secret material that has to live *inside* the encrypted database. Right now
//! that is the recovery key and nothing else.
//!
//! §3 is explicit about why it is stored at all, and it is worth restating
//! because it reads like a contradiction: the recovery key exists to unlock the
//! database, yet the stored copy is readable only once the database is already
//! open. That copy therefore serves `MODULE_06_UI_SHELL.md` §D2's
//! re-display — showing the user their key again from Settings, behind a fresh
//! auth check — and **not** recovery. Real recovery still depends entirely on
//! the user having saved the key externally at onboarding. An earlier revision
//! of §3 said the key was never stored; that was irreconcilable with §D2, and
//! this is the resolution.
//!
//! Nothing here is ever embedded, keyword-extracted, fed to a model, or synced.

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::recovery::RecoveryKey;

/// `app_secrets.key` for the recovery key row.
const RECOVERY_KEY: &str = "recovery_key";

/// Stores the recovery key, replacing any existing one.
///
/// An upsert rather than an insert because rotating the recovery key replaces
/// the row; the table's `key` is a primary key, so a plain insert would fail
/// the second time rather than doing the obvious thing.
///
/// The value is written as the **grouped display string**, not raw bytes. That
/// is the form §D2 re-displays and §B4 asks the user to verify, and
/// [`RecoveryKey::parse`] validates it on the way back in, so a corrupted row
/// surfaces as a parse error rather than as a silently wrong key.
pub fn put_recovery_key(conn: &Connection, recovery: &RecoveryKey) -> Result<()> {
    conn.execute(
        "INSERT INTO app_secrets (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![RECOVERY_KEY, recovery.to_grouped_string()],
    )?;
    Ok(())
}

/// Reads the stored recovery key, or `None` if onboarding never wrote one.
///
/// `None` is not an error: a vault created before this row existed, or one
/// mid-onboarding, simply has nothing to re-display.
pub fn get_recovery_key(conn: &Connection) -> Result<Option<RecoveryKey>> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT value FROM app_secrets WHERE key = ?1",
            [RECOVERY_KEY],
            |row| row.get(0),
        )
        .optional()?;

    stored.map(|value| RecoveryKey::parse(&value)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::keyring::MasterKey;
    use crate::migrations;

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        db
    }

    #[test]
    fn a_stored_recovery_key_round_trips() {
        let db = migrated();
        let recovery = RecoveryKey::generate();

        put_recovery_key(db.conn(), &recovery).unwrap();

        let read = get_recovery_key(db.conn()).unwrap().expect("a key was stored");
        assert_eq!(read.as_bytes(), recovery.as_bytes());
    }

    #[test]
    fn an_empty_table_yields_none_rather_than_an_error() {
        // A vault mid-onboarding has nothing to re-display, which is ordinary.
        let db = migrated();
        assert!(get_recovery_key(db.conn()).unwrap().is_none());
    }

    #[test]
    fn storing_twice_replaces_rather_than_failing_on_the_primary_key() {
        let db = migrated();
        let first = RecoveryKey::generate();
        let second = RecoveryKey::generate();

        put_recovery_key(db.conn(), &first).unwrap();
        put_recovery_key(db.conn(), &second).unwrap();

        let read = get_recovery_key(db.conn()).unwrap().unwrap();
        assert_eq!(read.as_bytes(), second.as_bytes(), "the newer key should win");
    }

    #[test]
    fn the_value_is_stored_in_the_form_settings_re_displays() {
        // §D2 shows the grouped string, and §B4 asks the user to re-enter two
        // of its groups. Storing raw bytes would mean formatting on every read
        // and would not round-trip through `RecoveryKey::parse`.
        let db = migrated();
        let recovery = RecoveryKey::generate();
        put_recovery_key(db.conn(), &recovery).unwrap();

        let raw: String = db
            .conn()
            .query_row(
                "SELECT value FROM app_secrets WHERE key = 'recovery_key'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(raw, recovery.to_grouped_string());
        assert!(raw.contains('-'), "expected the grouped form, got {raw}");
    }

    #[test]
    fn a_corrupted_row_surfaces_as_an_error_not_a_wrong_key() {
        let db = migrated();
        db.conn()
            .execute(
                "INSERT INTO app_secrets (key, value) VALUES ('recovery_key', 'not-a-key')",
                [],
            )
            .unwrap();

        assert!(get_recovery_key(db.conn()).is_err());
    }
}
