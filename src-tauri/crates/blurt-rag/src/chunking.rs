//! Chunking long text for embedding — `MODULE_04_EMBEDDINGS_RAG.md` §3.
//!
//! Pure logic, no model dependency: this module never loads or calls an
//! embedding model, which is what lets it be tested exhaustively and cheaply.
//!
//! §3 asks for ~256-token chunks with ~15–20% overlap, each carrying the
//! character range it covers so §7's jump-to-chunk navigation can scroll to and
//! highlight the exact span inside the full text. Those offsets are the reason
//! this module is worth the care: an embedding with a wrong offset still
//! retrieves the right item, so a mistake here fails silently at search time
//! and only surfaces as "tapping the source scrolls to the wrong place".
//!
//! ## Why words, not tokens
//!
//! Real tokenization belongs to the embedding model, and running it here would
//! drag the model into a module that has no other need for it. So chunk size is
//! approximated by word count — but *conservatively*, because §3's 256 is a
//! hard input limit rather than a target. English averages roughly 1.3–1.4
//! BPE tokens per whitespace-delimited word, so 256 words would be closer to
//! 340 tokens: the model would silently truncate the tail of every chunk, and
//! nothing downstream would report it. [`CHUNK_WORDS`] is sized to stay under
//! the limit with headroom for the tokenizer's own special tokens.

/// The embedding model's hard input limit in tokens (§3).
///
/// Recorded for the arithmetic behind [`CHUNK_WORDS`]; nothing counts real
/// tokens at this layer.
pub const CHUNK_TOKEN_LIMIT: usize = 256;

/// Words per chunk — [`CHUNK_TOKEN_LIMIT`] discounted by a conservative
/// tokens-per-word ratio, with room left for `[CLS]`/`[SEP]`.
pub const CHUNK_WORDS: usize = 180;

/// Words each chunk repeats from its predecessor: 30/180 ≈ 17%, inside §3's
/// 15–20% band. Overlap is what stops a sentence that straddles a boundary
/// from losing its meaning in both chunks.
pub const OVERLAP_WORDS: usize = 30;

/// Words between the starts of consecutive chunks.
const STRIDE_WORDS: usize = CHUNK_WORDS - OVERLAP_WORDS;

/// One embeddable span of an item's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// Position in the sequence, from zero — `embeddings.chunkIndex`.
    pub index: usize,
    /// The span verbatim, punctuation and internal spacing intact. Never
    /// re-joined from words: the model should see the text the user wrote.
    pub text: String,
    /// `embeddings.chunkStartOffset` — inclusive.
    pub start_offset: usize,
    /// `embeddings.chunkEndOffset` — exclusive.
    pub end_offset: usize,
}

/// Splits `text` into overlapping chunks for embedding.
///
/// Offsets are counted in **Unicode scalar values** (Rust `char`s), matching
/// the schema's "character range" wording rather than byte positions.
///
/// One caveat for whoever builds §7's jump-to-chunk in Module 6: JavaScript
/// string indices are UTF-16 code units, which agree with scalar counts for
/// everything in the Basic Multilingual Plane but not for astral characters —
/// most emoji included. That conversion belongs at the IPC boundary, and is
/// noted here because a silent off-by-one there would look exactly like a
/// chunking bug.
///
/// Text with no words at all yields no chunks: there is nothing to embed, and
/// an empty embedding would be a retrievable row matching nothing.
pub fn chunk(text: &str) -> Vec<Chunk> {
    let words = word_spans(text);
    if words.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    loop {
        let end = (start + CHUNK_WORDS).min(words.len());
        let first = &words[start];
        let last = &words[end - 1];

        chunks.push(Chunk {
            index: chunks.len(),
            // Sliced from the original rather than rejoined from `words`, so
            // punctuation and internal spacing reach the model as written.
            text: text[first.byte_start..last.byte_end].to_string(),
            start_offset: first.char_start,
            end_offset: last.char_end,
        });

        // Stopping on "the chunk reached the last word" is what keeps an exact
        // multiple of CHUNK_WORDS from emitting a duplicate tail chunk.
        if end == words.len() {
            break;
        }
        start += STRIDE_WORDS;
    }

    chunks
}

/// A whitespace-delimited word, located both ways: char offsets for the
/// schema's "character range", byte offsets so slicing stays O(1).
struct WordSpan {
    char_start: usize,
    char_end: usize,
    byte_start: usize,
    byte_end: usize,
}

