//! Voice transcript normalization — `MODULE_03_ROUTER.md` §4.
//!
//! Voice adds **zero new parsing logic**. A transcript is rewritten into
//! `@`-syntax text by this one pre-pass, after which it is indistinguishable
//! from typed input and goes through [`crate::chain`] unchanged.
//!
//! Speech-to-text writes spoken "at" as the word `at`, never the symbol, so
//! segment splitting keys off the word (§4.1). That makes "at" effectively
//! reserved in voice-parsed blurts: "meet Alex at 9pm" gets rewritten too.
//! §4.1 accepts this outright — a one-time per-sentence dismissal is judged
//! cheaper than silently mis-routing — and §4.2 is what pays for it, so every
//! rewrite is reported in [`NormalizedVoice::replacements`] with the exact
//! text it replaced, letting the UI highlight each one and restore it verbatim
//! when dismissed.
//!
//! Two details are forced by the typed grammar rather than stated in §4:
//!
//! * The whitespace *after* "at" is consumed, not just the word. Rewriting
//!   "at weekly" to "@ weekly" would produce the one construction §2.2 defines
//!   as inert, so voice routing would never fire at all.
//! * When a single word separates two spoken "at"s, the gap before the second
//!   `@` is closed as well, so "at shopping at grocery" becomes the one
//!   two-segment chain `@shopping@grocery` that §4.4 describes — rather than
//!   two unrelated chains of which only the last would survive.

/// One spoken "at" that was rewritten to `@`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenAt {
    /// Byte offset of the inserted `@` within [`NormalizedVoice::text`].
    pub offset: usize,
    /// Exactly what was replaced, whitespace included, so dismissing a false
    /// positive (§4.2) restores the transcript verbatim — capitalisation and
    /// spacing intact.
    pub original: String,
}

/// A transcript rewritten into `@` syntax, plus the rewrites made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedVoice {
    pub text: String,
    pub replacements: Vec<SpokenAt>,
}

/// Rewrites every standalone spoken "at" in `transcript` to `@` (§4.1).
pub fn normalize_spoken_at(transcript: &str) -> NormalizedVoice {
    let mut text = String::with_capacity(transcript.len());
    let mut replacements = Vec::new();
    let mut cursor = 0;
    let mut chain_open = false;

    for start in standalone_at_positions(transcript) {
        if start < cursor {
            continue;
        }
        let between = &transcript[cursor..start];

        // A single word since the last `@` means this "at" continues that
        // chain rather than starting a new one, so the gap closes up (§4.4).
        let trimmed = between.trim_end();
        let continues_chain =
            chain_open && !trimmed.is_empty() && !trimmed.contains(char::is_whitespace);
        text.push_str(if continues_chain { trimmed } else { between });

        let offset = text.len();
        text.push('@');

        // Swallow the whitespace after "at" as well — "@ weekly" would be
        // inert under §2.2 and could never route.
        let mut end = start + 2;
        while let Some(c) = transcript[end..].chars().next() {
            if !c.is_whitespace() {
                break;
            }
            end += c.len_utf8();
        }

        replacements.push(SpokenAt {
            offset,
            original: transcript[start..end].to_string(),
        });
        cursor = end;
        chain_open = true;
    }

    text.push_str(&transcript[cursor..]);
    NormalizedVoice { text, replacements }
}

/// Byte offsets of every standalone word "at", case-insensitive.
///
/// Standalone means alphanumeric-bounded on both sides, so "attempt", "chat"
/// and "that" are untouched — the same word rule [`crate::nl`] scores by.
fn standalone_at_positions(transcript: &str) -> Vec<usize> {
    let bytes = transcript.as_bytes();
    let mut found = Vec::new();

    for index in 0..bytes.len().saturating_sub(1) {
        if !bytes[index].eq_ignore_ascii_case(&b'a') || !bytes[index + 1].eq_ignore_ascii_case(&b't')
        {
            continue;
        }
        // Both bytes are ASCII, so index + 2 is necessarily a char boundary;
        // only the start needs checking, in case "at" sits mid-codepoint.
        if !transcript.is_char_boundary(index) {
            continue;
        }
        let preceded_by_word = transcript[..index]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric);
        let followed_by_word = transcript[index + 2..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
        if !preceded_by_word && !followed_by_word {
            found.push(index);
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{parse, ParsedCapture};

    #[test]
    fn a_transcript_with_no_at_is_returned_unchanged() {
        let out = normalize_spoken_at("buy tomatoes this week");
        assert_eq!(out.text, "buy tomatoes this week");
        assert!(out.replacements.is_empty());
    }

    #[test]
    fn a_standalone_at_becomes_an_at_symbol_closed_up_to_the_next_word() {
        let out = normalize_spoken_at("buy tomatoes at weekly");
        assert_eq!(out.text, "buy tomatoes @weekly");
        assert_eq!(out.replacements.len(), 1);
    }

    #[test]
    fn at_inside_another_word_is_left_alone() {
        let out = normalize_spoken_at("attempt to chat with Nat about that");
        assert_eq!(out.text, "attempt to chat with Nat about that");
        assert!(out.replacements.is_empty(), "only a standalone word 'at' is a separator");
    }

    #[test]
    fn is_case_insensitive() {
        let out = normalize_spoken_at("Buy milk At shopping");
        assert_eq!(out.text, "Buy milk @shopping");
    }

    #[test]
    fn records_the_replaced_text_so_a_false_positive_can_be_dismissed() {
        let out = normalize_spoken_at("meet Alex at 9pm");
        assert_eq!(out.text, "meet Alex @9pm");
        assert_eq!(
            out.replacements,
            vec![SpokenAt { offset: 10, original: "at ".to_string() }],
            "§4.1's accepted false positive, made reversible by §4.2"
        );
        assert_eq!(&out.text[10..11], "@", "offset points at the inserted symbol");
    }

    #[test]
    fn consecutive_ats_build_one_multi_segment_chain() {
        let out = normalize_spoken_at("buy milk at shopping at grocery");
        assert_eq!(
            out.text, "buy milk @shopping@grocery",
            "§4.4 — an explicit 'at' between each segment, one chain"
        );
        assert_eq!(out.replacements.len(), 2);
    }

    #[test]
    fn multi_word_text_between_two_ats_does_not_continue_the_chain() {
        let out = normalize_spoken_at("buy at shopping some words at grocery");
        assert_eq!(out.text, "buy @shopping some words @grocery");
    }

    #[test]
    fn a_trailing_at_becomes_a_bare_symbol() {
        let out = normalize_spoken_at("remind me at");
        assert_eq!(out.text, "remind me @");
        assert_eq!(out.replacements, vec![SpokenAt { offset: 10, original: "at".to_string() }]);
    }

    #[test]
    fn normalized_output_feeds_the_typed_parser_with_no_extra_logic() {
        // §4's whole claim: after this pass, voice is indistinguishable from
        // typed input.
        let out = normalize_spoken_at("buy tomatoes at shopping at grocery");
        assert_eq!(
            parse(&out.text),
            ParsedCapture::Chain {
                body: "buy tomatoes".to_string(),
                segments: vec!["shopping".to_string(), "grocery".to_string()],
            }
        );
    }

    #[test]
    fn a_dismissed_replacement_can_be_restored_verbatim() {
        let out = normalize_spoken_at("meet Alex at 9pm");
        let replacement = &out.replacements[0];

        let mut restored = out.text.clone();
        restored.replace_range(replacement.offset..replacement.offset + 1, &replacement.original);
        assert_eq!(restored, "meet Alex at 9pm");
    }
}
