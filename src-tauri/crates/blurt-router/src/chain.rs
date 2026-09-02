//! `@`-chain parsing — `MODULE_03_ROUTER.md` §2.
//!
//! `@` is **trailing, not leading**: the user says what they're blurting and
//! tags a destination at the end (`buy tomatoes @weekly`), so a chain is only
//! ever a suffix of the input. `@weekly buy tomatoes` is ordinary text.
//!
//! Two rules from §2 shape everything here, plus one this crate had to settle
//! because §2 describes the live picker rather than a parser:
//!
//! * §2.2 — `@` followed by a space is inert ("meet @ 9pm").
//! * §2.6 — a chain re-scopes at each segment, so `@shopping@grocery` is two
//!   segments, not one name containing an `@`.
//! * **Undocumented, decided here:** the `@` that *opens* a chain must sit on
//!   a word boundary — preceded by whitespace, or at the very start of the
//!   input. Without this, `email bob@example.com` would parse
//!   `example.com` as a destination segment. `@`s *inside* an already-open
//!   chain need no such boundary, which is what keeps §2.6 working.

/// The result of parsing raw capture text for `@` syntax.
///
/// Parsing is purely structural: it reports what the syntax says, not whether
/// any segment names a real destination. Resolving segments against the
/// `destinations` table is [`crate::resolve`]'s job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedCapture {
    /// No trailing chain. The whole input is the item's text.
    PlainText(String),
    /// A trailing chain. `body` is the text to capture (possibly empty, if the
    /// user typed nothing but a chain); `segments` are the raw, un-resolved
    /// names in the order typed, outermost first.
    Chain { body: String, segments: Vec<String> },
}

/// Splits raw capture text into its body and trailing `@` chain, if any.
pub fn parse(input: &str) -> ParsedCapture {
    let trimmed = input.trim_end();

    // Left to right, so a multi-segment chain is opened by its *first* `@`
    // rather than being clipped to its last segment.
    for (index, _) in trimmed.match_indices('@') {
        let is_chain_opener = index == 0 || trimmed[..index].ends_with(char::is_whitespace);
        if !is_chain_opener {
            continue;
        }

        // The slice starts with `@`, so `split` always leads with an empty
        // piece; every piece after it is a segment.
        let mut pieces = trimmed[index..].split('@');
        pieces.next();
        let segments: Vec<&str> = pieces.collect();

        // An empty piece means a bare or doubled `@`; whitespace inside one
        // means the user typed past the picker (§2.2, §2.5). Either way this
        // `@` did not open a chain — a later one still might.
        if segments.iter().any(|s| s.is_empty() || s.contains(char::is_whitespace)) {
            continue;
        }

        return ParsedCapture::Chain {
            body: trimmed[..index].trim_end().to_string(),
            segments: segments.into_iter().map(str::to_string).collect(),
        };
    }

    ParsedCapture::PlainText(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain(body: &str, segments: &[&str]) -> ParsedCapture {
        ParsedCapture::Chain {
            body: body.to_string(),
            segments: segments.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn text_with_no_at_is_plain() {
        assert_eq!(
            parse("buy tomatoes this week"),
            ParsedCapture::PlainText("buy tomatoes this week".to_string())
        );
    }

    #[test]
    fn empty_input_is_plain() {
        assert_eq!(parse(""), ParsedCapture::PlainText(String::new()));
    }

    #[test]
    fn a_trailing_at_segment_opens_a_chain() {
        assert_eq!(parse("buy tomatoes @weekly"), chain("buy tomatoes", &["weekly"]));
    }

    #[test]
    fn a_multi_segment_chain_splits_on_each_at() {
        assert_eq!(
            parse("buy tomatoes @shopping@grocery"),
            chain("buy tomatoes", &["shopping", "grocery"]),
            "§2.6 — each segment re-scopes the picker one level deeper"
        );
    }

    #[test]
    fn at_followed_by_a_space_is_inert() {
        assert_eq!(
            parse("meet Alex @ 9pm"),
            ParsedCapture::PlainText("meet Alex @ 9pm".to_string()),
            "§2.2 — the picker never engages on '@ '"
        );
    }

    #[test]
    fn a_leading_at_stays_plain_text_because_chains_are_trailing_only() {
        assert_eq!(
            parse("@weekly buy tomatoes"),
            ParsedCapture::PlainText("@weekly buy tomatoes".to_string())
        );
    }

    #[test]
    fn an_at_not_on_a_word_boundary_stays_plain_text() {
        assert_eq!(
            parse("email bob@example.com"),
            ParsedCapture::PlainText("email bob@example.com".to_string()),
            "an address is not a destination chain"
        );
    }

    #[test]
    fn a_bare_trailing_at_is_plain_text() {
        assert_eq!(
            parse("buy tomatoes @"),
            ParsedCapture::PlainText("buy tomatoes @".to_string()),
            "nothing follows the '@', so no segment has been typed yet"
        );
    }

    #[test]
    fn a_doubled_at_is_plain_text() {
        assert_eq!(
            parse("buy tomatoes @@weekly"),
            ParsedCapture::PlainText("buy tomatoes @@weekly".to_string()),
            "an empty segment is not a name"
        );
    }

    #[test]
    fn a_chain_with_no_body_still_parses_as_a_chain() {
        assert_eq!(parse("@weekly"), chain("", &["weekly"]));
    }

    #[test]
    fn trailing_whitespace_does_not_dismiss_a_chain() {
        // §2.5's "typing past it into a space" means typing *content* past the
        // picker, which moves the chain off the end of the input. A bare
        // trailing space has typed past nothing, and silently losing the
        // user's explicit routing to an invisible character would be a bad
        // trade for no benefit.
        assert_eq!(parse("buy tomatoes @weekly  "), chain("buy tomatoes", &["weekly"]));
    }

    #[test]
    fn content_typed_past_the_chain_dismisses_it() {
        assert_eq!(
            parse("buy tomatoes @weekly and bread"),
            ParsedCapture::PlainText("buy tomatoes @weekly and bread".to_string()),
            "§2.5 — the chain is no longer trailing"
        );
    }

    #[test]
    fn segment_case_is_preserved_for_the_resolver_to_fold() {
        assert_eq!(parse("buy tomatoes @Weekly"), chain("buy tomatoes", &["Weekly"]));
    }
}
