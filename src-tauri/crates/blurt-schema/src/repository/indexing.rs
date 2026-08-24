//! Sensitive-safe accessors — `MODULE_02_SCHEMA.md` §4.
//!
//! `isSensitive` destinations must be excluded from embedding generation and
//! keyword extraction **structurally**, not by caller discipline — a caller
//! that forgets to filter must still get the safe result. [`items_for_indexing`]
//! is the query every indexing pipeline (Module 4) is meant to call instead of
//! reading `items` directly.

use rusqlite::Connection;

use crate::error::Result;
use crate::repository::items::{row_to_item, Item};

/// Every live item eligible for embedding/keyword indexing: excludes items in
/// `isSensitive` destinations, and any tombstoned item or destination.
pub fn items_for_indexing(conn: &Connection) -> Result<Vec<Item>> {
    let mut stmt = conn.prepare(
        "SELECT items.id, items.destinationId, items.originalText, items.currentText,
                items.checked, items.createdAt, items.deletedAt
         FROM items
         JOIN destinations ON destinations.id = items.destinationId
         WHERE items.deletedAt IS NULL
           AND destinations.deletedAt IS NULL
           AND destinations.isSensitive = 0",
    )?;
    let items = stmt.query_map([], row_to_item)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::keyring::MasterKey;
    use crate::migrations;
    use crate::repository::destinations::{self, DestinationKind};
    use crate::repository::items;

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        db
    }

    /// The invariant this crate cannot afford to get wrong: a sensitive
    /// destination's items must never come back from the indexing-facing
    /// query, structurally — not because every caller remembers to filter.
    #[test]
    fn sensitive_destinations_items_are_structurally_excluded() {
        let db = migrated();

        let ordinary = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let sensitive = destinations::create(db.conn(), "Passwords", "pw", DestinationKind::List, None, false, true, 0).unwrap();

        let safe_item = items::capture(db.conn(), ordinary.id, "buy milk").unwrap();
        let secret_item = items::capture(db.conn(), sensitive.id, "gmail: hunter2").unwrap();

        let indexable = items_for_indexing(db.conn()).unwrap();
        let ids: Vec<_> = indexable.iter().map(|i| i.id).collect();

        assert!(ids.contains(&safe_item.id), "non-sensitive item should be indexable");
        assert!(!ids.contains(&secret_item.id), "sensitive destination's item leaked into the indexing query");
    }

    #[test]
    fn tombstoned_items_are_excluded_even_when_not_sensitive() {
        let db = migrated();
        let ordinary = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), ordinary.id, "buy milk").unwrap();
        items::tombstone(db.conn(), item.id).unwrap();

        let indexable = items_for_indexing(db.conn()).unwrap();
        assert!(indexable.iter().all(|i| i.id != item.id));
    }

    #[test]
    fn items_in_a_tombstoned_destination_are_excluded() {
        let db = migrated();
        let destination = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();
        destinations::tombstone(db.conn(), destination.id).unwrap();

        let indexable = items_for_indexing(db.conn()).unwrap();
        assert!(indexable.iter().all(|i| i.id != item.id));
    }
}
