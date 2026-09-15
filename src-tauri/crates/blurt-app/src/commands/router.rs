//! Capture commands — `MODULE_03_ROUTER.md`.
//!
//! The seam between deciding and writing. `blurt-router` resolves text to a
//! [`RoutingDecision`] and never touches a row; this module performs the write
//! that decision implies and tells the frontend what happened.
//!
//! ## Every path writes
//!
//! `CLAUDE.md`'s "capture is never blocked" and "nothing is ever unrouted" mean
//! the same thing here: **every** branch below ends in an `items` row. There is
//! no decision that returns "tell the user something first". Even the branch
//! where the chain names a destination that does not exist saves to Unsorted
//! and hands the creation details back afterwards, so the user can create it
//! and move the item — an ordinary move, per §3, structurally no different from
//! dragging a list around.
//!
//! That branch deserves a note, because §2.5 might suggest otherwise. The `+`
//! create it describes is a **tap in the live picker**, which is Module 6 and
//! happens *before* submit. By the time text reaches this command an unresolved
//! segment means the picker did not resolve it — the user typed past it, or
//! there was no picker at all (voice). Silently creating a destination here
//! would turn a typo into a permanent list.
//!
//! ## Events
//!
//! §3 of `MODULE_01_ARCHITECTURE.md` asks for event-driven frontend updates:
//! the return value answers the caller, the event updates every other open
//! view. Emission lives in the thin `#[tauri::command]` wrapper rather than the
//! `_impl`, because an `AppHandle` is exactly what the `_impl` split exists to
//! avoid needing (decision #13). The wrapper stays a delegation plus one
//! `emit`.

use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};

use blurt_router::{route, RoutingDecision};
use blurt_schema::repository::{destinations, items};

use crate::dto::ItemDto;
use crate::error::{CommandError, CommandResult};
use crate::state::AppState;

/// Event name for a capture landing, so background views can refresh.
pub const ITEM_CAPTURED_EVENT: &str = "item-captured";

/// What a capture actually did.
///
/// Mirrors [`RoutingDecision`] rather than collapsing to "saved", because the
/// frontend renders a different affordance per branch: nothing for `Resolved`,
/// §3's dismissible quick-confirm chip for `NlMatched`, the Unsorted badge for
/// `Unrouted`, and a create-and-move offer for `NeedsDestination`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CaptureOutcomeDto {
    /// Every segment resolved. Filed immediately, no confirmation (§3).
    Resolved { item: ItemDto },

    /// Natural-language matched confidently. Already saved; the chip is
    /// dismissible, never a prompt (§3).
    NlMatched { item: ItemDto, confidence: f32 },

    /// Nothing matched. Lives in Unsorted, which is a real destination rather
    /// than a holding pen, and bumps its badge (§3).
    Unrouted { item: ItemDto },

    /// The chain named a destination that does not exist. The item is in
    /// Unsorted; these fields are what §2.5's create would need.
    NeedsDestination {
        item: ItemDto,
        parent_id: Option<String>,
        name: String,
        trigger: String,
        remaining_segments: Vec<String>,
    },
}

impl CaptureOutcomeDto {
    /// The row that was written, whichever branch wrote it.
    pub fn item(&self) -> &ItemDto {
        match self {
            Self::Resolved { item }
            | Self::NlMatched { item, .. }
            | Self::Unrouted { item }
            | Self::NeedsDestination { item, .. } => item,
        }
    }
}

fn capture_item_via_router_impl(state: &AppState, raw_text: &str) -> CommandResult<CaptureOutcomeDto> {
    let guard = state.db.lock().unwrap();
    let db = guard.as_ref().ok_or(CommandError::Locked)?;

    let routing = route(db.conn(), raw_text)?;
    if routing.text.trim().is_empty() {
        // The router leaves this to the caller by design: a chain with no body
        // ("@weekly" alone) is a routing gesture, not something to file.
        return Err(CommandError::EmptyCapture);
    }

    let outcome = match routing.decision {
        RoutingDecision::Resolved { destination_id } => CaptureOutcomeDto::Resolved {
            item: items::capture(db.conn(), destination_id, &routing.text)?.into(),
        },
        RoutingDecision::NlMatched {
            destination_id,
            confidence,
        } => CaptureOutcomeDto::NlMatched {
            item: items::capture(db.conn(), destination_id, &routing.text)?.into(),
            confidence,
        },
        RoutingDecision::Unrouted => CaptureOutcomeDto::Unrouted {
            item: items::capture(db.conn(), destinations::UNSORTED_ID, &routing.text)?.into(),
        },
        RoutingDecision::Create {
            parent_id,
            name,
            trigger,
            remaining_segments,
        } => CaptureOutcomeDto::NeedsDestination {
            item: items::capture(db.conn(), destinations::UNSORTED_ID, &routing.text)?.into(),
            parent_id: parent_id.map(|p| p.to_string()),
            name,
            trigger,
            remaining_segments,
        },
    };

    Ok(outcome)
}

