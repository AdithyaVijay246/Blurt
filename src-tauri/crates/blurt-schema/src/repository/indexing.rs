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

/// Whether one specific item may be embedded and keyword-extracted.
///
/// The single-item counterpart to [`items_for_indexing`], and the same
/// structural guarantee: the eligibility rule lives in the query, so a caller
/// that forgets to check still cannot index a sensitive item. Module 4's
/// per-item pipeline asks this rather than re-deriving eligibility from an
/// [`Item`] it happens to be holding — an item struct carries no
/// `isSensitive`, and by the time a debounced re-index fires, the destination
/// may have been moved, tombstoned, or marked sensitive.
///
/// `false` for a missing item, a tombstoned item, an item whose destination is
/// tombstoned, and any item in an `isSensitive` destination.
pub fn is_item_indexable(conn: &Connection, item_id: uuid::Uuid) -> Result<bool> {
    let eligible: i64 = conn.query_row(
        "SELECT count(*)
         FROM items
         JOIN destinations ON destinations.id = items.destinationId
         WHERE items.id = ?1
           AND items.deletedAt IS NULL
           AND destinations.deletedAt IS NULL
           AND destinations.isSensitive = 0",
        [item_id.to_string()],
        |row| row.get(0),
    )?;
    Ok(eligible > 0)
}

/// One text version still waiting to be indexed: the item, and which version
/// of it — `None` for the original capture, `Some(edit_id)` for that edit.
pub type PendingVersion = (uuid::Uuid, Option<uuid::Uuid>);