fn word_spans(text: &str) -> Vec<WordSpan> {
    let mut spans = Vec::new();
    let mut open: Option<(usize, usize)> = None;

    for (char_index, (byte_index, character)) in text.char_indices().enumerate() {
        match (character.is_whitespace(), open) {
            (true, Some((char_start, byte_start))) => {
                spans.push(WordSpan {
                    char_start,
                    char_end: char_index,
                    byte_start,
                    byte_end: byte_index,
                });
                open = None;
            }
            (false, None) => open = Some((char_index, byte_index)),
            _ => {}
        }
    }

    if let Some((char_start, byte_start)) = open {
        spans.push(WordSpan {
            char_start,
            char_end: text.chars().count(),
            byte_start,
            byte_end: text.len(),
        });
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` distinct words, so a chunk's contents identify its position.
    fn words(count: usize) -> String {
        (0..count).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ")
    }

    fn word_count(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /// Slices `text` by the char offsets a chunk reports.
    fn slice_by_offsets(text: &str, chunk: &Chunk) -> String {
        text.chars()
            .skip(chunk.start_offset)
            .take(chunk.end_offset - chunk.start_offset)
            .collect()
    }

    #[test]
    fn empty_text_produces_no_chunks() {
        assert!(chunk("").is_empty());
    }

    #[test]
    fn whitespace_only_text_produces_no_chunks() {
        assert!(chunk("   \n\t  ").is_empty(), "there is nothing to embed");
    }

    #[test]
    fn short_text_is_one_chunk_covering_all_of_it() {
        let chunks = chunk("buy tomatoes this week");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "buy tomatoes this week");
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].start_offset, 0);
        assert_eq!(chunks[0].end_offset, 22);
    }

    #[test]
    fn text_of_exactly_one_chunk_is_not_split() {
        let text = words(CHUNK_WORDS);
        let chunks = chunk(&text);
        assert_eq!(chunks.len(), 1, "an exact fit must not emit a duplicate tail chunk");
        assert_eq!(word_count(&chunks[0].text), CHUNK_WORDS);
    }

    #[test]
    fn one_word_over_a_chunk_splits_into_two() {
        let text = words(CHUNK_WORDS + 1);
        let chunks = chunk(&text);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn no_chunk_exceeds_the_word_budget() {
        let text = words(CHUNK_WORDS * 4 + 7);
        for c in chunk(&text) {
            assert!(
                word_count(&c.text) <= CHUNK_WORDS,
                "chunk {} has {} words, over the budget",
                c.index,
                word_count(&c.text)
            );
        }
    }

    #[test]
    fn consecutive_chunks_overlap_inside_the_documented_band() {
        let text = words(CHUNK_WORDS * 3);
        let chunks = chunk(&text);
        assert!(chunks.len() >= 3, "expected several chunks, got {}", chunks.len());

        for pair in chunks.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            assert!(
                later.start_offset < earlier.end_offset,
                "chunks {} and {} do not overlap at all",
                earlier.index,
                later.index
            );

            let earlier_words: Vec<&str> = earlier.text.split_whitespace().collect();
            let later_words: Vec<&str> = later.text.split_whitespace().collect();
            let shared = earlier_words.iter().filter(|w| later_words.contains(w)).count();
            let ratio = shared as f32 / CHUNK_WORDS as f32;
            assert!(
                (0.15..=0.20).contains(&ratio),
                "overlap of {shared} words ({ratio:.3}) is outside §3's 15-20% band"
            );
        }
    }

    #[test]
    fn chunk_indexes_are_sequential_from_zero() {
        let text = words(CHUNK_WORDS * 3);
        for (position, c) in chunk(&text).iter().enumerate() {
            assert_eq!(c.index, position);
        }
    }

    #[test]
    fn every_word_lands_in_at_least_one_chunk() {
        let total = CHUNK_WORDS * 2 + 13;
        let text = words(total);
        let chunks = chunk(&text);

        for i in 0..total {
            let word = format!("w{i}");
            assert!(
                chunks
                    .iter()
                    .any(|c| c.text.split_whitespace().any(|w| w == word)),
                "{word} fell into the gap between chunks"
            );
        }
    }

    /// The load-bearing property for §7: the offsets must slice the original
    /// text back to exactly the chunk's own text.
    #[test]
    fn offsets_slice_back_to_the_chunks_own_text() {
        let text = words(CHUNK_WORDS * 2 + 5);
        for c in chunk(&text) {
            assert_eq!(
                slice_by_offsets(&text, &c),
                c.text,
                "chunk {} reports offsets that do not match its text",
                c.index
            );
        }
    }

    /// Offsets count characters, not bytes — so multi-byte text must not shift
    /// the highlight downstream.
    #[test]
    fn offsets_count_characters_not_bytes() {
        let text = "café naïve résumé";
        let chunks = chunk(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].start_offset, 0);
        assert_eq!(
            chunks[0].end_offset, 17,
            "17 characters, though more than 17 bytes"
        );
        assert_eq!(slice_by_offsets(text, &chunks[0]), text);
    }

    #[test]
    fn surrounding_whitespace_is_not_part_of_a_chunk() {
        let chunks = chunk("\n  buy milk  \n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "buy milk");
        assert_eq!(chunks[0].start_offset, 3);
        assert_eq!(chunks[0].end_offset, 11);
    }

    #[test]
    fn internal_spacing_and_punctuation_are_preserved_verbatim() {
        let text = "Call Dr. Ada  —  ask about the 3rd of May.";
        let chunks = chunk(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].text, text,
            "chunks are slices of the original, never words rejoined by spaces"
        );
    }

    #[test]
    fn a_single_word_is_a_single_chunk() {
        let chunks = chunk("tomatoes");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, "tomatoes");
        assert_eq!((chunks[0].start_offset, chunks[0].end_offset), (0, 8));
    }

    #[test]
    fn the_final_chunk_is_never_a_degenerate_sliver() {
        // A tail that would otherwise be one or two words still carries the
        // full overlap, so it has enough context to embed meaningfully.
        let text = words(CHUNK_WORDS + STRIDE_WORDS + 1);
        let chunks = chunk(&text);
        let last = chunks.last().expect("at least one chunk");
        assert!(
            word_count(&last.text) > OVERLAP_WORDS,
            "final chunk has only {} words",
            word_count(&last.text)
        );
    }
}
