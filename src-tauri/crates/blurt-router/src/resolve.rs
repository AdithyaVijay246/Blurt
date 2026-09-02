//! Routing decisions — `MODULE_03_ROUTER.md` §2–3.
//!
//! [`route`] is the crate's entry point: raw capture text in, a decision out.
//! It performs **no writes**. Creating the destination, capturing the item and
//! emitting the "new destination" event all stay in `blurt-app`, which is the
//! crate Module 1 designates as the seam between Tauri and the domain. That
//! split is also what lets every test in this file run against a
//! `destinations` table with no `items` row in sight.
//!
//! The branches of §3 map onto the four decisions below:
//!
//! * an `@` chain that fully resolves → [`RoutingDecision::Resolved`],
//!   deterministic, no confirmation (§3 "Explicit");
//! * an `@` chain whose next segment names nothing → [`RoutingDecision::Create`],
//!   §2.5's `+` option, nested wherever in the chain the name was typed;
//! * freeform text scoring above the threshold → [`RoutingDecision::NlMatched`],
//!   saved instantly with a dismissible quick-confirm chip;
//! * anything else → [`RoutingDecision::Unrouted`], which means Unsorted and a
//!   badge, never a blocking prompt.

use rusqlite::Connection;
use uuid::Uuid;

use blurt_schema::repository::destinations;

use crate::candidates;
use crate::chain::{self, ParsedCapture};
use crate::error::Result;
use crate::nl;

/// A routing decision together with the text it was made about.
///
/// `text` is the chain body, or the whole input when there was no chain. It is
/// reported verbatim and may be empty — a user who types nothing but
/// `@weekly` has expressed a destination and no content, and it is `blurt-app`,
/// not the router, that decides an empty capture is not worth writing.
#[derive(Debug, Clone, PartialEq)]
pub struct Routing {
    pub text: String,
    pub decision: RoutingDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RoutingDecision {
    /// Every segment resolved. File it immediately, no confirmation — the user
    /// drove the selection through the picker themselves (§3).
    Resolved { destination_id: Uuid },

    /// The next segment names no destination at this depth: §2.5's `+` create.
    ///
    /// `parent_id` is whatever resolved before it, which is the whole of how
    /// `@shopping@grocery` means "create grocery under shopping" without a
    /// separate parent-selection step — nesting falls out of *where in the
    /// chain* the name was typed.
    ///
    /// Only the first unresolved segment is described, per §4.3: fixing it
    /// re-validates everything downstream. The rest are handed back in
    /// `remaining_segments` rather than dropped, so the caller can re-route
    /// once this one exists.
    ///
    /// No `kind` is offered: list-or-note is the caller's call, not routing's.
    Create {
        parent_id: Option<Uuid>,
        name: String,
        trigger: String,
        remaining_segments: Vec<String>,
    },

    /// Natural-language fallback found a confident destination (§3). The save
    /// happens regardless; the quick-confirm chip is dismissible, not a prompt.
    NlMatched { destination_id: Uuid, confidence: f32 },

    /// Nothing matched. Goes to the reserved Unsorted destination and bumps its
    /// badge (§3) — "nothing is ever unrouted" is satisfied by Unsorted being a
    /// real destination, not by guessing.
    Unrouted,
}

/// Resolves raw capture text to a routing decision.
pub fn route(conn: &Connection, raw_input: &str) -> Result<Routing> {
    match chain::parse(raw_input) {
        ParsedCapture::PlainText(text) => {
            // Every live destination at any depth: freeform text carries no
            // hierarchy to scope the search by.
            let candidates = destinations::list_all(conn)?;
            let decision = match nl::best_match(&text, &candidates) {
                Some(matched) => RoutingDecision::NlMatched {
                    destination_id: matched.destination_id,
                    confidence: matched.confidence,
                },
                None => RoutingDecision::Unrouted,
            };
            Ok(Routing { text, decision })
        }

        ParsedCapture::Chain { body, segments } => {
            let mut parent_id = None;

            for (index, segment) in segments.iter().enumerate() {
                match candidates::exact_match(conn, parent_id, segment)? {
                    // Resolved: re-scope one level down and keep walking (§2.6).
                    Some(found) => parent_id = Some(found.id),
                    None => {
                        return Ok(Routing {
                            text: body,
                            decision: RoutingDecision::Create {
                                parent_id,
                                name: segment.clone(),
                                trigger: slugify_trigger(conn, segment)?,
                                remaining_segments: segments[index + 1..].to_vec(),
                            },
                        })
                    }
                }
            }

            let destination_id = parent_id.expect("chain::parse never yields zero segments");
            Ok(Routing {
                text: body,
                decision: RoutingDecision::Resolved { destination_id },
            })
        }
    }
}

/// Derives a free trigger for a router-created destination.
///
/// Lowercased and whitespace-stripped, then given a numeric suffix until it no
/// longer collides with a live trigger (`shop`, `shop2`, ...). The check spans
/// every live destination at any depth, not just the target one, because
/// `idx_destinations_trigger_live` makes triggers unique database-wide — and
/// it spans hidden system rows too, so `@unsorted` cannot quietly steal the
/// seeded Unsorted trigger.
///
/// `name` is expected to be non-empty — chain segments always are, by
/// construction in [`crate::chain`].
pub fn slugify_trigger(conn: &Connection, name: &str) -> Result<String> {
    let base: String = name
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();

    let taken: Vec<String> = destinations::list_all(conn)?
        .into_iter()
        .map(|d| d.trigger.to_lowercase())
        .collect();

    if !taken.contains(&base) {
        return Ok(base);
    }
    for suffix in 2u32.. {
        let candidate = format!("{base}{suffix}");
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
    }
    unreachable!("the suffix search is unbounded, so some candidate is always free")
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations::{Destination, DestinationKind};
    use blurt_schema::{Database, MasterKey};

    fn migrated() -> Database {
        let db = Database::open_in_memory(&MasterKey::generate()).unwrap();
        blurt_schema::migrations::run(db.conn()).unwrap();
        db
    }

    fn make(db: &Database, name: &str, trigger: &str, parent: Option<Uuid>) -> Destination {
        destinations::create(db.conn(), name, trigger, DestinationKind::List, parent, false, false, 0).unwrap()
    }

    #[test]
    fn a_resolved_chain_files_directly() {
        let db = migrated();
        let weekly = make(&db, "Weekly", "weekly", None);

        let routed = route(db.conn(), "buy tomatoes @weekly").unwrap();
        assert_eq!(routed.text, "buy tomatoes");
        assert_eq!(routed.decision, RoutingDecision::Resolved { destination_id: weekly.id });
    }

    #[test]
    fn a_multi_segment_chain_files_at_its_deepest_segment() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        let grocery = make(&db, "Grocery", "grocery", Some(shopping.id));

        let routed = route(db.conn(), "tomatoes @shop@grocery").unwrap();
        assert_eq!(routed.decision, RoutingDecision::Resolved { destination_id: grocery.id });
    }