/// One rewritten "at" and the text it replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpokenAtDto {
    /// Where the `@` sits in the normalized text.
    pub offset: usize,
    /// The replaced text verbatim, so dismissing restores capitalisation and
    /// spacing exactly rather than an approximation of them.
    pub original: String,
}

/// A transcript rewritten into `@`-syntax, plus what was rewritten.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedVoiceDto {
    pub text: String,
    pub replacements: Vec<SpokenAtDto>,
}

/// Voice pre-processing — `MODULE_03_ROUTER.md` §4.
///
/// **Normalizes only; it writes nothing.** §4 puts a review step between
/// transcription and capture: every standalone "at" is highlighted and swapped
/// to `@`, the user dismisses any that were not routing, and only what survives
/// is resolved. Capturing here would delete that step.
///
/// It matters because §4 accepts a real false-positive rate to get there —
/// "meet Alex at 9pm" *will* be flagged, which the doc calls a known tradeoff,
/// "a one-time per-sentence dismissal cost, judged cheaper than silently
/// mis-routing". Normalizing and capturing in one call would produce exactly
/// the silent mis-routing that tradeoff was made to avoid. An earlier draft of
/// this module did that; the roadmap had sketched it that way too.
///
/// `replacements` is what makes dismissal possible, carrying each original
/// substring verbatim. The reviewed text then goes to
/// [`capture_item_via_router`] like any typed input — §4's "voice adds zero new
/// parsing logic".
fn normalize_voice_transcript_impl(transcript: &str) -> NormalizedVoiceDto {
    let normalized = blurt_router::voice::normalize_spoken_at(transcript);
    NormalizedVoiceDto {
        text: normalized.text,
        replacements: normalized
            .replacements
            .into_iter()
            .map(|r| SpokenAtDto {
                offset: r.offset,
                original: r.original,
            })
            .collect(),
    }
}

#[tauri::command]
pub fn capture_item_via_router(
    app: tauri::AppHandle,
    state: State<AppState>,
    raw_text: String,
) -> CommandResult<CaptureOutcomeDto> {
    let outcome = capture_item_via_router_impl(state.inner(), &raw_text)?;
    let _ = app.emit(ITEM_CAPTURED_EVENT, &outcome);
    Ok(outcome)
}

