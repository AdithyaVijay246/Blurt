//! Statement-vs-question classification — `MODULE_04_EMBEDDINGS_RAG.md` §9.
//!
//! One input bar handles both capture and ask, so something has to decide which
//! the user meant — and it has to decide **without** the generative model. §9 is
//! explicit about why: invoking a multi-GB model merely to classify would
//! defeat Sleep-Mode's entire battery rationale (§2). So this is local string
//! inspection, no model, no parser, no allocation beyond one lowercased word.
//!
//! - A **statement** goes to Module 3's router — trigger/`@` resolution, NL
//!   matching, Unsorted fallback.
//! - A **question** skips the router entirely and runs Sleep-Mode retrieval and
//!   synthesis.
//!
//! ## Deliberately imperfect — do not "fix" this with a model
//!
//! §9 does not ask for a correct classifier, it asks for heuristics that
//! "reliably cover the overwhelming majority of real questions in English" and
//! leaves the remainder to the UI. Module 6's "did you mean to ask this?"
//! affordance is the correction path for a question captured as a statement.
//! The inverse — a rhetorical or thinking-out-loud question typed into a
//! capture tool — §9 explicitly declines to engineer around, calling it user
//! error against the app's actual purpose.
//!
//! So the known false positives below ("what a day", "how to reset the router")
//! are accepted, not defects. The imprecision is the design and the cost of
//! being wrong is one tap; a smarter classifier here would buy very little and
//! would reintroduce exactly the model dependency §9 rules out.
//!
//! ## Punctuation is not assumed
//!
//! Two of the three signals work without a `?` on purpose. Voice capture
//! (`MODULE_03_ROUTER.md` §4) produces no punctuation at all, so a classifier
//! that leant on `?` alone would send every spoken question to the router to be
//! filed as a note.

/// What the user meant by an input — §9's two branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    /// Capture it. Runs through Module 3's router.
    Statement,
    /// Answer it. Skips the router for Sleep-Mode retrieval and synthesis.
    Question,
}

/// Interrogative openings — §9's "starts with an interrogative word".
///
/// §9 names who/what/when/where/why/how; `whom`, `whose` and `which` complete
/// the same class and are listed for consistency rather than from a separate
/// decision.
const INTERROGATIVES: &[&str] = &[
    "who", "whom", "whose", "what", "when", "where", "why", "how", "which",
];

/// Auxiliary verbs that signal §9's "inverted auxiliary-verb-first structure".
///
/// English forms a yes/no question by fronting the auxiliary — "is the store
/// open", "did I buy milk" — so an auxiliary in first position is the whole
/// signal. §9 names does/is/are/can/did/should/could; the rest are the same
/// class in other tenses and moods.
const AUXILIARIES: &[&str] = &[
    "am", "is", "are", "was", "were", "be", "been", "do", "does", "did", "have", "has", "had",
    "can", "could", "shall", "should", "will", "would", "may", "might", "must",
];

/// Decides whether `input` is something to file or something to answer.
///
/// Three signals, any one of which is enough, in the order §9 lists them:
/// a trailing `?`, an interrogative opening word, or an auxiliary-first
/// inversion. Everything else is a statement — which is also where genuinely
/// ambiguous input lands, since a misfiled capture is recoverable and an
/// unanswered question is merely retyped.
pub fn classify(input: &str) -> InputKind {
    let trimmed = input.trim();

    // Content-free input has nothing to ask. A bare "?" reaching the router is
    // captured and can be deleted; routed to Sleep-Mode it would simply vanish
    // into an empty search.
    let Some(first) = first_word(trimmed) else {
        return InputKind::Statement;
    };

    if trimmed.ends_with('?') {
        return InputKind::Question;
    }

    if opens_a_question(&first) {
        return InputKind::Question;
    }

    InputKind::Statement
}

/// Whether `word` is an interrogative or a fronted auxiliary.
///
/// The negation-stripped form is tried as well, because a contraction cut at
/// its apostrophe does not always land on the auxiliary: "didn't" tokenizes as
/// `didn` + `t`, with the negation's `n` on the auxiliary's side. Stripping it
/// unconditionally would be wrong the other way — "can't" is already `can` +
/// `t`, and taking an `n` off that gives `ca`. So both forms are tested rather
/// than guessing which case applies. No word in either list is another list
/// word plus an `n`, so trying both cannot introduce a false positive.
fn opens_a_question(word: &str) -> bool {
    let listed = |candidate: &str| {
        INTERROGATIVES.contains(&candidate) || AUXILIARIES.contains(&candidate)
    };

    listed(word) || word.strip_suffix('n').is_some_and(listed)
}

