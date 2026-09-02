//! Embeddings repository — `MODULE_02_SCHEMA.md` §2.
//!
//! The join layer between SQLCipher and the LanceDB vector store: a row here
//! says what a stored vector *is* — which item, which text version, and the
//! character span it covers — while the vector itself lives in LanceDB under
//! [`Embedding::vector_ref`].
//!
//! This side is authoritative. A LanceDB vector with no row here is
//! unreachable, because nothing joins to it; a row here with no vector simply
//! never matches a search. That asymmetry is what lets Module 4 write the
//! vector first and the row second without a distributed transaction.

use rusqlite::Connection;
use uuid::Uuid;

use crate::error::Result;

/// One embedded chunk of one version of an item's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedding {
    pub id: Uuid,
    pub item_id: Uuid,
    /// Which text version this chunk came from. `None` is the original
    /// capture, which has no `edits` row by design — see migration `0002`.
    pub edit_id: Option<Uuid>,
    /// Key into the LanceDB table.
    pub vector_ref: String,
    pub chunk_index: i64,
    pub chunk_start_offset: i64,
    pub chunk_end_offset: i64,
}

fn row_to_embedding(row: &rusqlite::Row) -> rusqlite::Result<Embedding> {
    let id: String = row.get("id")?;
    let item_id: String = row.get("itemId")?;
    let edit_id: Option<String> = row.get("editId")?;

    Ok(Embedding {
        id: Uuid::parse_str(&id).expect("embeddings.id is always a UUID"),
        item_id: Uuid::parse_str(&item_id).expect("embeddings.itemId is always a UUID"),
        edit_id: edit_id.map(|e| Uuid::parse_str(&e).expect("embeddings.editId is always a UUID")),
        vector_ref: row.get("vectorRef")?,
        chunk_index: row.get("chunkIndex")?,
        chunk_start_offset: row.get("chunkStartOffset")?,
        chunk_end_offset: row.get("chunkEndOffset")?,
    })
}

const SELECT_COLUMNS: &str =
    "id, itemId, editId, vectorRef, chunkIndex, chunkStartOffset, chunkEndOffset";

