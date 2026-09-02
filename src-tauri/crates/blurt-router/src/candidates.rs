//! Depth-scoped destination lookup for the `@` picker — `MODULE_03_ROUTER.md` §2.
//!
//! §2.3 and §2.6 describe one interaction repeated at every depth: the
//! dropdown searches destinations *at the current hierarchy depth*, and each
//! resolved segment re-scopes the next lookup to its own children. Both
//! functions here take that depth as `Option<Uuid>` — `None` is the top level,
//! `Some(id)` is that destination's children.
//!
//! The reserved **Unsorted** destination is excluded from both: §1 and §3 make
//! it `isSystem` and hidden from the normal picker. **Random Thoughts** is
//! not excluded — §3 keeps it explicitly reachable by `@`, and only bars it
//! from natural-language matching ([`crate::nl`]).

use rusqlite::Connection;
use uuid::Uuid;

use blurt_schema::repository::destinations::{self, Destination};

use crate::error::Result;

/// Live, filtered dropdown contents for one hierarchy depth (§2.3).
///
/// `query` is what the user has typed since the `@`. An empty query lists
/// everything at that depth. Matching is case-insensitive: a prefix of the
/// trigger (the handle the user is actually typing) or a substring of the
/// name (so "groc" still finds a list named "Grocery Run").
pub fn candidates_at_depth(
    conn: &Connection,
    parent_id: Option<Uuid>,
    query: &str,
) -> Result<Vec<Destination>> {
    let needle = query.to_lowercase();
    Ok(visible_at_depth(conn, parent_id)?
        .into_iter()
        .filter(|d| {
            needle.is_empty()
                || d.trigger.to_lowercase().starts_with(&needle)
                || d.name.to_lowercase().contains(&needle)
        })
        .collect())
}

/// Resolves one already-typed chain segment to a real destination at this
/// depth, or `None` if nothing matches — which is what turns a segment into
/// the `+` create affordance of §2.5.
///
/// Trigger first, name second: triggers are the user-defined handles §1 says
/// the chain is written in, and the name fallback only exists so tapping a
/// dropdown entry whose trigger the user never memorised still resolves.
pub fn exact_match(
    conn: &Connection,
    parent_id: Option<Uuid>,
    segment: &str,
) -> Result<Option<Destination>> {
    let needle = segment.to_lowercase();
    let visible = visible_at_depth(conn, parent_id)?;

    if let Some(by_trigger) = visible.iter().find(|d| d.trigger.to_lowercase() == needle) {
        return Ok(Some(by_trigger.clone()));
    }
    Ok(visible.into_iter().find(|d| d.name.to_lowercase() == needle))
}

/// Live destinations at one depth, minus the ones the picker hides.
///
/// Case folding happens in Rust rather than SQL on purpose: SQLite's `LOWER`
/// is ASCII-only without ICU, which would silently fail to match a
/// non-ASCII destination name.
fn visible_at_depth(conn: &Connection, parent_id: Option<Uuid>) -> Result<Vec<Destination>> {
    Ok(destinations::list_children(conn, parent_id)?
        .into_iter()
        .filter(|d| !d.is_system)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations::DestinationKind;
    use blurt_schema::{Database, MasterKey};

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        db
    }

    fn make(db: &Database, name: &str, trigger: &str, parent: Option<Uuid>) -> Destination {
        destinations::create(db.conn(), name, trigger, DestinationKind::List, parent, false, false, 0).unwrap()
    }

    fn names(found: &[Destination]) -> Vec<&str> {
        found.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn an_empty_query_lists_everything_at_that_depth() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        make(&db, "Weekly", "weekly", Some(shopping.id));
        make(&db, "Monthly", "monthly", Some(shopping.id));

        let found = candidates_at_depth(db.conn(), Some(shopping.id), "").unwrap();
        assert_eq!(names(&found), vec!["Monthly", "Weekly"]);
    }

    #[test]
    fn the_system_unsorted_destination_is_hidden_from_the_picker() {
        let db = migrated();
        let found = candidates_at_depth(db.conn(), None, "").unwrap();
        assert!(
            !names(&found).contains(&"Unsorted"),
            "§1/§3 — Unsorted is isSystem and hidden from the normal @ picker"
        );
    }

    #[test]
    fn random_thoughts_stays_reachable_by_explicit_at() {
        let db = migrated();
        let found = candidates_at_depth(db.conn(), None, "").unwrap();
        assert!(
            names(&found).contains(&"Random Thoughts"),
            "§3 — barred from NL matching, but explicitly reachable via @"
        );
    }

    #[test]
    fn filters_by_trigger_prefix_case_insensitively() {
        let db = migrated();
        make(&db, "Shopping", "shop", None);
        make(&db, "Errands", "errands", None);

        let found = candidates_at_depth(db.conn(), None, "SH").unwrap();
        assert_eq!(names(&found), vec!["Shopping"]);
    }

    #[test]
    fn filters_by_name_substring() {
        let db = migrated();
        make(&db, "Grocery Run", "gr", None);
        make(&db, "Errands", "errands", None);

        let found = candidates_at_depth(db.conn(), None, "cery").unwrap();
        assert_eq!(names(&found), vec!["Grocery Run"]);
    }

    #[test]
    fn does_not_leak_candidates_from_another_depth() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        make(&db, "Weekly", "weekly", Some(shopping.id));

        let top = candidates_at_depth(db.conn(), None, "weekly").unwrap();
        assert!(top.is_empty(), "Weekly lives one level down, not at the top level");
    }

    #[test]
    fn exact_match_resolves_a_trigger_case_insensitively() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);

        let found = exact_match(db.conn(), None, "SHOP").unwrap().expect("trigger match");
        assert_eq!(found.id, shopping.id);
    }

    #[test]
    fn exact_match_falls_back_to_the_name() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);

        let found = exact_match(db.conn(), None, "shopping").unwrap().expect("name match");
        assert_eq!(found.id, shopping.id);
    }

    #[test]
    fn exact_match_prefers_a_trigger_over_another_destinations_name() {
        let db = migrated();
        // "list" is Errands' trigger and, separately, Old Lists' name.
        let errands = make(&db, "Errands", "list", None);
        make(&db, "list", "old", None);

        let found = exact_match(db.conn(), None, "list").unwrap().expect("resolves");
        assert_eq!(found.id, errands.id, "the trigger is the handle the chain is written in");
    }

    #[test]
    fn exact_match_returns_none_for_an_unknown_segment() {
        let db = migrated();
        assert!(exact_match(db.conn(), None, "nope").unwrap().is_none());
    }

    #[test]
    fn exact_match_never_resolves_the_hidden_system_destination() {
        let db = migrated();
        assert!(
            exact_match(db.conn(), None, "unsorted").unwrap().is_none(),
            "@unsorted must not be typeable — it is hidden from the picker"
        );
    }

    #[test]
    fn exact_match_is_scoped_to_the_given_depth() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        make(&db, "Weekly", "weekly", Some(shopping.id));

        assert!(exact_match(db.conn(), None, "weekly").unwrap().is_none());
        assert!(exact_match(db.conn(), Some(shopping.id), "weekly").unwrap().is_some());
    }

    #[test]
    fn exact_match_ignores_a_tombstoned_destination() {
        let db = migrated();
        let gone = make(&db, "Shopping", "shop", None);
        destinations::tombstone(db.conn(), gone.id).unwrap();

        assert!(exact_match(db.conn(), None, "shop").unwrap().is_none());
    }
}