/// Every indexable item whose **latest** version has no embeddings yet.
///
/// Drives `blurt-app`'s catch-up pass on unlock. Module 4 §3 queues indexing
/// as a background job, and an in-memory queue loses jobs to a quit, a crash,
/// or an embedding model that failed to load; without this, any such item would
/// stay unsearchable until it happened to be edited again.
///
/// Only the latest version is reported. §3's debounce deliberately leaves the
/// intermediate versions of a burst of edits unembedded, and from here those
/// are indistinguishable from versions that were lost — so backfilling them
/// would undo the debounce.
///
/// "Latest" breaks `editedAt` ties by `rowid`, the same rule
/// [`super::edits::history_for_item`] uses. Same sensitive-safe filter as
/// [`items_for_indexing`]: sensitive and tombstoned items are never reported.
pub fn pending_index_versions(conn: &Connection) -> Result<Vec<PendingVersion>> {
    // The latest-edit subquery appears twice rather than via a column alias,
    // because referencing a SELECT alias inside WHERE is a SQLite extension.
    //
    // `em.editId IS (...)` is deliberate: `IS` treats NULL as equal to NULL,
    // which is what matches an original-capture embedding (editId NULL) against
    // an item that has no edits at all.
    let mut stmt = conn.prepare(
        "SELECT items.id,
                (SELECT e.id FROM edits e WHERE e.itemId = items.id
                 ORDER BY e.editedAt DESC, e.rowid DESC LIMIT 1) AS latestEdit
         FROM items
         JOIN destinations ON destinations.id = items.destinationId
         WHERE items.deletedAt IS NULL
           AND destinations.deletedAt IS NULL
           AND destinations.isSensitive = 0
           AND NOT EXISTS (
               SELECT 1 FROM embeddings em
               WHERE em.itemId = items.id
                 AND em.editId IS (SELECT e.id FROM edits e WHERE e.itemId = items.id
                                   ORDER BY e.editedAt DESC, e.rowid DESC LIMIT 1))
         ORDER BY items.createdAt ASC, items.rowid ASC",
    )?;
    let pending = stmt
        .query_map([], |row| {
            let item: String = row.get(0)?;
            let edit: Option<String> = row.get(1)?;
            Ok((
                uuid::Uuid::parse_str(&item).expect("items.id is always a UUID"),
                edit.map(|e| uuid::Uuid::parse_str(&e).expect("edits.id is always a UUID")),
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(pending)
}

/// Stores one indexing pass's output — chunk rows and tags — atomically.
///
/// Module 4 writes its LanceDB vectors first and calls this second. The two
/// halves land together or not at all, so an item is never left tagged but
/// unsearchable, or searchable with tags describing an older wording.
///
/// It exists as one function rather than a call to
/// [`super::embeddings::insert_many`] followed by
/// [`super::keywords::replace_for_item`] because each of those opens its own
/// transaction, and SQLite will not nest them.
///
/// Tags replace; chunk rows accumulate. That asymmetry is deliberate and is
/// explained on each of those modules.
pub fn store_index_results(
    conn: &Connection,
    item_id: uuid::Uuid,
    embeddings: &[crate::repository::embeddings::Embedding],
    keywords: &[String],
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;

    tx.execute("DELETE FROM keywords WHERE itemId = ?1", [item_id.to_string()])?;
    {
        let mut stmt = tx.prepare("INSERT INTO keywords (itemId, keyword) VALUES (?1, ?2)")?;
        for keyword in keywords {
            stmt.execute(rusqlite::params![item_id.to_string(), keyword])?;
        }
    }
    {
        let mut stmt = tx.prepare(
            "INSERT INTO embeddings
                (id, itemId, editId, vectorRef, chunkIndex, chunkStartOffset, chunkEndOffset)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        for embedding in embeddings {
            stmt.execute(rusqlite::params![
                embedding.id.to_string(),
                embedding.item_id.to_string(),
                embedding.edit_id.map(|e| e.to_string()),
                embedding.vector_ref,
                embedding.chunk_index,
                embedding.chunk_start_offset,
                embedding.chunk_end_offset,
            ])?;
        }
    }

    tx.commit()?;
    Ok(())
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

    #[test]
    fn an_ordinary_live_item_is_indexable() {
        let db = migrated();
        let destination = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();

        assert!(is_item_indexable(db.conn(), item.id).unwrap());
    }

    #[test]
    fn an_item_in_a_sensitive_destination_is_not_indexable() {
        let db = migrated();
        let sensitive = destinations::create(db.conn(), "Passwords", "pw", DestinationKind::List, None, false, true, 0).unwrap();
        let item = items::capture(db.conn(), sensitive.id, "gmail: hunter2").unwrap();

        assert!(!is_item_indexable(db.conn(), item.id).unwrap());
    }

    #[test]
    fn an_item_becomes_unindexable_when_its_destination_turns_sensitive() {
        let db = migrated();
        let destination = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();
        assert!(is_item_indexable(db.conn(), item.id).unwrap());

        db.conn()
            .execute("UPDATE destinations SET isSensitive = 1 WHERE id = ?1", [destination.id.to_string()])
            .unwrap();

        assert!(
            !is_item_indexable(db.conn(), item.id).unwrap(),
            "eligibility is read at index time, not captured once"
        );
    }

    #[test]
    fn a_tombstoned_item_is_not_indexable() {
        let db = migrated();
        let destination = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();
        items::tombstone(db.conn(), item.id).unwrap();

        assert!(!is_item_indexable(db.conn(), item.id).unwrap());
    }

    #[test]
    fn an_item_in_a_tombstoned_destination_is_not_indexable() {
        let db = migrated();
        let destination = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();
        destinations::tombstone(db.conn(), destination.id).unwrap();

        assert!(!is_item_indexable(db.conn(), item.id).unwrap());
    }

    #[test]
    fn an_unknown_item_is_not_indexable() {
        let db = migrated();
        assert!(!is_item_indexable(db.conn(), uuid::Uuid::new_v4()).unwrap());
    }

    #[test]
    fn stores_chunk_rows_and_tags_together() {
        use crate::repository::embeddings::{self, Embedding};
        use crate::repository::keywords;

        let db = migrated();
        let destination = destinations::create(db.conn(), "Notes", "notes", DestinationKind::Note, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();

        let chunk = Embedding {
            id: uuid::Uuid::new_v4(),
            item_id: item.id,
            edit_id: None,
            vector_ref: "v0".to_string(),
            chunk_index: 0,
            chunk_start_offset: 0,
            chunk_end_offset: 8,
        };

        store_index_results(db.conn(), item.id, std::slice::from_ref(&chunk), &["milk".to_string()]).unwrap();

        assert_eq!(embeddings::list_for_item(db.conn(), item.id).unwrap(), vec![chunk]);
        assert_eq!(keywords::list_for_item(db.conn(), item.id).unwrap(), vec!["milk".to_string()]);
    }

    /// The reason this is one transaction: a failure part-way through must not
    /// leave an item tagged but unsearchable.
    #[test]
    fn a_failure_writing_chunks_rolls_the_tags_back_too() {
        use crate::repository::embeddings::Embedding;
        use crate::repository::keywords;

        let db = migrated();
        let destination = destinations::create(db.conn(), "Notes", "notes", DestinationKind::Note, None, false, false, 0).unwrap();
        let item = items::capture(db.conn(), destination.id, "buy milk").unwrap();
        keywords::replace_for_item(db.conn(), item.id, &["original".to_string()]).unwrap();

        let shared_id = uuid::Uuid::new_v4();
        let good = Embedding {
            id: shared_id,
            item_id: item.id,
            edit_id: None,
            vector_ref: "v0".to_string(),
            chunk_index: 0,
            chunk_start_offset: 0,
            chunk_end_offset: 8,
        };
        // Same primary key, so the second insert violates the constraint.
        let clashing = Embedding { vector_ref: "v1".to_string(), chunk_index: 1, ..good.clone() };

        let outcome = store_index_results(
            db.conn(),
            item.id,
            &[good, clashing],
            &["replacement".to_string()],
        );

        assert!(outcome.is_err());
        assert_eq!(
            keywords::list_for_item(db.conn(), item.id).unwrap(),
            vec!["original".to_string()],
            "the tag replacement must roll back with the chunk insert"
        );
    }

    fn list(db: &Database, name: &str, trigger: &str, sensitive: bool) -> uuid::Uuid {
        destinations::create(db.conn(), name, trigger, DestinationKind::List, None, false, sensitive, 0)
            .unwrap()
            .id
    }

    /// Records one chunk for a version, as a finished indexing pass would.
    fn mark_indexed(db: &Database, item_id: uuid::Uuid, edit_id: Option<uuid::Uuid>) {
        crate::repository::embeddings::insert_many(
            db.conn(),
            &[crate::repository::embeddings::Embedding {
                id: uuid::Uuid::new_v4(),
                item_id,
                edit_id,
                vector_ref: uuid::Uuid::new_v4().to_string(),
                chunk_index: 0,
                chunk_start_offset: 0,
                chunk_end_offset: 1,
            }],
        )
        .unwrap();
    }

    #[test]
    fn an_unindexed_capture_is_pending_as_its_original() {
        let db = migrated();
        let shop = list(&db, "Shopping", "shop", false);
        let item = items::capture(db.conn(), shop, "oat milk").unwrap();

        assert_eq!(pending_index_versions(db.conn()).unwrap(), vec![(item.id, None)]);
    }

    #[test]
    fn an_indexed_original_is_not_pending() {
        let db = migrated();
        let shop = list(&db, "Shopping", "shop", false);
        let item = items::capture(db.conn(), shop, "oat milk").unwrap();
        mark_indexed(&db, item.id, None);

        assert!(pending_index_versions(db.conn()).unwrap().is_empty());
    }

    #[test]
    fn an_unindexed_edit_is_pending_even_when_the_original_was_indexed() {
        let db = migrated();
        let shop = list(&db, "Shopping", "shop", false);
        let item = items::capture(db.conn(), shop, "milk").unwrap();
        mark_indexed(&db, item.id, None);
        let edit = crate::repository::edits::append(db.conn(), item.id, "oat milk").unwrap();

        assert_eq!(pending_index_versions(db.conn()).unwrap(), vec![(item.id, Some(edit.id))]);
    }

    /// Two edits appended back to back usually share a millisecond, so this also
    /// exercises the rowid tiebreak.
    #[test]
    fn only_the_latest_edit_is_reported_never_an_intermediate_one() {
        let db = migrated();
        let shop = list(&db, "Shopping", "shop", false);
        let item = items::capture(db.conn(), shop, "milk").unwrap();
        let _first = crate::repository::edits::append(db.conn(), item.id, "oat milk").unwrap();
        let second = crate::repository::edits::append(db.conn(), item.id, "oat milk, 2 cartons").unwrap();

        assert_eq!(pending_index_versions(db.conn()).unwrap(), vec![(item.id, Some(second.id))]);

        mark_indexed(&db, item.id, Some(second.id));
        assert!(
            pending_index_versions(db.conn()).unwrap().is_empty(),
            "the skipped intermediate edit must not be backfilled"
        );
    }

    #[test]
    fn sensitive_and_tombstoned_items_are_never_pending() {
        let db = migrated();
        let secret = list(&db, "Passwords", "pw", true);
        let shop = list(&db, "Shopping", "shop", false);
        items::capture(db.conn(), secret, "gmail: hunter2").unwrap();
        let gone = items::capture(db.conn(), shop, "old").unwrap();
        items::tombstone(db.conn(), gone.id).unwrap();

        assert!(pending_index_versions(db.conn()).unwrap().is_empty());
    }
}
