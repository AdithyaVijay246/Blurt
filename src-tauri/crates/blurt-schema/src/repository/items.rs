//! Items repository — `MODULE_02_SCHEMA.md` §2.
//!
//! Reparenting, resolving out of Unsorted, and dragging between lists are all
//! the same operation here: a write to `destinationId` via [`move_to`].

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::Result;
use crate::repository::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: Uuid,
    pub destination_id: Uuid,
    pub original_text: String,
    pub current_text: String,
    pub checked: Option<bool>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

pub(crate) fn row_to_item(row: &rusqlite::Row) -> rusqlite::Result<Item> {
    let id: String = row.get("id")?;
    let destination_id: String = row.get("destinationId")?;
    let checked: Option<i64> = row.get("checked")?;

    Ok(Item {
        id: Uuid::parse_str(&id).expect("items.id is always a UUID"),
        destination_id: Uuid::parse_str(&destination_id).expect("items.destinationId is always a UUID"),
        original_text: row.get("originalText")?,
        current_text: row.get("currentText")?,
        checked: checked.map(|c| c != 0),
        created_at: row.get("createdAt")?,
        deleted_at: row.get("deletedAt")?,
    })
}

const SELECT_COLUMNS: &str = "id, destinationId, originalText, currentText, checked, createdAt, deletedAt";

/// Captures a new item. `currentText` starts identical to `originalText`;
/// they diverge only once an edit is appended (`repository::edits::append`).
pub fn capture(conn: &Connection, destination_id: Uuid, original_text: &str) -> Result<Item> {
    let id = Uuid::new_v4();
    let created_at = now_ms();

    conn.execute(
        "INSERT INTO items (id, destinationId, originalText, currentText, createdAt)
         VALUES (?1, ?2, ?3, ?3, ?4)",
        rusqlite::params![id.to_string(), destination_id.to_string(), original_text, created_at],
    )?;

    Ok(Item {
        id,
        destination_id,
        original_text: original_text.to_string(),
        current_text: original_text.to_string(),
        checked: None,
        created_at,
        deleted_at: None,
    })
}

/// Fetches an item by id. Returns `None` for a missing or tombstoned row.
pub fn get_by_id(conn: &Connection, id: Uuid) -> Result<Option<Item>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM items WHERE id = ?1 AND deletedAt IS NULL");
    conn.query_row(&sql, [id.to_string()], row_to_item).optional().map_err(Into::into)
}

/// Updates `destinationId` only. Covers reparenting, resolving an Unsorted
/// item, and dragging between lists — per §1, all the same write.
pub fn move_to(conn: &Connection, id: Uuid, destination_id: Uuid) -> Result<()> {
    conn.execute(
        "UPDATE items SET destinationId = ?1 WHERE id = ?2",
        rusqlite::params![destination_id.to_string(), id.to_string()],
    )?;
    Ok(())
}

/// Sets `checked`. Meaningful only for items in `list`-type destinations by
/// convention; this crate does not enforce that — it's `blurt-app`/UI policy.
pub fn set_checked(conn: &Connection, id: Uuid, checked: Option<bool>) -> Result<()> {
    conn.execute(
        "UPDATE items SET checked = ?1 WHERE id = ?2",
        rusqlite::params![checked.map(|c| c as i64), id.to_string()],
    )?;
    Ok(())
}

/// Tombstones an item. Never a hard delete — sets `deletedAt` only.
pub fn tombstone(conn: &Connection, id: Uuid) -> Result<()> {
    conn.execute(
        "UPDATE items SET deletedAt = ?1 WHERE id = ?2",
        rusqlite::params![now_ms(), id.to_string()],
    )?;
    Ok(())
}

/// Lists live (non-tombstoned) items in a destination.
pub fn list_for_destination(conn: &Connection, destination_id: Uuid) -> Result<Vec<Item>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM items WHERE destinationId = ?1 AND deletedAt IS NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let items = stmt
        .query_map([destination_id.to_string()], row_to_item)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::keyring::MasterKey;
    use crate::migrations;
    use crate::repository::destinations::{self, DestinationKind};

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        db
    }

    fn a_destination(db: &Database) -> Uuid {
        destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0)
            .unwrap()
            .id
    }

    #[test]
    fn captures_an_item_with_matching_original_and_current_text() {
        let db = migrated();
        let destination_id = a_destination(&db);

        let item = capture(db.conn(), destination_id, "buy milk").unwrap();

        assert_eq!(item.original_text, "buy milk");
        assert_eq!(item.current_text, "buy milk");
        assert_eq!(item.destination_id, destination_id);
        assert!(item.deleted_at.is_none());
    }

    #[test]
    fn capture_into_an_unknown_destination_is_rejected() {
        let db = migrated();
        let result = capture(db.conn(), Uuid::new_v4(), "buy milk");
        assert!(result.is_err(), "the destinationId foreign key should reject an unknown destination");
    }

    #[test]
    fn get_by_id_returns_none_for_unknown_id() {
        let db = migrated();
        assert!(get_by_id(db.conn(), Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn moves_an_item_between_destinations_touching_only_destination_id() {
        let db = migrated();
        let from = a_destination(&db);
        let to = destinations::create(db.conn(), "Errands", "errands", DestinationKind::List, None, false, false, 0)
            .unwrap()
            .id;
        let item = capture(db.conn(), from, "buy milk").unwrap();

        move_to(db.conn(), item.id, to).unwrap();

        let moved = get_by_id(db.conn(), item.id).unwrap().unwrap();
        assert_eq!(moved.destination_id, to);
        assert_eq!(moved.original_text, "buy milk");
        assert_eq!(moved.current_text, "buy milk");
    }

    #[test]
    fn checks_and_unchecks_an_item() {
        let db = migrated();
        let destination_id = a_destination(&db);
        let item = capture(db.conn(), destination_id, "buy milk").unwrap();
        assert_eq!(item.checked, None);

        set_checked(db.conn(), item.id, Some(true)).unwrap();
        assert_eq!(get_by_id(db.conn(), item.id).unwrap().unwrap().checked, Some(true));

        set_checked(db.conn(), item.id, Some(false)).unwrap();
        assert_eq!(get_by_id(db.conn(), item.id).unwrap().unwrap().checked, Some(false));
    }

    #[test]
    fn tombstoning_removes_an_item_from_destination_listings_without_deleting_the_row() {
        let db = migrated();
        let destination_id = a_destination(&db);
        let item = capture(db.conn(), destination_id, "buy milk").unwrap();

        tombstone(db.conn(), item.id).unwrap();

        assert!(get_by_id(db.conn(), item.id).unwrap().is_none());
        assert!(list_for_destination(db.conn(), destination_id).unwrap().is_empty());

        let still_present: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM items WHERE id = ?1", [item.id.to_string()], |r| r.get(0))
            .unwrap();
        assert_eq!(still_present, 1, "tombstone must not hard-delete the row");
    }

    #[test]
    fn list_for_destination_returns_only_live_items_in_that_destination() {
        let db = migrated();
        let destination_id = a_destination(&db);
        let other_destination = destinations::create(db.conn(), "Errands", "errands", DestinationKind::List, None, false, false, 0)
            .unwrap()
            .id;

        let keep = capture(db.conn(), destination_id, "buy milk").unwrap();
        let _elsewhere = capture(db.conn(), other_destination, "pick up dry cleaning").unwrap();
        let deleted = capture(db.conn(), destination_id, "buy eggs").unwrap();
        tombstone(db.conn(), deleted.id).unwrap();

        let items = list_for_destination(db.conn(), destination_id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, keep.id);
    }
}
