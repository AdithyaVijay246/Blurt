//! Edit commands — thin wraps over `blurt_schema::repository::edits`.
//! See `commands/destinations.rs`'s module doc for why each command splits
//! into a testable `_impl` plus a one-line `#[tauri::command]` wrapper.

use tauri::State;

use blurt_schema::repository::edits;

use crate::commands::parse_uuid;
use crate::dto::EditDto;
use crate::error::{CommandError, CommandResult};
use crate::state::AppState;

fn append_edit_impl(state: &AppState, item_id: String, text: String) -> CommandResult<EditDto> {
    let item_id = parse_uuid(&item_id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    let edit = edits::append(db.conn(), item_id, &text)?;
    Ok(edit.into())
}

#[tauri::command]
pub fn append_edit(state: State<AppState>, item_id: String, text: String) -> CommandResult<EditDto> {
    append_edit_impl(state.inner(), item_id, text)
}

fn item_edit_history_impl(state: &AppState, item_id: String) -> CommandResult<Vec<EditDto>> {
    let item_id = parse_uuid(&item_id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    let history = edits::history_for_item(db.conn(), item_id)?;
    Ok(history.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn item_edit_history(state: State<AppState>, item_id: String) -> CommandResult<Vec<EditDto>> {
    item_edit_history_impl(state.inner(), item_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use blurt_schema::repository::{destinations, items, DestinationKind};
    use blurt_schema::{migrations, Database, MasterKey};

    fn unlocked_state() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        AppState { db: Mutex::new(Some(db)), ..Default::default() }
    }

    fn an_item(state: &AppState) -> String {
        let guard = state.db.lock().unwrap();
        let conn = guard.as_ref().unwrap().conn();
        let destination = destinations::create(conn, "Shopping", "shop", DestinationKind::List, None, false, false, 0).unwrap();
        items::capture(conn, destination.id, "buy milk").unwrap().id.to_string()
    }

    #[test]
    fn appends_an_edit() {
        let state = unlocked_state();
        let item_id = an_item(&state);

        let edit = append_edit_impl(&state, item_id.clone(), "buy oat milk".to_string()).unwrap();

        assert_eq!(edit.item_id, item_id);
        assert_eq!(edit.text, "buy oat milk");
    }

    #[test]
    fn append_edit_fails_when_locked() {
        let state = AppState::default();
        let err = append_edit_impl(&state, uuid::Uuid::new_v4().to_string(), "x".to_string()).unwrap_err();
        assert_eq!(err, CommandError::Locked);
    }

    #[test]
    fn append_edit_rejects_a_malformed_item_id() {
        let state = unlocked_state();
        let err = append_edit_impl(&state, "not-a-uuid".to_string(), "x".to_string()).unwrap_err();
        assert_eq!(err, CommandError::InvalidId("not-a-uuid".to_string()));
    }

    #[test]
    fn returns_full_edit_history_in_order() {
        let state = unlocked_state();
        let item_id = an_item(&state);

        append_edit_impl(&state, item_id.clone(), "buy oat milk".to_string()).unwrap();
        append_edit_impl(&state, item_id.clone(), "buy oat milk and eggs".to_string()).unwrap();

        let history = item_edit_history_impl(&state, item_id).unwrap();
        let texts: Vec<&str> = history.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, vec!["buy oat milk", "buy oat milk and eggs"]);
    }

    #[test]
    fn history_is_empty_for_a_never_edited_item() {
        let state = unlocked_state();
        let item_id = an_item(&state);
        assert!(item_edit_history_impl(&state, item_id).unwrap().is_empty());
    }
}
