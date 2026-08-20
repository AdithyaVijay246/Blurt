//! Schema migrations, versioned via `PRAGMA user_version`.
//!
//! Deliberately hand-rolled rather than pulling a migration framework: there
//! is one linear sequence of forward-only migrations, and `user_version` is a
//! SQLite built-in that costs nothing.
//!
//! `blurt-sync` will add its own migration for `yrs` update logs and paired
//! devices when Module 5 is built. Those tables are Module 5's to design and
//! are not invented here.

use rusqlite::Connection;

use crate::error::Result;

/// Ordered, forward-only migrations. Index + 1 is the resulting `user_version`.
const MIGRATIONS: &[&str] = &[include_str!("../migrations/0001_initial.sql")];

/// The `user_version` a fully-migrated database reports.
pub const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

/// Reads the current schema version.
pub fn current_version(conn: &Connection) -> Result<i64> {
    let _ = conn;
    todo!("read user_version")
}

/// Applies every migration newer than the database's current version.
///
/// Idempotent: running it against an up-to-date database is a no-op.
pub fn run(conn: &Connection) -> Result<()> {
    let _ = conn;
    todo!("run migrations")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::keyring::MasterKey;

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        run(db.conn()).expect("migrations should apply");
        db
    }

    fn table_exists(db: &Database, name: &str) -> bool {
        db.conn()
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [name],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            > 0
    }

    #[test]
    fn fresh_database_starts_at_version_zero() {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        assert_eq!(current_version(db.conn()).unwrap(), 0);
    }

    #[test]
    fn migrating_reaches_the_latest_version() {
        let db = migrated();
        assert_eq!(current_version(db.conn()).unwrap(), LATEST_VERSION);
    }

    #[test]
    fn migrating_is_idempotent() {
        let db = migrated();
        run(db.conn()).expect("second run should be a no-op");
        run(db.conn()).expect("third run should be a no-op");
        assert_eq!(current_version(db.conn()).unwrap(), LATEST_VERSION);
    }

    #[test]
    fn creates_every_table_from_the_schema_doc() {
        let db = migrated();
        for table in ["destinations", "items", "edits", "embeddings", "keywords"] {
            assert!(table_exists(&db, table), "missing table {table}");
        }
    }

    /// §1: Unsorted is not a special structure — it is an ordinary row with
    /// `isSystem = true`.
    #[test]
    fn seeds_the_system_unsorted_destination() {
        let db = migrated();
        let (name, is_system, is_sensitive): (String, i64, i64) = db
            .conn()
            .query_row(
                "SELECT name, isSystem, isSensitive FROM destinations WHERE isSystem = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("Unsorted destination should exist");
        assert_eq!(name, "Unsorted");
        assert_eq!(is_system, 1);
        assert_eq!(is_sensitive, 0);
    }

    /// §6: only "Random Thoughts" ships premade, and it is ordinary — not
    /// `isSystem`, not `isSensitive`.
    #[test]
    fn seeds_random_thoughts_as_an_ordinary_destination() {
        let db = migrated();
        let (is_system, is_sensitive): (i64, i64) = db
            .conn()
            .query_row(
                "SELECT isSystem, isSensitive FROM destinations WHERE name = 'Random Thoughts'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("Random Thoughts should exist");
        assert_eq!(is_system, 0, "Random Thoughts must not be a system destination");
        assert_eq!(is_sensitive, 0, "nothing ships sensitive-by-default");
    }

    #[test]
    fn seeded_destinations_have_uuid_primary_keys() {
        let db = migrated();
        let mut stmt = db.conn().prepare("SELECT id FROM destinations").unwrap();
        let ids: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(ids.len(), 2);
        for id in ids {
            assert_eq!(id.len(), 36, "id {id} is not a hyphenated UUID");
            assert!(uuid::Uuid::parse_str(&id).is_ok(), "id {id} is not a UUID");
        }
    }

    #[test]
    fn live_destinations_cannot_share_a_trigger() {
        let db = migrated();
        db.conn()
            .execute(
                "INSERT INTO destinations (id, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
                 VALUES (?1, 'Shopping', 'shop', 'list', 0, 0, 0, 0)",
                [uuid::Uuid::new_v4().to_string()],
            )
            .unwrap();

        let clash = db.conn().execute(
            "INSERT INTO destinations (id, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
             VALUES (?1, 'Shopping 2', 'shop', 'list', 0, 0, 0, 0)",
            [uuid::Uuid::new_v4().to_string()],
        );
        assert!(clash.is_err(), "duplicate live trigger should be rejected");
    }

    /// Deletes are tombstones, so a plain UNIQUE constraint would burn a
    /// trigger forever once its destination was deleted.
    #[test]
    fn a_tombstoned_destinations_trigger_can_be_reused() {
        let db = migrated();
        let first = uuid::Uuid::new_v4().to_string();
        db.conn()
            .execute(
                "INSERT INTO destinations (id, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
                 VALUES (?1, 'Shopping', 'shop', 'list', 0, 0, 0, 0)",
                [&first],
            )
            .unwrap();

        db.conn()
            .execute("UPDATE destinations SET deletedAt = 1 WHERE id = ?1", [&first])
            .unwrap();

        db.conn()
            .execute(
                "INSERT INTO destinations (id, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
                 VALUES (?1, 'Shopping Again', 'shop', 'list', 0, 0, 0, 0)",
                [uuid::Uuid::new_v4().to_string()],
            )
            .expect("trigger should be reusable once the original is tombstoned");
    }

    #[test]
    fn items_reference_destinations_by_foreign_key() {
        let db = migrated();
        let orphan = db.conn().execute(
            "INSERT INTO items (id, destinationId, originalText, currentText, createdAt)
             VALUES (?1, ?2, 'x', 'x', 0)",
            [uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string()],
        );
        assert!(orphan.is_err(), "item pointing at a nonexistent destination was accepted");
    }

    #[test]
    fn creates_the_expected_indexes() {
        let db = migrated();
        let mut stmt = db
            .conn()
            .prepare("SELECT name FROM sqlite_master WHERE type='index'")
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        for expected in [
            "idx_items_destination",
            "idx_destinations_parent",
            "idx_edits_item",
            "idx_keywords_keyword",
            "idx_embeddings_item",
            "idx_destinations_trigger_live",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing index {expected}; found {names:?}"
            );
        }
    }
}
