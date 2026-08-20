# Module 4 — Local Embeddings & RAG Pipeline (LanceDB + Ollama)

Status: **Design finalized.** Not yet implemented.

## 1. Core Model

This module is Blurt's memory layer — how captured items become
searchable, and how natural-language questions get answered from the
user's own history. It has three consumers of the same underlying index:

1. **Capture-time indexing** — every item gets embedded (and re-embedded
   on edit) as a background process, invisible to the user.
2. **Plain search** — LLM-free, instant, browsable retrieval.
3. **Sleep-Mode AI ("ask")** — natural-language question answering,
   synthesized by a local generative LLM grounded in retrieved results.

All three share the same embedding index and hybrid retrieval mechanism;
they differ only in whether a generative model gets involved and how
results are scoped/displayed.

## 2. Two Models, Two Lifecycle Policies

Sleep-Mode's entire rationale rests on treating these as separate
resources with separate costs:

| Model | Size | Lifecycle |
|---|---|---|
| Embedding model (`all-MiniLM-L6-v2` / `bge-small`) | ~100MB | Effectively always-on. Cheap enough to run on every single capture/edit without a meaningful battery or latency cost. |
| Generative LLM (Qwen 2.5 3B / Llama 3.2 3B / Phi-3.5) | Several GB | **Sleep-Mode**: unloaded by default. Loads only when a query is classified as a question requiring synthesis, runs inference, then unloads immediately after. Never kept resident "just in case." |

Rationale: keeping a multi-GB generative model resident in memory 24/7 for
a feature the user might invoke a handful of times a day would meaningfully
drain battery on mobile for no benefit. The embedding model is cheap enough
that this tradeoff doesn't apply to it.

## 3. Indexing Pipeline

- **Trigger:** async, non-blocking, per item — industry-standard pattern.
  Capture saves instantly (per the app-wide zero-friction rule); embedding
  generation is queued as a background job immediately after.
- **Edit debouncing:** edits wait ~1–2 seconds after the user stops typing
  before triggering re-embedding, so active revision doesn't generate an
  embedding per keystroke — only the settled version gets embedded.
- **Per-version embeddings:** every version of an item's text (original +
  each edit) gets its own embedding entry, per the mind-map search
  philosophy established in Module 3 — searching old or new wording both
  find the item.
- **Chunking for long notes:** ~256-token chunks (matching the embedding
  model's hard input limit), generated with **~15–20% overlap** between
  consecutive chunks — standard RAG practice, prevents meaning loss when a
  sentence falls on a chunk boundary. Each chunk stores its character
  offset range (`chunkStartOffset`/`chunkEndOffset` in `MODULE_02_SCHEMA.md`)
  to support jump-to-chunk navigation (§7).
- **Keyword extraction:** runs alongside embedding, using **YAKE**
  (unsupervised, no reference corpus required, performs well on the
  short-to-medium text Blurt mostly captures — a better fit than TF-IDF,
  which needs a comparison corpus, or RAKE, which tends to produce noisier
  phrases). Extracted keywords are:
  - Stored in the `keywords` table (Module 2), indexed for fast exact-match
    filtering.
  - **Shown to the user** as visible tags (e.g. under a note) — fully
    transparent, not a hidden backend-only mechanism.
- **Sensitive exclusion:** if `destinations.isSensitive = true`, none of
  the above runs. No embedding, no chunking, no keyword extraction. Content
  is simply invisible to this entire pipeline. Non-negotiable, and applies
  identically regardless of local-only or cloud-provider mode (see §6).

## 4. Retrieval — Hybrid Search

Every query (from plain search or an "ask") runs the same underlying
retrieval:

- **Hybrid ranking:** combines semantic similarity (vector search against
  embeddings) with keyword exact-match boosting (against the `keywords`
  table) — catches both fuzzy/paraphrased queries and exact-term lookups.
- **Sort order:** pure relevance within whatever result set is being
  shown. Recency is not double-applied as a secondary ranking factor —
  time-scoping (§5) already handles recency bias at the window level, so
  re-weighting by recency again within that window would risk burying a
  genuinely better match just because it's older.

## 5. Retrieval Scope — Time Windows Differ by Mode

- **Sleep-Mode (ask):** defaults to the **last 30 days**. This keeps the
  synthesis stage's context manageable (a 30-day window is a few thousand
  items even for a heavy user, comfortably filterable to a reasonable
  top-N for the LLM) and matches how people actually think about recall —
  "what did I blurt about X" usually means recently. A **"load more"**
  action expands the window (next 30/60/90 days, or unbounded) if the
  answer feels incomplete.
- **Plain search:** no window by default — searches the full history,
  since there's no LLM context-size constraint to manage. Paginated /
  infinite-scroll if the result set is large.

## 6. `@` Scoping — Soft Prioritization, Not a Hard Filter

