//! Item commands — thin wraps over `blurt_schema::repository::items`.
//! See `commands/destinations.rs`'s module doc for why each command splits
//! into a testable `_impl` plus a one-line `#[tauri::command]` wrapper.

use tauri::State;

use blurt_schema::repository::items;

use crate::commands::parse_uuid;
use crate::dto::ItemDto;
use crate::error::{CommandError, CommandResult};
use crate::state::AppState;

fn capture_item_impl(state: &AppState, destination_id: String, text: String) -> CommandResult<ItemDto> {
    let destination_id = parse_uuid(&destination_id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    let captured = items::capture(db.conn(), destination_id, &text)?;
    Ok(captured.into())
}

#[tauri::command]
pub fn capture_item(state: State<AppState>, destination_id: String, text: String) -> CommandResult<ItemDto> {
    capture_item_impl(state.inner(), destination_id, text)
}

fn move_item_impl(state: &AppState, id: String, destination_id: String) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let destination_id = parse_uuid(&destination_id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    items::move_to(db.conn(), id, destination_id)?;
    Ok(())
}

#[tauri::command]
pub fn move_item(state: State<AppState>, id: String, destination_id: String) -> CommandResult<()> {
    move_item_impl(state.inner(), id, destination_id)
}

fn set_item_checked_impl(state: &AppState, id: String, checked: Option<bool>) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    items::set_checked(db.conn(), id, checked)?;
    Ok(())
}

#[tauri::command]
pub fn set_item_checked(state: State<AppState>, id: String, checked: Option<bool>) -> CommandResult<()> {
    set_item_checked_impl(state.inner(), id, checked)
}

fn tombstone_item_impl(state: &AppState, id: String) -> CommandResult<()> {
    let id = parse_uuid(&id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    items::tombstone(db.conn(), id)?;
    Ok(())
}

#[tauri::command]
pub fn tombstone_item(state: State<AppState>, id: String) -> CommandResult<()> {
    tombstone_item_impl(state.inner(), id)
}

fn list_items_for_destination_impl(state: &AppState, destination_id: String) -> CommandResult<Vec<ItemDto>> {
    let destination_id = parse_uuid(&destination_id)?;
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;
    let items = items::list_for_destination(db.conn(), destination_id)?;
    Ok(items.into_iter().map(Into::into).collect())
}

#[tauri::command]
pub fn list_items_for_destination(state: State<AppState>, destination_id: String) -> CommandResult<Vec<ItemDto>> {
    list_items_for_destination_impl(state.inner(), destination_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use blurt_schema::repository::{destinations, DestinationKind};
    use blurt_schema::{migrations, Database, MasterKey};

    fn unlocked_state() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        migrations::run(db.conn()).unwrap();
        AppState { db: Mutex::new(Some(db)), ..Default::default() }
    }

    fn a_destination(state: &AppState) -> String {
        let guard = state.db.lock().unwrap();
        destinations::create(
            guard.as_ref().unwrap().conn(),
            "Shopping",
            "shop",
            DestinationKind::List,
            None,
            false,
            false,
            0,
        )
        .unwrap()
        .id
        .to_string()
    }

    #[test]
    fn captures_an_item() {
        let state = unlocked_state();
        let destination_id = a_destination(&state);

        let dto = capture_item_impl(&state, destination_id.clone(), "buy milk".to_string()).unwrap();

        assert_eq!(dto.destination_id, destination_id);
        assert_eq!(dto.original_text, "buy milk");
        assert_eq!(dto.current_text, "buy milk");
        assert_eq!(dto.checked, None);
    }

    #[test]
    fn capture_item_fails_when_locked() {
        let state = AppState::default();
        let err = capture_item_impl(&state, uuid::Uuid::new_v4().to_string(), "buy milk".to_string()).unwrap_err();
        assert_eq!(err, CommandError::Locked);
    }

    #[test]
    fn capture_item_rejects_a_malformed_destination_id() {
        let state = unlocked_state();
        let err = capture_item_impl(&state, "not-a-uuid".to_string(), "buy milk".to_string()).unwrap_err();
        assert_eq!(err, CommandError::InvalidId("not-a-uuid".to_string()));
    }

    #[test]
    fn moves_an_item_between_destinations() {
        let state = unlocked_state();
        let from = a_destination(&state);
        let to = {
            let guard = state.db.lock().unwrap();
            destinations::create(
                guard.as_ref().unwrap().conn(),
                "Errands",
                "errands",
                DestinationKind::List,
                None,
                false,
                false,
                0,
            )
            .unwrap()
            .id
            .to_string()
        };
        let item = capture_item_impl(&state, from, "buy milk".to_string()).unwrap();

        move_item_impl(&state, item.id.clone(), to.clone()).unwrap();

        let items = list_items_for_destination_impl(&state, to).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, item.id);
    }

    #[test]
    fn checks_and_unchecks_an_item() {
        let state = unlocked_state();
        let destination_id = a_destination(&state);
        let item = capture_item_impl(&state, destination_id.clone(), "buy milk".to_string()).unwrap();

        set_item_checked_impl(&state, item.id.clone(), Some(true)).unwrap();
        let items = list_items_for_destination_impl(&state, destination_id).unwrap();
        assert_eq!(items[0].checked, Some(true));
    }

    #[test]
    fn tombstoning_removes_an_item_from_the_listing() {
        let state = unlocked_state();
        let destination_id = a_destination(&state);
        let item = capture_item_impl(&state, destination_id.clone(), "buy milk".to_string()).unwrap();

        tombstone_item_impl(&state, item.id).unwrap();

        let items = list_items_for_destination_impl(&state, destination_id).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn lists_only_live_items_in_a_destination() {
        let state = unlocked_state();
        let destination_id = a_destination(&state);
        let keep = capture_item_impl(&state, destination_id.clone(), "buy milk".to_string()).unwrap();
        let deleted = capture_item_impl(&state, destination_id.clone(), "buy eggs".to_string()).unwrap();
        tombstone_item_impl(&state, deleted.id).unwrap();

        let items = list_items_for_destination_impl(&state, destination_id).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, keep.id);
    }
}
