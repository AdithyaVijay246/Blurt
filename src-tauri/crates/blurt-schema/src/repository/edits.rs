//! Edits repository — `MODULE_02_SCHEMA.md` §2.
//!
//! Append-only: [`append`] inserts an `edits` row and updates the parent
//! item's `currentText` in the same call, atomically — one logical write from
//! the caller's perspective. `originalText` is never touched here.

use rusqlite::Connection;
use uuid::Uuid;

use crate::error::Result;
use crate::repository::now_ms;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub id: Uuid,
    pub item_id: Uuid,
    pub text: String,
    pub edited_at: i64,
}

fn row_to_edit(row: &rusqlite::Row) -> rusqlite::Result<Edit> {
    let id: String = row.get("id")?;
    let item_id: String = row.get("itemId")?;

    Ok(Edit {
        id: Uuid::parse_str(&id).expect("edits.id is always a UUID"),
        item_id: Uuid::parse_str(&item_id).expect("edits.itemId is always a UUID"),
        text: row.get("text")?,
        edited_at: row.get("editedAt")?,
    })
}

/// Appends an edit and updates `items.currentText` to match, in one
/// transaction. `originalText` is never written here.
pub fn append(conn: &Connection, item_id: Uuid, text: &str) -> Result<Edit> {
    let id = Uuid::new_v4();
    let edited_at = now_ms();

    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO edits (id, itemId, text, editedAt) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id.to_string(), item_id.to_string(), text, edited_at],
    )?;
    tx.execute(
        "UPDATE items SET currentText = ?1 WHERE id = ?2",
        rusqlite::params![text, item_id.to_string()],
    )?;
    tx.commit()?;

    Ok(Edit { id, item_id, text: text.to_string(), edited_at })
}

/// Full edit history for an item, oldest first.
pub fn history_for_item(conn: &Connection, item_id: Uuid) -> Result<Vec<Edit>> {
    let mut stmt = conn.prepare(
        // rowid as a tiebreaker: editedAt is millisecond resolution, so two
        // edits appended within the same millisecond would otherwise have no
        // guaranteed relative order.
        "SELECT id, itemId, text, editedAt FROM edits WHERE itemId = ?1 ORDER BY editedAt ASC, rowid ASC",
    )?;
    let edits = stmt
        .query_map([item_id.to_string()], row_to_edit)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(edits)
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

    fn an_item(db: &Database) -> items::Item {
        let destination_id = destinations::create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0)
            .unwrap()
            .id;
        items::capture(db.conn(), destination_id, "buy milk").unwrap()
    }

    #[test]
    fn appending_an_edit_updates_current_text_but_never_original_text() {
        let db = migrated();
        let item = an_item(&db);

        let edit = append(db.conn(), item.id, "buy oat milk").unwrap();
        assert_eq!(edit.text, "buy oat milk");
        assert_eq!(edit.item_id, item.id);

        let updated = items::get_by_id(db.conn(), item.id).unwrap().unwrap();
        assert_eq!(updated.current_text, "buy oat milk");
        assert_eq!(updated.original_text, "buy milk", "original text must never change");
    }

    #[test]
    fn multiple_edits_preserve_full_ordered_history() {
        let db = migrated();
        let item = an_item(&db);

        append(db.conn(), item.id, "buy oat milk").unwrap();
        append(db.conn(), item.id, "buy oat milk and eggs").unwrap();

        let history = history_for_item(db.conn(), item.id).unwrap();
        let texts: Vec<&str> = history.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["buy oat milk", "buy oat milk and eggs"]);

        let latest = items::get_by_id(db.conn(), item.id).unwrap().unwrap();
        assert_eq!(latest.current_text, "buy oat milk and eggs");
    }

    #[test]
    fn history_is_empty_for_a_never_edited_item() {
        let db = migrated();
        let item = an_item(&db);
        assert!(history_for_item(db.conn(), item.id).unwrap().is_empty());
    }
}
