//! Natural-language routing fallback — `MODULE_03_ROUTER.md` §3.
//!
//! For freeform input with no `@` at all. Deliberately *lightweight local
//! keyword matching, not an LLM call* (§3) — routine capture must not wake a
//! model, or the sleep-mode battery story collapses.
//!
//! ## Scoring
//!
//! `MODULE_03_ROUTER.md` specifies the confident-match/no-match branch but no
//! formula, and does not list one under its own "Explicitly Deferred" section
//! — so this is an implementation detail settled here rather than a design
//! question left open:
//!
//! ```text
//! score = 1.0 × (trigger appears as a whole word)
//!       + 0.6 × (name appears as a whole word)
//!       + 0.4 × (fraction of the name's words present anywhere in the text)
//! ```
//!
//! with a confident match above [`CONFIDENCE_THRESHOLD`].
//!
//! One consequence is worth stating outright, because it is not obvious from
//! the numbers: the fraction term alone tops out at `0.4`, below the
//! threshold. So a confident match *always* requires a whole-word hit on a
//! trigger or a name, and the fraction only ever separates candidates that
//! both already hit. That conservatism is the right side to err on — an
//! unconfident blurt lands in Unsorted with a badge (§3), which the user
//! resolves later, whereas a wrong confident guess quietly files it somewhere
//! they will not think to look.

use uuid::Uuid;

use blurt_schema::repository::destinations::{Destination, RANDOM_THOUGHTS_ID};

/// A blurt routes automatically only above this score. See the module docs for
/// what that threshold means in practice.
pub const CONFIDENCE_THRESHOLD: f32 = 0.6;

/// A confident natural-language match.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NlMatch {
    pub destination_id: Uuid,
    pub confidence: f32,
}

/// Best confident destination for `text`, or `None` if nothing clears
/// [`CONFIDENCE_THRESHOLD`] — in which case §3 sends the capture to Unsorted.
///
/// Two kinds of candidate are dropped before scoring, both per §3: `isSystem`
/// destinations (Unsorted is not a routing target, it is the absence of one),
/// and **Random Thoughts**, which is "never auto-suggested by NL matching" —
/// it answers "does this belong anywhere at all?", a question only the user
/// can answer. Random Thoughts is recognised by its well-known seed id rather
/// than its name, since the user may rename it freely.
///
/// Ties resolve to the earlier candidate, so callers passing a stably ordered
/// list (as [`crate::candidates`] does) get a stable answer.
pub fn best_match(text: &str, candidates: &[Destination]) -> Option<NlMatch> {
    let haystack = words(text);
    let mut best: Option<(&Destination, f32)> = None;

    for destination in candidates
        .iter()
        .filter(|d| !d.is_system && d.id != RANDOM_THOUGHTS_ID)
    {
        let score = score(&haystack, destination);
        if score <= CONFIDENCE_THRESHOLD {
            continue;
        }
        // Strictly greater, so a tie keeps the earlier candidate.
        if best.is_none_or(|(_, incumbent)| score > incumbent) {
            best = Some((destination, score));
        }
    }

    best.map(|(destination, confidence)| NlMatch {
        destination_id: destination.id,
        confidence,
    })
}

fn score(haystack: &[String], destination: &Destination) -> f32 {
    let trigger = words(&destination.trigger);
    let name = words(&destination.name);

    let trigger_hit = contains_sequence(haystack, &trigger);
    let name_hit = contains_sequence(haystack, &name);
    let present = name.iter().filter(|word| haystack.contains(*word)).count();
    let fraction = if name.is_empty() {
        0.0
    } else {
        present as f32 / name.len() as f32
    };

    (if trigger_hit { 1.0 } else { 0.0 }) + (if name_hit { 0.6 } else { 0.0 }) + 0.4 * fraction
}