    #[test]
    fn a_chain_segment_resolves_case_insensitively() {
        let db = migrated();
        let weekly = make(&db, "Weekly", "weekly", None);

        let routed = route(db.conn(), "tomatoes @WEEKLY").unwrap();
        assert_eq!(routed.decision, RoutingDecision::Resolved { destination_id: weekly.id });
    }

    #[test]
    fn an_empty_body_still_routes_and_is_reported_as_empty() {
        let db = migrated();
        let weekly = make(&db, "Weekly", "weekly", None);

        let routed = route(db.conn(), "@weekly").unwrap();
        assert_eq!(routed.text, "");
        assert_eq!(routed.decision, RoutingDecision::Resolved { destination_id: weekly.id });
    }

    #[test]
    fn an_unknown_first_segment_becomes_a_top_level_create() {
        let db = migrated();

        let routed = route(db.conn(), "tomatoes @produce").unwrap();
        assert_eq!(routed.text, "tomatoes");
        assert_eq!(
            routed.decision,
            RoutingDecision::Create {
                parent_id: None,
                name: "produce".to_string(),
                trigger: "produce".to_string(),
                remaining_segments: vec![],
            }
        );
    }

    #[test]
    fn an_unknown_later_segment_creates_under_the_resolved_parent() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);

        let routed = route(db.conn(), "tomatoes @shop@grocery").unwrap();
        assert_eq!(
            routed.decision,
            RoutingDecision::Create {
                parent_id: Some(shopping.id),
                name: "grocery".to_string(),
                trigger: "grocery".to_string(),
                remaining_segments: vec![],
            },
            "§2.5 — nesting falls out of where in the chain the name was typed"
        );
    }

    #[test]
    fn segments_after_the_first_unknown_one_are_carried_not_dropped() {
        let db = migrated();

        let routed = route(db.conn(), "tomatoes @produce@leafy").unwrap();
        match routed.decision {
            RoutingDecision::Create { name, remaining_segments, .. } => {
                assert_eq!(name, "produce");
                assert_eq!(remaining_segments, vec!["leafy".to_string()]);
            }
            other => panic!("expected a Create decision, got {other:?}"),
        }
    }

    #[test]
    fn a_create_trigger_avoids_colliding_with_a_live_trigger_anywhere() {
        let db = migrated();
        // "shop" is Errands' trigger at the top level; the new destination is
        // being created one level down, but triggers are unique database-wide.
        let errands = make(&db, "Errands", "shop", None);

        let routed = route(db.conn(), "tomatoes @shop@shop").unwrap();
        assert_eq!(
            routed.decision,
            RoutingDecision::Create {
                parent_id: Some(errands.id),
                name: "shop".to_string(),
                trigger: "shop2".to_string(),
                remaining_segments: vec![],
            }
        );
    }

    #[test]
    fn the_hidden_unsorted_destination_cannot_be_typed_into() {
        let db = migrated();

        let routed = route(db.conn(), "tomatoes @unsorted").unwrap();
        assert_eq!(
            routed.decision,
            RoutingDecision::Create {
                parent_id: None,
                name: "unsorted".to_string(),
                trigger: "unsorted2".to_string(),
                remaining_segments: vec![],
            },
            "@unsorted resolves to nothing, and the seeded trigger is still taken"
        );
    }

    #[test]
    fn plain_text_with_a_confident_match_is_nl_matched() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);

        let routed = route(db.conn(), "add milk to shopping").unwrap();
        assert_eq!(routed.text, "add milk to shopping");
        match routed.decision {
            RoutingDecision::NlMatched { destination_id, confidence } => {
                assert_eq!(destination_id, shopping.id);
                assert!(confidence > nl::CONFIDENCE_THRESHOLD);
            }
            other => panic!("expected an NlMatched decision, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_with_no_match_is_unrouted() {
        let db = migrated();
        make(&db, "Shopping", "shop", None);

        let routed = route(db.conn(), "the sky was purple today").unwrap();
        assert_eq!(routed.text, "the sky was purple today");
        assert_eq!(routed.decision, RoutingDecision::Unrouted);
    }

    #[test]
    fn nl_matching_never_lands_in_random_thoughts() {
        let db = migrated();

        let routed = route(db.conn(), "just a random thoughts kind of day").unwrap();
        assert_eq!(
            routed.decision,
            RoutingDecision::Unrouted,
            "§3 — Random Thoughts is reachable by @ only, never auto-suggested"
        );
    }

    #[test]
    fn nl_matching_searches_every_depth_not_just_the_top_level() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        let grocery = make(&db, "Grocery", "grocery", Some(shopping.id));

        let routed = route(db.conn(), "tomatoes for the grocery list").unwrap();
        match routed.decision {
            RoutingDecision::NlMatched { destination_id, .. } => assert_eq!(
                destination_id, grocery.id,
                "freeform text carries no depth to scope by"
            ),
            other => panic!("expected an NlMatched decision, got {other:?}"),
        }
    }

    #[test]
    fn a_voice_transcript_routes_through_the_same_path_as_typed_input() {
        let db = migrated();
        let shopping = make(&db, "Shopping", "shop", None);
        let grocery = make(&db, "Grocery", "grocery", Some(shopping.id));

        let spoken = crate::voice::normalize_spoken_at("buy tomatoes at shop at grocery");
        let routed = route(db.conn(), &spoken.text).unwrap();

        assert_eq!(routed.text, "buy tomatoes");
        assert_eq!(routed.decision, RoutingDecision::Resolved { destination_id: grocery.id });
    }

    #[test]
    fn slugify_trigger_lowercases_and_strips_whitespace() {
        let db = migrated();
        assert_eq!(slugify_trigger(db.conn(), "Grocery Run").unwrap(), "groceryrun");
    }

    #[test]
    fn slugify_trigger_counts_up_past_every_taken_variant() {
        let db = migrated();
        make(&db, "One", "shop", None);
        make(&db, "Two", "shop2", None);

        assert_eq!(slugify_trigger(db.conn(), "shop").unwrap(), "shop3");
    }

    #[test]
    fn slugify_trigger_reuses_a_tombstoned_trigger() {
        let db = migrated();
        let gone = make(&db, "One", "shop", None);
        destinations::tombstone(db.conn(), gone.id).unwrap();

        assert_eq!(
            slugify_trigger(db.conn(), "shop").unwrap(),
            "shop",
            "uniqueness is over live rows only"
        );
    }
}