/// No state and no event: this reads nothing and writes nothing.
#[tauri::command]
pub fn normalize_voice_transcript(transcript: String) -> NormalizedVoiceDto {
    normalize_voice_transcript_impl(&transcript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations::DestinationKind;
    use blurt_schema::{Database, MasterKey};

    /// An unlocked state with a migrated in-memory database.
    fn unlocked() -> AppState {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        let state = AppState::default();
        *state.db.lock().unwrap() = Some(db);
        state
    }

    fn destination(state: &AppState, name: &str, trigger: &str) -> uuid::Uuid {
        let guard = state.db.lock().unwrap();
        destinations::create(
            guard.as_ref().unwrap().conn(),
            name,
            trigger,
            DestinationKind::List,
            None,
            false,
            false,
            0,
        )
        .unwrap()
        .id
    }

    fn item_count(state: &AppState, destination_id: uuid::Uuid) -> usize {
        let guard = state.db.lock().unwrap();
        items::list_for_destination(guard.as_ref().unwrap().conn(), destination_id)
            .unwrap()
            .len()
    }

    #[test]
    fn a_resolved_chain_files_the_item_and_strips_the_chain_from_its_text() {
        let state = unlocked();
        let weekly = destination(&state, "Weekly", "weekly");

        let outcome = capture_item_via_router_impl(&state, "buy tomatoes @weekly").unwrap();

        match &outcome {
            CaptureOutcomeDto::Resolved { item } => {
                assert_eq!(item.destination_id, weekly.to_string());
                assert_eq!(item.current_text, "buy tomatoes", "the chain is routing, not content");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn unmatched_freeform_text_lands_in_unsorted() {
        // "Nothing is ever unrouted" — Unsorted is a real destination, so this
        // is a normal row, not a special case.
        let state = unlocked();

        let outcome = capture_item_via_router_impl(&state, "the sky was purple today").unwrap();

        match &outcome {
            CaptureOutcomeDto::Unrouted { item } => {
                assert_eq!(item.destination_id, destinations::UNSORTED_ID.to_string());
            }
            other => panic!("expected Unrouted, got {other:?}"),
        }
    }

    #[test]
    fn a_chain_naming_something_that_does_not_exist_still_saves_the_capture() {
        // The branch that would be easiest to get wrong: refusing here, or
        // silently creating a destination, both break a stated rule.
        let state = unlocked();

        let outcome = capture_item_via_router_impl(&state, "buy milk @groceries").unwrap();

        match &outcome {
            CaptureOutcomeDto::NeedsDestination {
                item, name, trigger, ..
            } => {
                assert_eq!(item.destination_id, destinations::UNSORTED_ID.to_string());
                assert_eq!(item.current_text, "buy milk");
                assert_eq!(name, "groceries");
                assert!(!trigger.is_empty(), "the UI needs a trigger to offer");
            }
            other => panic!("expected NeedsDestination, got {other:?}"),
        }
    }

    #[test]
    fn an_unresolved_chain_does_not_create_the_destination() {
        // §2.5's `+` create is a tap in the picker, before submit. Creating
        // here would turn every typo into a permanent list.
        let state = unlocked();

        capture_item_via_router_impl(&state, "buy milk @groceries").unwrap();

        let guard = state.db.lock().unwrap();
        let all = destinations::list_all(guard.as_ref().unwrap().conn()).unwrap();
        assert!(
            !all.iter().any(|d| d.name == "groceries"),
            "the router must not create destinations on its own"
        );
    }

    #[test]
    fn a_chain_with_no_body_is_refused_rather_than_filed_as_an_empty_item() {
        // A routing gesture with nothing attached. `blurt-router` explicitly
        // leaves this call to the caller.
        let state = unlocked();
        destination(&state, "Weekly", "weekly");

        let error = capture_item_via_router_impl(&state, "@weekly").unwrap_err();

        assert_eq!(error, CommandError::EmptyCapture);
        assert_eq!(item_count(&state, destinations::UNSORTED_ID), 0);
    }

    #[test]
    fn a_locked_vault_refuses_to_capture() {
        let state = AppState::default();
        assert_eq!(
            capture_item_via_router_impl(&state, "buy milk").unwrap_err(),
            CommandError::Locked
        );
    }

    #[test]
    fn a_spoken_at_becomes_the_typed_grammar_so_capture_needs_no_voice_path() {
        // §4: "voice adds zero new parsing logic". Once normalized, the text
        // goes through `capture_item_via_router` like anything typed.
        let state = unlocked();
        let weekly = destination(&state, "Weekly", "weekly");

        let normalized = normalize_voice_transcript_impl("buy tomatoes at weekly");
        let outcome = capture_item_via_router_impl(&state, &normalized.text).unwrap();

        match &outcome {
            CaptureOutcomeDto::Resolved { item } => {
                assert_eq!(item.destination_id, weekly.to_string());
                assert_eq!(item.current_text, "buy tomatoes");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn an_everyday_at_is_flagged_but_reversible_rather_than_silently_routed() {
        // §4 accepts this false positive on purpose: "meet Alex at 9pm" gets
        // flagged, and the dismissal is what makes it cheap. This pins both
        // halves — that it *is* rewritten, and that the original survives so
        // the UI can put it back.
        let normalized = normalize_voice_transcript_impl("meet Dana at 9pm");

        assert!(normalized.text.contains('@'), "§4 rewrites every standalone at");
        assert_eq!(normalized.replacements.len(), 1);
        assert_eq!(normalized.replacements[0].original.trim(), "at");
    }

    #[test]
    fn normalizing_writes_nothing() {
        // The whole reason this is a separate command: review happens between
        // transcription and capture, so nothing may be filed yet.
        let state = unlocked();

        normalize_voice_transcript_impl("buy tomatoes at weekly");

        assert_eq!(item_count(&state, destinations::UNSORTED_ID), 0);
    }

    #[test]
    fn a_word_merely_containing_at_is_left_alone() {
        // "attempt" and "chat" must not be split; §4 keys off the standalone
        // word, not the letters.
        let normalized = normalize_voice_transcript_impl("attempt to chat later");

        assert!(!normalized.text.contains('@'), "got {}", normalized.text);
        assert!(normalized.replacements.is_empty());
    }

    #[test]
    fn the_outcome_serializes_as_a_tagged_union_the_frontend_can_switch_on() {
        let state = unlocked();
        destination(&state, "Weekly", "weekly");
        let outcome = capture_item_via_router_impl(&state, "buy tomatoes @weekly").unwrap();

        let json = serde_json::to_value(&outcome).unwrap();
        assert_eq!(json["kind"], "resolved");
        assert!(json["item"]["id"].is_string());
    }
}