Both search and ask support scoping a query to a specific destination via
the same `@` picker used for routing (Module 3), e.g. `oranges @shopping`.
Scoping is a **display/ranking hint, not a query-time exclusion** — a hard
filter risks hiding a real match that was simply filed somewhere the user
didn't expect (e.g. under `@pantry` instead of `@shopping`), which conflicts
with the "nothing is ever truly unfindable" principle.

- **Plain search:** results render in two sections — **"In [Scope]"**
  (items living in the scoped destination, ranked by relevance) followed by
  **"Elsewhere"** (everything else that matched, also ranked by relevance).
  Both sections always render; scope changes grouping/prioritization, not
  visibility.
- **Ask mode:** the LLM synthesizes its answer from **in-scope results
  first**. If the in-scope set is empty or too thin to answer well, it
  falls back gracefully to the best elsewhere-match rather than hard-failing
  — e.g. "Nothing in Shopping, but I found a mention in Random Thoughts —
  want me to include it?" This mirrors the empty-state fallback in §8 rather
  than introducing a separate failure mode.
- Keyword matching respects the same in-scope/elsewhere split as semantic
  matching — both halves of hybrid search are grouped identically, no
  separate scoping logic per mechanism.

## 7. Sources & Navigation

Both modes surface real underlying items as sources/results, never just a
bare AI-generated claim with nothing to verify it against:

- Each result/source shows its live destination path (computed from the
  `parentId` chain, per Module 2 — never a stored path string) and
  timestamp.
- Tapping a source **jumps directly to the matching chunk within its
  original context**, scrolled and highlighted — not just "open the note
  and let the user find it." This is why chunk offsets are stored in the
  schema (§3): knowing which chunk matched isn't enough without knowing
  exactly where it sits in the full text.
- For list items, this is simpler — jump straight to the item in its list.

## 8. Sleep-Mode Answer Presentation

- The generative LLM reads the top-N retrieved results (bounded by the
  30-day-default window, not an arbitrary global rank cutoff) and produces
  a synthesized natural-language answer.
- The answer is visibly labeled (e.g. "AI summary") — distinct from the
  sources list below it — so the user always knows they're looking at a
  generated synthesis, not a direct quote, and can verify against the real
  underlying blurts if the summary seems off. Stakes here are low (a wrong
  grocery-list summary, never sensitive content, which never reaches this
  pipeline at all) but the framing should stay honest regardless.
- **Empty-result state:** if genuinely nothing matches within the current
  window, the UI shows an actionable message — e.g. *"Couldn't find
  anything about that — try rephrasing, or expand the search to look
  further back"* — paired with the window-expansion action, rather than a
  flat dead end.

## 9. Statement vs. Question Classification

The same input bar handles capture and ask — the system must classify
intent locally, without invoking the generative LLM just to decide (which
would defeat the sleep-mode battery model entirely).

- **Lightweight local heuristics only:** ends in `?`; starts with an
  interrogative word (who/what/when/where/why/how/does/is/are/can/did/
  should/could); inverted auxiliary-verb-first structure. These reliably
  cover the overwhelming majority of real questions in English.
- **Classified as a statement** → runs through the normal Module 3 router
  (trigger/`@` resolution, NL matching, Unsorted fallback).
- **Classified as a question** → skips the router, runs Sleep-Mode
  retrieval + synthesis instead.
- **Misclassification handling:** kept low-stakes and reversible, not
  over-engineered. A "did you mean to ask this?" affordance (same pattern
  as the quick-confirm chip from Module 3) lets the user correct a
  statement that was actually meant as a question. The inverse case
  (a rhetorical/thinking-out-loud question typed into a non-chatbot capture
  tool) is treated as user error against the app's actual purpose, not a
  case worth building special handling for.

## 10. Cloud Provider Interaction

Per the pluggable `AIProvider` interface in `BLUEPRINT.md` §1 — if a user
opts into a cloud provider (OpenAI/Claude/etc.) instead of the local
generative LLM:

- **Retrieval always stays 100% local**, regardless of provider choice.
  Only the final synthesis step's *input* (already-retrieved, non-sensitive
  snippets) would ever be sent externally — never raw database content,
  never the retrieval/ranking process itself.
- **The `isSensitive` exclusion is absolute and unaffected by provider
  choice.** Sensitive content was never indexed in the first place (§3),
  so there is nothing sensitive for a cloud provider to ever receive,
  local-only or not. This is a non-negotiable, cross-module constraint
  (see `BLUEPRINT.md` §4 and `CLAUDE.md`).

## 11. Explicitly Deferred / Not Yet Designed

- Exact top-N cap for how many retrieved results get fed to the generative
  LLM during synthesis (bounded by the 30-day window in practice, but no
  precise number chosen yet).
- Exact wording/visual treatment of the "AI summary" label and the
  "did you mean to ask this?" affordance — UI copy and styling belong to
  Module 6.
- Standalone search-icon placement in the UI shell — functionally specified
  here (LLM-free, full-history, `@`-scoped, distinct entry point from the
  input bar), but its exact location/affordance is a Module 6 concern.
