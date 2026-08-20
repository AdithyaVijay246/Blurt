# Module 3 — Hierarchical List Router & Syntax Parser

Status: **Design finalized.** Not yet implemented. (A rough, non-final
proof-of-concept exists from early design discussion — front-loaded
`trigger@sub@sub payload` syntax with regex parsing. The design below
**supersedes** it, most notably by moving `@` to a trailing, UI-driven
picker instead of a leading typed prefix. Treat the earlier code as
throwaway scaffolding, not a spec to build from.)

## 1. Core Model

Triggers are **user-defined**, created alongside each list/note at
creation time — there is no fixed/hardcoded trigger vocabulary. This means
the router's job isn't "recognize known keywords," it's "resolve typed/
selected segments against the user's actual `destinations` table."

Every blurt is captured **instantly**, with zero blocking decisions.
Routing ambiguity is resolved asynchronously, never by holding up capture.

## 2. The `@` Interaction (typed input)

`@` is trailing, not leading. The user types freely and tags a destination
at the end if they want to (`buy tomatoes @weekly`), rather than deciding
the destination before they've said what they're blurting.

1. User types normally.
2. `@` followed immediately by a **space** → inert, treated as plain text.
   Picker never engages. (Covers cases like "meet @ 9pm.")
3. `@` followed by a non-space character → a live, filtered dropdown opens,
   searching destinations at the **current hierarchy depth**, updating as
   the user types.
4. Match found → user can tap the dropdown entry or keep typing to
   converge on it.
5. No match → dropdown shows a `+` create option in place of a list.
   - Tapping it creates a new destination **at the current depth** — i.e.
     if this is the first segment, a new standalone top-level destination;
     if it's a subsequent segment after an already-resolved parent, a new
     destination nested under that parent. This is how
     `@shopping@grocery` becomes "create `grocery` under `shopping`"
     without any separate parent-selection step — nesting falls out of
     *where in the chain* the new name was typed.
   - Ignoring it (typing past it, e.g. into a space) → nothing happens,
     it's just plain text (covers "@9pm" etc.).
6. After a segment resolves (matched or created), the dropdown **re-scopes**
   to that destination's children and the same interaction repeats for the
   next segment — full tap-through hierarchy, one consistent interaction
   pattern at every depth.
7. Reorganizing destinations after the fact (renaming, reparenting) is a
   **manual, separate** action via drag-and-drop in list management — the
   inline picker never handles reorganization, only creation/selection in
   the moment.

## 3. Explicit vs. Natural-Language Routing

### Explicit (`@` chain resolved)
Deterministic, no guessing. Once every segment resolves to a real
destination, the item is filed there immediately. No confirmation needed —
the user drove the selection directly via the picker.

### Natural-language (no `@` used at all)
For freeform input like *"I need to buy tomatoes this week."* Parsed via
lightweight local regex/keyword matching (**not** a full LLM call — that
would defeat the sleep-mode battery model for routine capture).

- **Confident match** → saves instantly to the guessed destination, with a
  dismissible, non-blocking **quick-confirm chip** shown under the input
  bar for a few seconds or until the next blurt. Not a blocking prompt —
  the save already happened.
- **No match at all** → saves instantly to the reserved **Unsorted**
  system destination (see `MODULE_02_SCHEMA.md` — `isSystem = true`, hidden
  from the normal `@` picker). An eye-catching icon with a numeric badge
  (notification-style) reflects the current Unsorted count. Resolving an
  Unsorted item later is just a normal move (`destinationId` update) —
  structurally no different from dragging a list around.
- **Optional setting:** "Always ask me where uncertain blurts go" — for
  users who'd rather triage immediately instead of async via the badge.
  Off by default; zero-friction capture is the default behavior, not this.