/// Inserts every chunk row for one text version, in one transaction.
///
/// All-or-nothing on purpose: a half-written chunk set would leave some spans
/// of a note searchable and others silently not, with nothing to indicate
/// which.
pub fn insert_many(conn: &Connection, embeddings: &[Embedding]) -> Result<()> {
    if embeddings.is_empty() {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
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

/// Every chunk row for an item, across all its text versions, ordered by
/// version then chunk index.
pub fn list_for_item(conn: &Connection, item_id: Uuid) -> Result<Vec<Embedding>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM embeddings
         WHERE itemId = ?1
         ORDER BY editId IS NOT NULL, editId, chunkIndex"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([item_id.to_string()], row_to_embedding)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Looks a chunk row up by the LanceDB key a search hit came back with.
///
/// This is the read that turns a vector match into something displayable —
/// without it a hit is an opaque string.
pub fn get_by_vector_ref(conn: &Connection, vector_ref: &str) -> Result<Option<Embedding>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM embeddings WHERE vectorRef = ?1");
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query_map([vector_ref], row_to_embedding)?;
    rows.next().transpose().map_err(Into::into)
}

/// Deletes every chunk row for an item and returns the `vectorRef`s that were
/// removed, so the caller can drop the matching vectors from LanceDB.
///
/// A hard delete, and deliberately so: this is derived index data, not user
/// content. The tombstone rule in `BLUEPRINT.md` §4 protects what the user
/// wrote — the `items` and `edits` rows — and an embedding is neither. Keeping
/// tombstoned embeddings would mean either filtering them on every search or
/// surfacing deleted text as a result.
pub fn delete_for_item(conn: &Connection, item_id: Uuid) -> Result<Vec<String>> {
    let removed: Vec<String> = {
        let mut stmt = conn.prepare("SELECT vectorRef FROM embeddings WHERE itemId = ?1")?;
        let rows = stmt.query_map([item_id.to_string()], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    conn.execute("DELETE FROM embeddings WHERE itemId = ?1", [item_id.to_string()])?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::keyring::MasterKey;
    use crate::migrations;
    use crate::repository::destinations::{self, DestinationKind};
    use crate::repository::{edits, items};

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        db
    }

    fn an_item(db: &Database) -> Uuid {
        let destination = destinations::create(
            db.conn(), "Notes", "notes", DestinationKind::Note, None, false, false, 0,
        )
        .unwrap();
        items::capture(db.conn(), destination.id, "buy milk").unwrap().id
    }

    fn chunk(item_id: Uuid, edit_id: Option<Uuid>, index: i64, vector_ref: &str) -> Embedding {
        Embedding {
            id: Uuid::new_v4(),
            item_id,
            edit_id,
            vector_ref: vector_ref.to_string(),
            chunk_index: index,
            chunk_start_offset: index * 100,
            chunk_end_offset: index * 100 + 80,
        }
    }

    #[test]
    fn inserts_and_reads_back_chunk_rows() {
        let db = migrated();
        let item = an_item(&db);

        let written = vec![chunk(item, None, 0, "v0"), chunk(item, None, 1, "v1")];
        insert_many(db.conn(), &written).unwrap();

        let read = list_for_item(db.conn(), item).unwrap();
        assert_eq!(read, written);
    }

    #[test]
    fn inserting_nothing_is_a_no_op() {
        let db = migrated();
        let item = an_item(&db);
        insert_many(db.conn(), &[]).unwrap();
        assert!(list_for_item(db.conn(), item).unwrap().is_empty());
    }

    /// §3's per-version embedding: the original capture and a later edit both
    /// keep their own chunks, so searching either wording finds the item.
    #[test]
    fn the_original_and_each_edit_keep_their_own_chunks() {
        let db = migrated();
        let item = an_item(&db);
        let edit = edits::append(db.conn(), item, "buy oat milk").unwrap();

        insert_many(db.conn(), &[chunk(item, None, 0, "orig")]).unwrap();
        insert_many(db.conn(), &[chunk(item, Some(edit.id), 0, "edited")]).unwrap();

        let read = list_for_item(db.conn(), item).unwrap();
        assert_eq!(read.len(), 2, "an edit must not replace the original's embedding");
        assert_eq!(read[0].edit_id, None, "the original capture sorts first");
        assert_eq!(read[1].edit_id, Some(edit.id));
    }

    #[test]
    fn a_partly_failing_batch_writes_nothing() {
        let db = migrated();
        let item = an_item(&db);

        let mut clashing = chunk(item, None, 1, "v1");
        clashing.id = Uuid::new_v4();
        let good = chunk(item, None, 0, "v0");
        // Same primary key twice: the second insert fails.
        let duplicate = Embedding { id: good.id, ..chunk(item, None, 2, "v2") };

        let outcome = insert_many(db.conn(), &[good, duplicate, clashing]);

        assert!(outcome.is_err());
        assert!(
            list_for_item(db.conn(), item).unwrap().is_empty(),
            "a failed batch must not leave half a note searchable"
        );
    }

    #[test]
    fn finds_a_chunk_by_the_vector_ref_a_search_hit_carries() {
        let db = migrated();
        let item = an_item(&db);
        insert_many(db.conn(), &[chunk(item, None, 3, "lance-key")]).unwrap();

        let found = get_by_vector_ref(db.conn(), "lance-key").unwrap().expect("stored");
        assert_eq!(found.item_id, item);
        assert_eq!(found.chunk_index, 3);
        assert_eq!((found.chunk_start_offset, found.chunk_end_offset), (300, 380));
    }

    #[test]
    fn an_unknown_vector_ref_resolves_to_nothing() {
        let db = migrated();
        assert!(get_by_vector_ref(db.conn(), "never-stored").unwrap().is_none());
    }

    #[test]
    fn deleting_an_items_chunks_reports_the_vectors_to_drop() {
        let db = migrated();
        let item = an_item(&db);
        let other = an_item_in_another_destination(&db);
        insert_many(db.conn(), &[chunk(item, None, 0, "a"), chunk(item, None, 1, "b")]).unwrap();
        insert_many(db.conn(), &[chunk(other, None, 0, "keep")]).unwrap();

        let mut removed = delete_for_item(db.conn(), item).unwrap();
        removed.sort();

        assert_eq!(removed, vec!["a".to_string(), "b".to_string()]);
        assert!(list_for_item(db.conn(), item).unwrap().is_empty());
        assert_eq!(
            list_for_item(db.conn(), other).unwrap().len(),
            1,
            "another item's chunks must survive"
        );
    }

    #[test]
    fn deleting_chunks_for_an_item_that_has_none_is_harmless() {
        let db = migrated();
        let item = an_item(&db);
        assert!(delete_for_item(db.conn(), item).unwrap().is_empty());
    }

    fn an_item_in_another_destination(db: &Database) -> Uuid {
        let destination = destinations::create(
            db.conn(), "Other", "other", DestinationKind::List, None, false, false, 0,
        )
        .unwrap();
        items::capture(db.conn(), destination.id, "something else").unwrap().id
    }
}
