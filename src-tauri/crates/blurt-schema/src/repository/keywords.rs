//! Keywords repository — `MODULE_02_SCHEMA.md` §2.
//!
//! Cheap exact-match tags extracted locally by Module 4 (`YAKE`, no LLM), used
//! two ways: as the keyword half of §4's hybrid ranking, and as the visible
//! tags `MODULE_04_EMBEDDINGS_RAG.md` §3 shows under a note.
//!
//! Unlike embeddings, keywords are **replaced** rather than accumulated. An
//! embedding per text version is the point — §3 wants the old wording to stay
//! searchable — but a tag list is a statement about what an item is *now*, and
//! showing tags drawn from text the user has since rewritten would be wrong on
//! the screen rather than merely redundant. [`replace_for_item`] is therefore
//! the only way to write them.

use rusqlite::Connection;
use uuid::Uuid;

use crate::error::Result;

/// Replaces an item's tags with `keywords`, in one transaction.
///
/// Order is preserved on read: Module 4 returns keywords most-important-first,
/// and that is the order a tag list renders in.
pub fn replace_for_item(conn: &Connection, item_id: Uuid, keywords: &[String]) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM keywords WHERE itemId = ?1", [item_id.to_string()])?;
    {
        let mut stmt = tx.prepare("INSERT INTO keywords (itemId, keyword) VALUES (?1, ?2)")?;
        for keyword in keywords {
            stmt.execute(rusqlite::params![item_id.to_string(), keyword])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// An item's tags, in the order they were written.
pub fn list_for_item(conn: &Connection, item_id: Uuid) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT keyword FROM keywords WHERE itemId = ?1 ORDER BY rowid")?;
    let rows = stmt.query_map([item_id.to_string()], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Items carrying any of `keywords`, with how many matched.
///
/// The keyword half of §4's hybrid ranking: a cheap exact-match lookup that
/// catches the terms a paraphrase-tolerant vector search can miss. Tombstoned
/// items and items in tombstoned or `isSensitive` destinations are excluded
/// here rather than by the caller, for the same structural reason
/// [`super::indexing`] exists — a sensitive item should be unreachable through
/// every path, not just the one that remembered to filter.
pub fn items_matching_any(conn: &Connection, keywords: &[String]) -> Result<Vec<(Uuid, usize)>> {
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = std::iter::repeat_n("?", keywords.len()).collect::<Vec<_>>().join(", ");
    let sql = format!(
        "SELECT keywords.itemId, count(*) AS hits
         FROM keywords
         JOIN items ON items.id = keywords.itemId
         JOIN destinations ON destinations.id = items.destinationId
         WHERE keywords.keyword IN ({placeholders})
           AND items.deletedAt IS NULL
           AND destinations.deletedAt IS NULL
           AND destinations.isSensitive = 0
         GROUP BY keywords.itemId
         ORDER BY hits DESC, keywords.itemId"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(keywords.iter()), |row| {
        let id: String = row.get(0)?;
        let hits: i64 = row.get(1)?;
        Ok((
            Uuid::parse_str(&id).expect("keywords.itemId is always a UUID"),
            hits as usize,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
}

/// Removes an item's tags outright.
///
/// A hard delete, like embeddings: derived index data, not the user's words.
pub fn delete_for_item(conn: &Connection, item_id: Uuid) -> Result<()> {
    conn.execute("DELETE FROM keywords WHERE itemId = ?1", [item_id.to_string()])?;
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

    fn item_in(db: &Database, name: &str, trigger: &str, sensitive: bool) -> Uuid {
        let destination = destinations::create(
            db.conn(), name, trigger, DestinationKind::List, None, false, sensitive, 0,
        )
        .unwrap();
        items::capture(db.conn(), destination.id, "text").unwrap().id
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn writes_and_reads_back_tags_in_order() {
        let db = migrated();
        let item = item_in(&db, "Notes", "notes", false);

        replace_for_item(db.conn(), item, &strings(&["billing", "migration", "payments"])).unwrap();

        assert_eq!(
            list_for_item(db.conn(), item).unwrap(),
            strings(&["billing", "migration", "payments"]),
            "importance order is what a tag list renders in"
        );
    }

    #[test]
    fn replacing_discards_the_previous_tags() {
        let db = migrated();
        let item = item_in(&db, "Notes", "notes", false);

        replace_for_item(db.conn(), item, &strings(&["old", "stale"])).unwrap();
        replace_for_item(db.conn(), item, &strings(&["fresh"])).unwrap();

        assert_eq!(
            list_for_item(db.conn(), item).unwrap(),
            strings(&["fresh"]),
            "tags describe the item now, not every wording it has ever had"
        );
    }

    #[test]
    fn replacing_with_nothing_clears_the_tags() {
        let db = migrated();
        let item = item_in(&db, "Notes", "notes", false);
        replace_for_item(db.conn(), item, &strings(&["something"])).unwrap();

        replace_for_item(db.conn(), item, &[]).unwrap();

        assert!(list_for_item(db.conn(), item).unwrap().is_empty());
    }

    #[test]
    fn an_item_with_no_tags_reads_back_empty() {
        let db = migrated();
        let item = item_in(&db, "Notes", "notes", false);
        assert!(list_for_item(db.conn(), item).unwrap().is_empty());
    }

    #[test]
    fn finds_items_by_any_matching_keyword_ranked_by_hit_count() {
        let db = migrated();
        let both = item_in(&db, "A", "a", false);
        let one = item_in(&db, "B", "b", false);
        let neither = item_in(&db, "C", "c", false);

        replace_for_item(db.conn(), both, &strings(&["billing", "migration"])).unwrap();
        replace_for_item(db.conn(), one, &strings(&["billing", "unrelated"])).unwrap();
        replace_for_item(db.conn(), neither, &strings(&["something else"])).unwrap();

        let hits = items_matching_any(db.conn(), &strings(&["billing", "migration"])).unwrap();

        assert_eq!(hits, vec![(both, 2), (one, 1)]);
    }

    #[test]
    fn matching_nothing_returns_nothing() {
        let db = migrated();
        assert!(items_matching_any(db.conn(), &[]).unwrap().is_empty());
        assert!(items_matching_any(db.conn(), &strings(&["absent"])).unwrap().is_empty());
    }

    /// The same structural guarantee `indexing::items_for_indexing` makes: a
    /// sensitive item must be unreachable through *every* path, including the
    /// keyword half of hybrid search.
    #[test]
    fn a_sensitive_destinations_items_never_match_a_keyword_query() {
        let db = migrated();
        let secret = item_in(&db, "Passwords", "pw", true);
        replace_for_item(db.conn(), secret, &strings(&["gmail"])).unwrap();

        assert!(
            items_matching_any(db.conn(), &strings(&["gmail"])).unwrap().is_empty(),
            "a sensitive item leaked through keyword search"
        );
    }

    #[test]
    fn a_tombstoned_item_never_matches_a_keyword_query() {
        let db = migrated();
        let item = item_in(&db, "Notes", "notes", false);
        replace_for_item(db.conn(), item, &strings(&["billing"])).unwrap();
        items::tombstone(db.conn(), item).unwrap();

        assert!(items_matching_any(db.conn(), &strings(&["billing"])).unwrap().is_empty());
    }

    #[test]
    fn an_item_in_a_tombstoned_destination_never_matches() {
        let db = migrated();
        let destination = destinations::create(
            db.conn(), "Notes", "notes", DestinationKind::List, None, false, false, 0,
        )
        .unwrap();
        let item = items::capture(db.conn(), destination.id, "text").unwrap();
        replace_for_item(db.conn(), item.id, &strings(&["billing"])).unwrap();
        destinations::tombstone(db.conn(), destination.id).unwrap();

        assert!(items_matching_any(db.conn(), &strings(&["billing"])).unwrap().is_empty());
    }

    #[test]
    fn deleting_removes_only_that_items_tags() {
        let db = migrated();
        let doomed = item_in(&db, "A", "a", false);
        let kept = item_in(&db, "B", "b", false);
        replace_for_item(db.conn(), doomed, &strings(&["gone"])).unwrap();
        replace_for_item(db.conn(), kept, &strings(&["stays"])).unwrap();

        delete_for_item(db.conn(), doomed).unwrap();

        assert!(list_for_item(db.conn(), doomed).unwrap().is_empty());
        assert_eq!(list_for_item(db.conn(), kept).unwrap(), strings(&["stays"]));
    }
}