/// The first word, lowercased and reduced to the part that carries the signal.
///
/// Two reductions. Edge punctuation is stripped so a quoted or bracketed
/// opening still matches. And a contraction is cut at its apostrophe, because
/// its head is what does the asking: "what's for dinner" asks exactly what
/// "what is for dinner" asks. That matters most for voice input, which supplies
/// no `?` to fall back on. Both the ASCII `'` and the typographic `’` that
/// phone keyboards insert are treated the same.
///
/// Note that the head is not always the bare auxiliary — "didn't" cuts to
/// `didn`, not `did` — so [`opens_a_question`] is what reconciles that, not
/// this function.
///
/// Returns `None` when nothing alphanumeric survives.
fn first_word(text: &str) -> Option<String> {
    let raw = text.split_whitespace().next()?.to_lowercase();
    let head = raw.split(['\'', '\u{2019}']).next().unwrap_or_default();
    let cleaned = head.trim_matches(|c: char| !c.is_alphanumeric());

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_question_mark_is_the_strongest_signal() {
        // "buy" opens an imperative, so only the punctuation makes this a
        // question — and it has to be enough on its own.
        assert_eq!(classify("buy milk?"), InputKind::Question);
        assert_eq!(classify("oranges?"), InputKind::Question);
    }

    #[test]
    fn an_interrogative_opening_is_a_question_without_punctuation() {
        assert_eq!(classify("what did I say about oranges"), InputKind::Question);
        assert_eq!(classify("where is the spare key"), InputKind::Question);
        assert_eq!(classify("why did the build fail"), InputKind::Question);
    }

    #[test]
    fn an_inverted_auxiliary_opening_is_a_question() {
        // The voice path's bread and butter: no punctuation, but the auxiliary
        // is fronted, which in English only happens in a question.
        assert_eq!(classify("did I buy milk"), InputKind::Question);
        assert_eq!(classify("is the store open on Sunday"), InputKind::Question);
        assert_eq!(classify("should I call Dana back"), InputKind::Question);
    }

    #[test]
    fn a_plain_capture_is_a_statement() {
        assert_eq!(classify("buy milk"), InputKind::Statement);
        assert_eq!(classify("meeting at 3pm with Dana"), InputKind::Statement);
        assert_eq!(classify("the build failed again"), InputKind::Statement);
    }

    #[test]
    fn classification_ignores_case_and_surrounding_punctuation() {
        assert_eq!(classify("  WHAT DID I SAY?  "), InputKind::Question);
        assert_eq!(classify("\"did I buy milk\""), InputKind::Question);
        assert_eq!(classify("   Is the store open"), InputKind::Question);
    }

    #[test]
    fn an_interrogative_word_later_in_the_sentence_does_not_make_a_question() {
        // §9's signal is the *opening* word. This is a note about a question,
        // not a question.
        assert_eq!(
            classify("remember what I said about oranges"),
            InputKind::Statement
        );
        assert_eq!(classify("ask Dana why the build failed"), InputKind::Statement);
    }

    #[test]
    fn matching_is_whole_word_not_prefix() {
        // The same trap `blurt-router` hit with "at" inside "attempt": a
        // prefix match would classify half the dictionary as a question.
        assert_eq!(classify("isolate the failing test"), InputKind::Statement);
        assert_eq!(classify("candid photo of the dog"), InputKind::Statement);
        assert_eq!(classify("willow branch for the vase"), InputKind::Statement);
        assert_eq!(classify("doable before Friday"), InputKind::Statement);
    }

    #[test]
    fn contractions_are_recognized_since_voice_capture_has_no_punctuation() {
        assert_eq!(classify("what's for dinner"), InputKind::Question);
        assert_eq!(classify("didn't I buy milk already"), InputKind::Question);
        assert_eq!(classify("where's the spare key"), InputKind::Question);
        // The typographic apostrophe a phone keyboard inserts must behave
        // identically to the ASCII one.
        assert_eq!(classify("what\u{2019}s for dinner"), InputKind::Question);
    }

    /// Cutting at the apostrophe is not enough on its own: "didn't" leaves
    /// `didn`, with the negation's `n` on the auxiliary's side, while "can't"
    /// leaves `can` already intact. Both shapes have to work, and an earlier
    /// draft handled only the second.
    #[test]
    fn negative_contractions_reduce_to_their_auxiliary() {
        assert_eq!(classify("didn't I buy milk already"), InputKind::Question);
        assert_eq!(classify("isn't the store open today"), InputKind::Question);
        assert_eq!(classify("doesn't Dana have the key"), InputKind::Question);
        assert_eq!(classify("couldn't we just walk"), InputKind::Question);
        assert_eq!(classify("can't I bring the dog"), InputKind::Question);
    }

    #[test]
    fn stripping_the_negation_does_not_invent_questions() {
        // The `n`-stripping in `opens_a_question` is tried on every opening
        // word, so a plain word ending in `n` must not become a question.
        assert_eq!(classify("man the grill at six"), InputKind::Statement);
        assert_eq!(classify("in the morning before work"), InputKind::Statement);
        assert_eq!(classify("van needs new tyres"), InputKind::Statement);
    }

    #[test]
    fn content_free_input_is_a_statement() {
        // Nothing to ask. Captured, it is one tap to delete; sent to
        // Sleep-Mode it would disappear into an empty search.
        assert_eq!(classify(""), InputKind::Statement);
        assert_eq!(classify("   "), InputKind::Statement);
        assert_eq!(classify("?"), InputKind::Statement);
    }

    /// Not aspirational — these assert what the classifier *actually does*, so
    /// the accepted cost stays visible. §9 rules the correction out of scope:
    /// a rhetorical question typed into a capture tool is user error, and
    /// Module 6's "did you mean to ask this?" chip covers the other direction.
    ///
    /// If real usage shows the interrogative-opening rule is too eager, the
    /// tuning knob is to require an inversion after the wh-word ("what *did*
    /// I") rather than to accept a bare one. That is a behaviour change, so it
    /// belongs to real-usage data, not to a guess made here.
    #[test]
    fn known_false_positives_are_accepted_by_design() {
        assert_eq!(classify("what a day"), InputKind::Question);
        assert_eq!(classify("how to reset the router"), InputKind::Question);
        assert_eq!(classify("can of paint for the shed"), InputKind::Question);
    }

    #[test]
    fn a_trailing_question_mark_wins_over_a_statement_opening() {
        // Mixed input: the `?` is the last thing typed, so it decides. Worth
        // pinning because the alternative — splitting sentences and voting —
        // is exactly the over-engineering §9 warns off.
        assert_eq!(classify("buy milk. did I get eggs?"), InputKind::Question);
    }
}