### Random Thoughts
An **ordinary** user-facing destination (not a system one), with its own
normal trigger. Reachable via explicit `@`, or offered as a manual option
when resolving an item from Unsorted — but **never auto-suggested** by NL
matching. Deliberately kept separate from Unsorted: Unsorted asks "where
does this belong?" (provisional, expected to be resolved); Random Thoughts
answers "does this belong anywhere at all?" (final — "the sky was purple
today" isn't waiting to be filed, it already is filed). Merging the two
would either give Random Thoughts a badge it doesn't deserve, or make the
Unsorted badge dishonest about what actually needs attention.

## 4. Voice Input

Voice input is transcribed to plain text and dropped into the input box,
then evaluated **after** transcription completes (not live) — the entire
transcript re-uses the exact same typed-input pipeline described above,
via one normalization pre-step:

1. Every standalone occurrence of the word **"at"** in the transcript gets
   highlighted red and swapped to `@`.
   - Note: speech-to-text engines transcribe spoken "at" as the word "at,"
     not the symbol `@` — so voice segment-splitting must key off the word
     "at," not the character.
   - This means "at" is effectively reserved in voice-parsed blurts;
     unrelated uses (e.g. "meet Alex at 9pm") will also get flagged. This
     is a known, accepted tradeoff — a one-time per-sentence dismissal
     cost, judged cheaper than silently mis-routing.
2. If the user dismisses a specific highlight, that instance reverts to
   plain "at" text and is left alone.
3. Any `@` symbols remaining after review are resolved through the
   **identical** chain-resolution logic as typed `@`: match existing
   destination, or flag for create/leave-as-text if unmatched (first
   invalid segment in a chain is what gets flagged; fixing it re-validates
   everything downstream).
4. Multi-segment voice chains require an explicit "at" between each
   segment, same rule as typed `@` requiring the symbol between segments —
   no inferring hierarchy from word order alone.

Net effect: voice adds **zero new parsing logic** — it's entirely a
pre-processing normalization pass that converts a transcript into
`@`-syntax text, after which it's indistinguishable from typed input.

## 5. Long-Form Content (Notes)

Two triggers for converting a blurt into a full note instead of a routed
list item:

- **Typed:** crossing a character-count threshold surfaces an affordance —
  dragging/holding the input box upward expands it into a full
  note-editing surface. Everything typed so far copies into the new note;
  the user continues typing there uninterrupted.
- **Voice:** the same length check happens **after** the full transcript
  (and `@`-normalization pass) completes, since you can't sensibly offer
  a live "expand" gesture mid-speech.

### Notes and `@`
Notes still use the exact same `@` picker/routing system as list items —
but **file to a single destination**, no comma-split "multiple items"
behavior (a paragraph isn't a list).

### Notes as append-only logs
A note is not one continuously-growing block of text — it's a
**chronological log/container of entries**, structurally identical to a
list's items (same `items` + `edits` tables from Module 2). Each new blurt
filed to a note destination becomes a new, separately-timestamped entry
within it, rendered as a scrolling journal rather than a checklist. No new
data-layer mechanism was needed for this — it reuses append/edit/tombstone/
embedding machinery already defined for list items.

## 6. Search & Memory Implications

- Every **version** of an item's text (original + each edit) is
  independently embedded — searching either the old or new wording finds
  the item. This is a deliberate "mind map" philosophy: recall should work
  off whatever fragment of a memory the user still has, not just current
  state.
- Long notes are chunked (~256-token segments, matching the embedding
  model's input limit) for semantic search, **and** separately pass through
  local keyword/keyphrase extraction (cheap, no LLM) stored as filterable
  tags. Chunking preserves nuance for meaning-based recall; keywords
  provide fast, cheap exact-match filtering. Keyword extraction is an
  additive enhancement, not a replacement for chunked embeddings.
- Search results dedupe by item ID regardless of how many chunks/versions
  matched (see `MODULE_02_SCHEMA.md`).

## 7. Explicitly Deferred / Not Yet Designed

- Exact character-count threshold for note conversion.
- Visual/interaction spec for the drag-to-expand gesture itself (this doc
  covers *when* it triggers and *what* happens to the content, not the
  animation/gesture feel — that's a Module 6 UI detail).
- Smarter (non-string-match) detection of ambiguous spoken "at" — flagged
  as unresolved during design; current fallback is highlight-everything
  and let the user dismiss false positives.
