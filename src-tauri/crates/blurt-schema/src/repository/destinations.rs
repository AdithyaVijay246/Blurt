//! Destinations repository — `MODULE_02_SCHEMA.md` §1–2.
//!
//! Paths are never stored: [`path`] is the one place that walks the
//! `parentId` chain on read, per §1.

use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::Result;
use crate::repository::now_ms;

/// `destinations.type` — distinguishes checkable lists from append-only notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationKind {
    List,
    Note,
}

impl DestinationKind {
    fn as_str(self) -> &'static str {
        match self {
            DestinationKind::List => "list",
            DestinationKind::Note => "note",
        }
    }

    /// Only ever called on a value read back from `destinations.type`, which
    /// carries a `CHECK (type IN ('list', 'note'))` constraint — an
    /// unrecognized value means the DDL itself changed underneath this code.
    fn from_str(s: &str) -> Self {
        match s {
            "list" => DestinationKind::List,
            "note" => DestinationKind::Note,
            other => unreachable!("destinations.type CHECK constraint allows only list/note, got {other:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub name: String,
    pub trigger: String,
    pub kind: DestinationKind,
    pub is_system: bool,
    pub is_sensitive: bool,
    pub sort_order: i64,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

fn row_to_destination(row: &rusqlite::Row) -> rusqlite::Result<Destination> {
    let id: String = row.get("id")?;
    let parent_id: Option<String> = row.get("parentId")?;
    let kind: String = row.get("type")?;
    let is_system: i64 = row.get("isSystem")?;
    let is_sensitive: i64 = row.get("isSensitive")?;

    Ok(Destination {
        id: Uuid::parse_str(&id).expect("destinations.id is always a UUID"),
        parent_id: parent_id.map(|p| Uuid::parse_str(&p).expect("destinations.parentId is always a UUID")),
        name: row.get("name")?,
        trigger: row.get("trigger")?,
        kind: DestinationKind::from_str(&kind),
        is_system: is_system != 0,
        is_sensitive: is_sensitive != 0,
        sort_order: row.get("sortOrder")?,
        created_at: row.get("createdAt")?,
        deleted_at: row.get("deletedAt")?,
    })
}

const SELECT_COLUMNS: &str =
    "id, parentId, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt, deletedAt";

/// Creates a new destination and returns the row as stored.
#[allow(clippy::too_many_arguments)]
pub fn create(
    conn: &Connection,
    name: &str,
    trigger: &str,
    kind: DestinationKind,
    parent_id: Option<Uuid>,
    is_system: bool,
    is_sensitive: bool,
    sort_order: i64,
) -> Result<Destination> {
    let id = Uuid::new_v4();
    let created_at = now_ms();

    conn.execute(
        "INSERT INTO destinations (id, parentId, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            id.to_string(),
            parent_id.map(|p| p.to_string()),
            name,
            trigger,
            kind.as_str(),
            is_system as i64,
            is_sensitive as i64,
            sort_order,
            created_at,
        ],
    )?;

    Ok(Destination {
        id,
        parent_id,
        name: name.to_string(),
        trigger: trigger.to_string(),
        kind,
        is_system,
        is_sensitive,
        sort_order,
        created_at,
        deleted_at: None,
    })
}

/// Fetches a destination by id. Returns `None` for a missing or tombstoned row.
pub fn get_by_id(conn: &Connection, id: Uuid) -> Result<Option<Destination>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM destinations WHERE id = ?1 AND deletedAt IS NULL");
    conn.query_row(&sql, [id.to_string()], row_to_destination)
        .optional()
        .map_err(Into::into)
}

/// Updates `name` in place. Renaming never touches `id` or `trigger`.
pub fn rename(conn: &Connection, id: Uuid, new_name: &str) -> Result<()> {
    conn.execute(
        "UPDATE destinations SET name = ?1 WHERE id = ?2",
        rusqlite::params![new_name, id.to_string()],
    )?;
    Ok(())
}

/// Updates `parentId`. `None` moves the destination to the top level.
pub fn reparent(conn: &Connection, id: Uuid, new_parent_id: Option<Uuid>) -> Result<()> {
    conn.execute(
        "UPDATE destinations SET parentId = ?1 WHERE id = ?2",
        rusqlite::params![new_parent_id.map(|p| p.to_string()), id.to_string()],
    )?;
    Ok(())
}

/// Tombstones a destination. Never a hard delete — sets `deletedAt` only.
pub fn tombstone(conn: &Connection, id: Uuid) -> Result<()> {
    conn.execute(
        "UPDATE destinations SET deletedAt = ?1 WHERE id = ?2",
        rusqlite::params![now_ms(), id.to_string()],
    )?;
    Ok(())
}

/// Walks the `parentId` chain and returns the ordered ancestor chain from
/// root to `id` itself (e.g. `[Shopping, Weekly, Produce]`). Per §1, this is
/// the only place a display path is ever assembled — it is never stored.
pub fn path(conn: &Connection, id: Uuid) -> Result<Vec<Destination>> {
    let mut chain = Vec::new();
    let mut current = get_by_id(conn, id)?;

    while let Some(destination) = current {
        let parent_id = destination.parent_id;
        chain.push(destination);
        current = match parent_id {
            Some(parent_id) => get_by_id(conn, parent_id)?,
            None => None,
        };
    }

    chain.reverse();
    Ok(chain)
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
    fn creates_and_reads_back_a_destination() {
        let db = migrated();
        let created = create(
            db.conn(),
            "Shopping",
            "shop",
            DestinationKind::List,
            None,
            false,
            false,
            0,
        )
        .unwrap();

        let fetched = get_by_id(db.conn(), created.id).unwrap().expect("just created");
        assert_eq!(fetched, created);
        assert_eq!(fetched.kind, DestinationKind::List);
        assert!(!fetched.is_system);
        assert!(!fetched.is_sensitive);
        assert!(fetched.deleted_at.is_none());
    }

    #[test]
    fn get_by_id_returns_none_for_unknown_id() {
        let db = migrated();
        assert!(get_by_id(db.conn(), Uuid::new_v4()).unwrap().is_none());
    }

    #[test]
    fn renames_a_destination() {
        let db = migrated();
        let created = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();

        rename(db.conn(), created.id, "Groceries").unwrap();

        let fetched = get_by_id(db.conn(), created.id).unwrap().unwrap();
        assert_eq!(fetched.name, "Groceries");
        assert_eq!(fetched.trigger, "shop", "renaming must not touch the trigger");
    }

    #[test]
    fn reparents_a_destination_including_back_to_top_level() {
        let db = migrated();
        let parent = create(db.conn(), "Errands", "errands", DestinationKind::List, None, false, false, 0).unwrap();
        let child = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();

        reparent(db.conn(), child.id, Some(parent.id)).unwrap();
        let fetched = get_by_id(db.conn(), child.id).unwrap().unwrap();
        assert_eq!(fetched.parent_id, Some(parent.id));

        reparent(db.conn(), child.id, None).unwrap();
        let fetched = get_by_id(db.conn(), child.id).unwrap().unwrap();
        assert_eq!(fetched.parent_id, None);
    }

    #[test]
    fn tombstoning_hides_a_destination_without_deleting_the_row() {
        let db = migrated();
        let created = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();

        tombstone(db.conn(), created.id).unwrap();

        assert!(get_by_id(db.conn(), created.id).unwrap().is_none());

        let still_present: i64 = db
            .conn()
            .query_row(
                "SELECT count(*) FROM destinations WHERE id = ?1",
                [created.id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still_present, 1, "tombstone must not hard-delete the row");
    }

    #[test]
    fn a_tombstoned_destinations_trigger_can_be_reused() {
        let db = migrated();
        let first = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        tombstone(db.conn(), first.id).unwrap();

        let second = create(db.conn(), "Shopping Again", "shop", DestinationKind::List, None, false, false, 0);
        assert!(second.is_ok(), "trigger should be reusable once the original is tombstoned");
    }

    #[test]
    fn path_resolves_a_multi_level_chain_without_storing_it() {
        let db = migrated();
        let top = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        let mid = create(db.conn(), "Weekly", "weekly", DestinationKind::List, Some(top.id), false, false, 0).unwrap();
        let leaf = create(db.conn(), "Produce", "produce", DestinationKind::List, Some(mid.id), false, false, 0).unwrap();

        let chain = path(db.conn(), leaf.id).unwrap();
        let names: Vec<&str> = chain.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Shopping", "Weekly", "Produce"]);
    }

    #[test]
    fn path_for_a_top_level_destination_is_itself_only() {
        let db = migrated();
        let top = create(db.conn(), "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();

        let chain = path(db.conn(), top.id).unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].id, top.id);
    }
}