/// Lowercased alphanumeric words. Splitting on non-alphanumerics is what makes
/// the match whole-word: "shop" cannot match inside "shopping", and trailing
/// punctuation ("shopping.") does not hide a word.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `needle`'s words appear contiguously and in order within
/// `haystack` — so a two-word name matches "the grocery run list" but not
/// "grocery shopping on the run".
fn contains_sequence(haystack: &[String], needle: &[String]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use blurt_schema::repository::destinations::DestinationKind;

    fn dest(name: &str, trigger: &str) -> Destination {
        Destination {
            id: Uuid::new_v4(),
            parent_id: None,
            name: name.to_string(),
            trigger: trigger.to_string(),
            kind: DestinationKind::List,
            is_system: false,
            is_sensitive: false,
            sort_order: 0,
            created_at: 0,
            deleted_at: None,
        }
    }

    #[test]
    fn matches_a_destination_whose_name_appears_as_a_whole_word() {
        let shopping = dest("Shopping", "shop");
        let found = best_match("add milk to shopping", std::slice::from_ref(&shopping)).expect("confident");
        assert_eq!(found.destination_id, shopping.id);
        assert!(found.confidence > CONFIDENCE_THRESHOLD);
    }

    #[test]
    fn matches_a_destination_by_its_trigger() {
        let shopping = dest("Groceries", "shop");
        let found = best_match("tomatoes for the shop run", std::slice::from_ref(&shopping)).expect("confident");
        assert_eq!(found.destination_id, shopping.id);
    }

    #[test]
    fn is_case_insensitive() {
        let shopping = dest("Shopping", "shop");
        assert!(best_match("Add Milk To SHOPPING", &[shopping]).is_some());
    }

    #[test]
    fn returns_none_when_nothing_matches() {
        let shopping = dest("Shopping", "shop");
        assert!(
            best_match("the sky was purple today", &[shopping]).is_none(),
            "§3 — no match saves to Unsorted rather than guessing"
        );
    }

    #[test]
    fn a_prefix_of_a_word_is_not_a_whole_word_match() {
        let shopping = dest("Errands", "shop");
        assert!(
            best_match("i went shopping", &[shopping]).is_none(),
            "'shop' inside 'shopping' must not fire the trigger"
        );
    }

    #[test]
    fn a_partial_multi_word_name_alone_is_not_confident() {
        let run = dest("Weekly Grocery Run", "wgr");
        assert!(
            best_match("grocery run tomorrow", &[run]).is_none(),
            "the fraction term tops out below the threshold by design"
        );
    }

    #[test]
    fn never_matches_random_thoughts_even_when_named_outright() {
        let mut random = dest("Random Thoughts", "random");
        random.id = RANDOM_THOUGHTS_ID;

        assert!(
            best_match("a random thoughts kind of day", &[random]).is_none(),
            "§3 — Random Thoughts is never auto-suggested by NL matching"
        );
    }

    #[test]
    fn a_renamed_random_thoughts_is_still_excluded() {
        let mut random = dest("Musings", "musings");
        random.id = RANDOM_THOUGHTS_ID;

        assert!(
            best_match("some musings today", &[random]).is_none(),
            "exclusion keys off the seed id, which survives a rename"
        );
    }

    #[test]
    fn never_matches_a_system_destination() {
        let mut unsorted = dest("Unsorted", "unsorted");
        unsorted.is_system = true;

        assert!(best_match("put this in unsorted", &[unsorted]).is_none());
    }

    #[test]
    fn picks_the_highest_scoring_candidate() {
        let weak = dest("Shopping", "shop");
        let strong = dest("Groceries", "shopping");

        let found = best_match("shopping groceries", &[weak.clone(), strong.clone()]).expect("confident");
        assert_eq!(
            found.destination_id, strong.id,
            "trigger hit plus name hit outscores a name hit alone"
        );
    }

    #[test]
    fn a_multi_word_name_matches_when_its_words_appear_in_order() {
        let run = dest("Grocery Run", "gr");
        assert!(best_match("add this to the grocery run list", &[run]).is_some());
    }

    #[test]
    fn an_empty_candidate_list_matches_nothing() {
        assert!(best_match("anything at all", &[]).is_none());
    }
}
