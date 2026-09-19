# Implementation Progress

**This file is the source of truth for what has actually been built.** The
module docs and `BLUEPRINT.md` §3 track *design* status — all six modules are
design-finalized, which says nothing about what code exists. Only this file
tracks that.

> **Read this at the start of every session, together with `git log`.** They
> answer different questions: git records *what changed*, this file records
> *where we are and why* — the resume point, the decisions behind the code,
> and the traps. Neither alone is enough.
>
> **If this file and git history disagree, git is right about what happened**
> and this file is stale. Reconcile it rather than trusting it blindly.
>
> **Update this file and commit before the session ends.** A commit without
> the update loses the reasoning; an update without a commit loses the diff.

Remote: `https://github.com/AdithyaVijay246/Blurt`

Last updated: 2026-09-19

A roadmap through the rest of Module 3 (router) and Module 4 (embeddings/RAG)
is saved at `C:\Users\adith\.claude\plans\dynamic-gliding-wolf.md` — Phases 1-5
below are done; only Phase 6 (final `blurt-app` wiring) is still
ahead. Read that plan file at the start of the next session
rather than re-deriving the sequencing here.

---

## Resume here

**Next action:** finish Phase 6 — two items left.

1. **The sensitive-flip purge.** Marking an existing destination sensitive must
   call `blurt_rag::indexing::remove_item` for its items. **There is no way to
   flip the flag yet**: `isSensitive` is only ever set by `destinations::create`,
   and neither `blurt-schema` nor `blurt-app` has a `set_sensitive`. So this
   starts with that repository function and its command, and the purge hangs off
   the command. Retrieval already refuses to surface such content (#36), so the
   purge reclaims space rather than fixing a leak. Note `remove_item` is async
   *and* takes a `Connection` — it needs the same split as `index_item` (#62)
   before a command can call it.
2. **Bundle the two model files.** `rag_paths` already resolves them from
   Tauri's bundled resources (decision #61) and tolerates their absence, so this
   is obtaining the files and adding `bundle.resources` entries. Until then
   `ask` fails at load time and **the embedding model downloads on first use** —
   which now matters in practice, because captures are indexed (see below) and
   an offline first capture fails to embed until the catch-up pass retries it.

After that, Phase 6 is done and the backend is complete. `tauri-specta` is worth
revisiting at that point (decision #12), since Module 6 will consume the
bindings.

**Open question for the user, raised 2026-09-19 and not yet answered:**
`AppState.rag` is not cleared on lock, so after lock then unlock of a
*different* vault, search, ask and the indexer reuse the previous vault's
LanceDB handle. Pre-existing, not introduced by the indexer. Ask before fixing.

**Just finished: background indexing** (plan:
`C:\Users\adith\.claude\plans\blurt-background-indexer.md`). Captures and edits
are now actually embedded, so **search returns results on a real vault** for
the first time. Decisions #62–#65.

- Captures are indexed immediately, edits after a 1.5s quiet period with a
  newer edit replacing the waiting one, all through one serial worker in
  `blurt-app/src/indexer.rs`.
- Unlock re-queues anything whose latest version was never indexed.
- The frontend gets an `item-indexed` event when a job embeds something.

## Status by component

| Component | State | Notes |
|---|---|---|
| Repo scaffold | **Done, verified** | Workspace builds clean; `npm run tauri dev` opens a blank window |
| Frontend skeleton (`src/`) | **Stub only** | Blank `App.tsx`; `npm run build` passes. No Module 6 work started |
| `blurt-schema` — keyring | **Done, green** | Key-wrapping, 3 slot kinds, recovery-key encoding |
| `blurt-schema` — DDL | **Done, green** | `migrations/0001_initial.sql`, all §2 tables + indexes + seeds |
| `blurt-schema` — db/migrations | **Done, green** | SQLCipher raw-key open + probe; `user_version` runner |
| `blurt-schema` — repository/CRUD | **Done, green** | `repository/{destinations,items,edits,embeddings,keywords,indexing}.rs`; `list_children`/`list_all` + seed ids for M3, `is_item_indexable`/`store_index_results`/`edits::get_by_id` for M4; `repository/secrets.rs` for the `app_secrets` recovery key |
| `blurt-router` (M3) | **Done, green** | `chain`/`candidates`/`nl`/`voice`/`resolve` — all of `MODULE_03_ROUTER.md`. Decides only; never writes |
| `blurt-rag` (M4) | **Done, green** | All of `MODULE_04_EMBEDDINGS_RAG.md`: `chunking`/`embedding`/`keywords`/`vectorstore`/`indexing`/`search`/`classify`/`model_manager`/`synthesis`/`llama`. Retrieval is exposed as an async `retrieve_matches` plus a synchronous `rank` so a Tauri command can call it (decision #59). Real inference works; the GGUF is not bundled yet, so the ask path needs a model file before it runs end to end |
| `blurt-sync` (M5) | **Empty stub** | Will add its own migration for `yrs` update logs + paired devices |
| `blurt-app` | **Partial, green** | Vault lifecycle, CRUD slice, **capture via router**, **voice normalization**, **`classify_input`/`search`/`ask`**. Every domain crate is now reachable over IPC. **Background indexer** (capture, debounced edits, catch-up on unlock, `item-indexed` event). Missing: the sensitive-flip purge |

**360 tests green** across the workspace as of the last commit (109
`blurt-schema` + 65 `blurt-router` + 124 `blurt-rag` + 62 `blurt-app`), plus 10
`#[ignore]`d, which split into two groups with **different levels of proof**.
Eight need the real ~100MB embedding model and were last run and confirmed green
on 2026-09-10. The two added on 2026-09-15 need a real GGUF via
`BLURT_TEST_GGUF` and have **never been run**, so end-to-end text generation is
unproven until Phase 6 bundles a model. Everything else — including all of
`model_manager` and `synthesis` — is tested through fakes, which is the point
of their trait seam. Test command (note the PATH requirements under Environment below):

```bash
cargo test --workspace
# The real ~100MB embedding model. --test-threads=1 is REQUIRED, not optional:
# see the shared-model-cache note under Environment.
cargo test -p blurt-rag -- --ignored --test-threads=1
```

Use `--no-fail-fast` when you want the whole workspace's results: without it
cargo stops at the first failing crate, which is easy to misread as "the rest
passed" when they simply never ran.

Two bits of expected noise in that output, neither a problem:
`ERROR CORE sqlcipher_page_cipher: hmac check failed for pgno=1` is SQLCipher
logging the wrong-key rejection during `wrong_key_is_rejected_at_open_not_later`
— it is the test working. And `LNK4099` warnings are OpenSSL's static lib
shipping without PDB debug info.

---

## Implementation decisions not covered by any design doc

Recorded here because a future session would otherwise re-derive or contradict
them. Each follows a pattern the docs already establish, but none is stated in
one.

1. **Recovery key is stored inside the encrypted database** (`app_secrets`
   table). `MODULE_02_SCHEMA.md` §3/§5 originally said it was "never stored", but
   `MODULE_06_UI_SHELL.md` §D2 requires Settings to re-display it — a direct
   contradiction. Resolved in favor of storing it inside SQLCipher: plaintext
   nowhere on disk, readable only when unlocked *and* past a fresh auth check.
   `MODULE_02_SCHEMA.md` §3 and §5 have been amended to match, so the docs no
   longer disagree. Note the circularity §5 now spells out: the stored copy is
   readable only once the database is already open, so it serves §D2
   re-display, *not* recovery — real recovery still depends on the user having
   saved the key externally.

2. **Keyslots live in a separate `keyring.json`, not in the database.** The
   wrapped-key blobs cannot live inside the database they unlock. The file is
   not secret — every slot is useless without its passphrase or recovery key.

3. **The recovery slot uses HKDF-SHA256, not Argon2id.** Argon2id exists to make
   low-entropy passphrases expensive to guess; the recovery key is already 256
   bits of CSPRNG output. Passphrase slots use Argon2id at 64 MiB / t=3 / p=1.

4. **The sensitive-info passphrase (§5) is a third keyslot.** Verification is
   "does it unwrap", so there's no separate verifier to keep in sync. Works
   identically whether the user reuses the master passphrase or sets a distinct
   one — each slot has its own salt, so reuse isn't detectable from the file.

5. **Recovery key is 256-bit, encoded as 13 Crockford base32 groups of 4.**
   §B4's `XXXX-XXXX-XXXX-XXXX` example is only 80 bits, which would make the
   recovery slot the weakest link against a 256-bit master key. Crockford omits
   `I`/`L`/`O`/`U` so paper transcription is unambiguous. §B4's "re-enter two
   groups" verification is unaffected.

6. **Seed destinations use fixed, well-known UUIDs**
   (`00000000-b1c7-4000-8000-00000000000{1,2}`). Every device runs migration
   0001 on first launch, before pairing. If each generated its own ids, pairing
   two devices would produce *two* Unsorted and *two* Random Thoughts rather
   than converging — Module 5 keys a CRDT doc per destination id.

7. **`trigger` uniqueness is a partial index over live rows**
   (`WHERE deletedAt IS NULL`). Deletes are tombstones, so a plain `UNIQUE`
   would burn a trigger permanently the first time its destination was deleted.

8. **Storage conventions:** UUIDs as TEXT (whole-DB encryption means no privacy
   cost, and it keeps Module 5's CRDT doc keys readable); timestamps as INTEGER
   Unix milliseconds (directly comparable for Module 5's LWW, and a JS `Date`
   with no conversion).

9. **`src-tauri/Cargo.toml` is both workspace root and a thin `blurt` host
   package.** `MODULE_01_ARCHITECTURE.md` §2 pins the workspace root there but
   doesn't say where `tauri.conf.json` goes; the conventional location needs a
   real package beside it. `src-tauri/src/main.rs` is three lines calling
   `blurt_app::builder()`. All Tauri commands belong in `crates/blurt-app`.

10. **Scope boundary for `blurt-schema`:** storage primitives only. The *policy*
    around them — the fresh uncached auth gate for sensitive destinations, the
    app-level idle timer, biometric integration — belongs to `blurt-app` and
    Module 6.

11. **`repository/` is a directory, not a flat file** — the one place this
    crate breaks from the otherwise-flat `src/*.rs` layout. The CRUD surface
    (destinations + items + edits + a sensitive-safe accessor) was bigger than
    any existing single file; split into `repository/{destinations,items,edits,
    indexing}.rs` instead. Repository functions take `&rusqlite::Connection`
    directly (via `Database::conn()`) rather than wrapping it, matching
    `migrations::run`'s existing shape. Read accessors return
    `Result<Option<T>>` via `rusqlite`'s `OptionalExtension`, not a `NotFound`
    error variant — missing-by-id isn't exceptional here.

12. **`blurt-app` skips `tauri-specta` for now.** It has never had a stable
    release targeting Tauri v2 — only a long-running `2.0.0-rc.x` pre-release
    exists (confirmed via crates.io, latest `2.0.0-rc.25` as of 2026-05-08) —
    and there is no frontend yet to consume generated bindings (`src/App.tsx`
    is still a blank stub). Commands use plain `#[tauri::command]` +
    `tauri::generate_handler!`; DTOs (`blurt-app/src/dto.rs`) are hand-written
    `serde` structs mirroring `blurt_schema`'s domain structs, not derives
    added to `blurt_schema` itself (that crate stays storage-primitives-only).
    Revisit adopting `tauri-specta` whichever session actually starts Module 6.

14. **A chain-opening `@` must sit on a word boundary.** §2 describes the live
    picker, not a parser, and says nothing about what precedes the `@`. Without
    a boundary rule, `email bob@example.com` parses `example.com` as a
    destination segment. So the `@` that *opens* a chain must be preceded by
    whitespace or start the input; `@`s *inside* an already-open chain need no
    boundary, which is what keeps §2.6's `@shopping@grocery` working.

15. **Trailing whitespace does not dismiss a chain.** §2.5's "typing past it,
    e.g. into a space" means typing *content* past the picker — which moves the
    chain off the end of the input and is handled by the trailing-only rule
    already. A bare trailing space has typed past nothing, and losing the
    user's explicit routing to an invisible character buys nothing. `parse`
    trims trailing whitespace before looking for a chain.

16. **Voice normalization consumes the whitespace after "at", and closes the
    gap between consecutive segments.** Both are forced by the typed grammar
    rather than stated in §4. Rewriting "at weekly" to `@ weekly` would produce
    exactly the construction §2.2 calls inert, so voice routing could never
    fire; and rewriting "at shopping at grocery" to `@shopping @grocery` would
    make two unrelated chains of which only the last survives, rather than the
    single two-segment chain §4.4 describes. `normalize_spoken_at` therefore
    returns `SpokenAt { offset, original }` per rewrite — `original` holds the
    replaced text verbatim so §4.2's per-instance dismissal restores
    capitalisation and spacing exactly.

17. **NL confidence formula** (no formula is specified in
    `MODULE_03_ROUTER.md`, and it is not in that doc's own "Explicitly
    Deferred" list, so it is an implementation detail rather than a
    stop-and-ask item):
    `score = 1.0×(trigger appears as a whole word) + 0.6×(name appears as a
    whole word) + 0.4×(fraction of the name's words present)`, confident above
    `0.6`. The non-obvious consequence, documented in `nl.rs` too: the fraction
    term alone tops out at `0.4`, so a confident match *always* requires a
    whole-word trigger or name hit, and the fraction only separates candidates
    that both already hit. Erring conservative is right — an unconfident blurt
    lands in Unsorted with a badge, a wrong confident guess hides it.

18. **NL candidate exclusions are `isSystem` and Random Thoughts only.**
    §3 names exactly those two. Sensitive destinations stay eligible: NL
    matching is local string comparison with no model involved, so the
    "secrets never touch a model" constraint is not in play, and excluding
    them would silently mis-route. Random Thoughts is recognised by
    `RANDOM_THOUGHTS_ID`, not by name, since the user may rename it.

19. **`route` returns `Routing { text, decision }`, not a bare
    `RoutingDecision`.** Every decision variant needs the text it was made
    about; hanging it off the enum would repeat the same `text: String` field
    four times. The variant set is unchanged. `Create` describes only the
    *first* unresolved segment, per §4.3's "fixing it re-validates everything
    downstream", but carries the rest in `remaining_segments` so nothing is
    silently dropped.

20. **`list_children(conn, Option<Uuid>)` is one function, not a
    `list_children`/`list_top_level` pair.** `None` is the top level. Module
    3's picker walks depth with exactly that `Option<Uuid>` shape, so one
    function fits both callers; SQLite's null-safe `IS` operator makes it one
    statement too. `list_all` is the separate flat variant the NL fallback
    needs, since freeform text carries no depth to scope by.

13. **Every `blurt-app` command splits into a testable `<name>_impl(state:
    &AppState, ...)` plus a one-line `#[tauri::command]` wrapper that just
    unwraps `State` and delegates.** Forced by an environment issue, not a
    preference — see the `tauri::test` gotcha below. The `_impl` functions
    are the actual tested logic; the wrapper is untested by an automated test
    (a one-line delegation, low risk). Follow this pattern for every future
    command in this crate, not just the current ones.

---

21. **Chunks are sized in words, deliberately below §3's 256-token limit.**
    Real tokenization belongs to the embedding model, so chunk size is
    approximated by word count — but English runs ~1.3-1.4 BPE tokens per word,
    so a literal 256 *words* would be ~340 tokens and the model would silently
    truncate the tail of every chunk with nothing downstream reporting it.
    `CHUNK_WORDS = 180`, `OVERLAP_WORDS = 30` (~17%, inside §3's 15-20% band).

22. **Chunk offsets are Unicode scalar counts, not bytes.** This matches the
    schema's "character range" wording. One trap for Module 6: JavaScript string
    indices are UTF-16 code units, which agree with scalar counts across the
    BMP but *not* for astral characters — most emoji. That conversion belongs at
    the IPC boundary; a silent off-by-one there would look exactly like a
    chunking bug.

23. **Keyword extraction had to be made deterministic across processes.**
    `yake-rust` breaks tied scores by hash iteration order, and Rust reseeds
    that per process — a single note routinely produces several phrases scoring
    bit-identically. Left alone, the same note showed *different tags on
    different app launches*, and at the ten-item cut which tied phrase survived
    was arbitrary. Fixed by asking YAKE for `word_count * NGRAM_SIZE`
    candidates — a strict upper bound on what it can generate, so it never
    truncates — then applying our own total order on `(score, text)` and cutting
    to ten. A fixed over-fetch is *not* sufficient: it holds for short captures
    and fails silently on long notes, which is where tags matter most.
    `yake_never_truncates_at_the_candidate_ceiling` guards the premise.

24. **Tags replace; chunk rows accumulate.** §3 embeds every text version so the
    old wording stays searchable, but a tag list is a claim about what an item
    *is now* — showing tags drawn from text the user has since rewritten would
    be wrong on screen, not merely redundant. Hence
    `keywords::replace_for_item` versus `embeddings::insert_many`.

25. **Embeddings and keywords are hard-deleted, not tombstoned.** The
    append-only rule in `BLUEPRINT.md` §4 protects what the user *wrote* —
    `items` and `edits` — and an index entry is derived data that can be
    rebuilt. Tombstoning it would mean either filtering on every search or
    surfacing deleted text as a result.

26. **LanceDB is written before SQLCipher, and SQLCipher is authoritative.**
    Interrupted between the two, the result is orphaned vectors that nothing
    joins to: unreachable and harmless. The reverse order would leave
    `embeddings` rows pointing at vectors that do not exist, which a search
    would surface as results that cannot be opened. `store_index_results`
    exists as one function because `embeddings::insert_many` and
    `keywords::replace_for_item` each open their own transaction and SQLite
    will not nest them.

27. **`index_item` is not idempotent, on purpose.** Calling it twice for the
    same version stores a second set of chunks. Search dedupes by item so the
    user-visible effect is nil; the scheduler in `blurt-app` (Phase 6) owns not
    double-firing. Making it idempotent would need delete-by-(item, version) on
    both stores, which nothing needs yet.

28. **`lancedb` is pinned below 0.38.** 0.38.0 does not compile without its
    `remote` feature: `src/job.rs` uses `Error::Http`, which `src/error.rs`
    gates behind that feature, while `pub mod job;` is left ungated — an
    upstream regression, since 0.37.1 does not reference it at all. Enabling
    `remote` would compile a LanceDB Cloud REST client into a local-first app to
    work around a missing `cfg`, so the pin is the honest fix. Recheck on the
    next release. Note 0.37.1 needs `lance =10.0.0`, so moving between these
    versions is a full rebuild of the lance/datafusion stack, not a quick swap.

29. **`fastembed` downloads its model from Hugging Face on first load.**
    `BLUEPRINT.md` §2's bundling requirement is written about the *generative*
    model, so the embedding model is technically uncovered — but "zero AI setup
    friction and offline capability from first launch" reads no differently for
    a model that runs on every capture. `Embedder::new` therefore takes a cache
    directory rather than defaulting it, so pointing it at a bundled Tauri
    resource makes the load offline with no code change. Phase 5 has to solve
    the same problem for the GGUF; solve both together.

30. **`blurt-rag` is async, which the roadmap did not anticipate.** `lancedb`'s
    API is async throughout, so `vectorstore` and `indexing` are too, and the
    eventual search/ask commands in `blurt-app` will be `async fn` — supported
    natively by Tauri v2. Not a problem: the roadmap already put a Tokio task in
    `blurt-app` for the edit debounce, so tokio was arriving regardless.
    `chunking` and `keywords` stay synchronous, being pure.

31. **The vector store asks LanceDB for cosine distance, not its default
    squared L2.** Set in `vectorstore::search`, and the only Phase 4 change
    outside `search.rs`. Phase 4 is where a distance first becomes a *score*,
    and only cosine gives that conversion a stable meaning: `1 - distance` is
    the cosine similarity, 1 for an identical direction and 0 for an orthogonal
    one. Under squared L2 an orthogonal pair comes back as `2`, which the same
    arithmetic would read as similarity `-1` — sorting a weak match below one
    that never matched at all. Caught by writing the test first;
    `distance_is_cosine_distance_so_it_can_be_read_as_similarity` pins it, and
    it fails with `got 2` if the metric is ever dropped. Note for whenever a
    vector *index* is added: LanceDB requires the query's distance type to
    match the type the index was trained with, or results are silently invalid.

32. **Keyword-side query terms are enumerated n-grams, not YAKE output.**
    `query_terms` emits every contiguous run of 1..=`NGRAM_SIZE` words,
    lowercased and stripped of edge punctuation, to match how
    `keywords::extract` stores them. Running YAKE on the query instead would be
    asking an unsupervised statistical method to find the important terms in a
    two-word text that is all important terms, and it could discard the very
    word the user searched for. Two properties make the naive approach safe:
    the stored side is already stopword-filtered, so an unfiltered query n-gram
    containing "the" simply matches nothing; and the expansion is capped at
    `MAX_QUERY_TERMS = 200` because the lookup binds one SQL variable per term
    against SQLite's default 999 ceiling. Unigrams are emitted first so
    truncation costs a long query its phrases, never its individual words.

33. **Hybrid score formula** (no formula is given in
    `MODULE_04_EMBEDDINGS_RAG.md`, and it is not in §11's deferred list, so it
    is an implementation detail in the same sense as decision #17's NL
    confidence score):

    `score = clamp(1 - cosine_distance, 0, 1) + 0.25 × (1 - 0.5^keyword_hits)`

    The semantic half occupies `0.0..=1.0`; the keyword half is capped at 0.25,
    enough to lift an exact-term match past a marginally closer paraphrase and
    never enough to lift a semantically unrelated item above a strong match.
    The `1 - 0.5^n` curve matters at both ends and the first draft got it
    wrong: a linear ramp saturating at three hits gave a single hit only
    0.083, too little to close even a 0.1 semantic gap — which left §4's
    "exact-term lookup" case unserved, since most queries are a single phrase
    matching a single stored tag. A failing test caught it. The first hit now
    earns half the boost; the asymptote is what still stops a heavily-tagged
    note out-ranking a precisely relevant one purely for carrying more tags.

34. **Keyword matching boosts; it does not recall.** An item whose tags match
    but which produced no vector hit is *not* injected into the results. §4
    calls the keyword half "boosting", and §7 is the reason to read that
    literally: every result must be able to jump to the chunk that matched,
    scrolled and highlighted, and a keyword-only hit has no matching chunk to
    jump to — it would be a different kind of object wearing the same shape.
    `VECTOR_OVERFETCH = 8` is what keeps this from costing recall: the vector
    side is asked for eight times the requested page, so an exact-term match is
    almost always already in the candidate set waiting to be boosted.

35. **The window filters on the version's own timestamp, and the result carries
    that same timestamp.** `edits.editedAt` for an edit chunk, `items.createdAt`
    for an original capture (roadmap decision #4, now implemented). One
    consequence worth stating: an item captured a year ago but edited today is
    *recent content* and stays in a 30-day window, which is the intent — §3
    embeds each version separately precisely so versions are independent.
    Using one timestamp for both filtering and display means §5's scoping and
    §7's displayed date can never disagree. A timestamp in the *future* is
    never excluded: Module 5 syncs from devices whose clocks may run fast, and
    dropping the newest thing the user wrote for being too recent helps nobody.

36. **Retrieval re-checks `is_item_indexable` per candidate.** That function is
    named for indexing but states exactly the rule retrieval needs — live item,
    live destination, not sensitive — so it is asked rather than duplicated.
    This is what closes the window on the Phase 6 gap above: a destination
    marked sensitive after its items were indexed still has vectors sitting in
    LanceDB, and without this check a search would surface them. Guarded by
    `an_item_whose_destination_became_sensitive_after_indexing_is_dropped`.
    Orphaned vectors (written to LanceDB by a pass interrupted before its
    SQLCipher commit — the interruption `indexing.rs` deliberately tolerates)
    are skipped the same way, per candidate, rather than failing the query.

37. **Pagination is offset-based over an over-fetched candidate set, not a
    cursor.** Page N is taken from a candidate set sized for pages 1..N, so
    deep pagination degrades. A real cursor would mean keeping ranking state
    between calls; §5 asks for pagination on plain search, which in a personal
    log is a handful of pages, and this is not worth building until the
    behaviour is observed to matter. Documented on `hybrid_search` itself so
    the limitation is visible at the call site rather than only here.

38. **Result ordering is total, including its tiebreak.** Scope group first
    (§6's two sections), then score descending, then `item_id`. The last term
    is not decoration: `rank` deduplicates chunks through a `HashMap`, whose
    iteration order Rust reseeds per process, so without it two identical
    queries could return tied results in different orders on different app
    launches. The same class of bug as decision #23's YAKE tag instability, and
    caught by remembering it.

39. **The §9 classifier reduces contractions, and does it in two steps because
    one is not enough.** Voice capture supplies no `?` (`MODULE_03_ROUTER.md`
    §4), so two of §9's three signals have to work on bare words — which makes
    contractions load-bearing rather than cosmetic. The first draft cut the
    opening word at its apostrophe and stopped there. That is right for
    "what's" → `what`, and wrong for "didn't" → `didn`, because the negation's
    `n` sits on the *auxiliary's* side of the apostrophe. Stripping that `n`
    unconditionally is wrong the other way: "can't" is already `can` + `t`, and
    taking an `n` off gives `ca`. So `opens_a_question` tests both the head and
    its `n`-stripped form. That is safe rather than merely convenient: no word
    in either list is another list word plus an `n`, so trying both cannot
    invent a match. "won't" is still missed (`wo`), which is irregular and rare
    enough to leave. Both the ASCII `'` and the typographic `’` phone keyboards
    insert are handled.

    Two smaller calls in the same file. Content-free input ("", "?") classifies
    as a **statement**, because a captured stray character is one tap to delete
    whereas one routed to Sleep-Mode vanishes into an empty search. And the
    known false positives are asserted in tests as *current behaviour* rather
    than left undocumented — "what a day", "how to reset the router", "can of
    paint" all classify as questions. §9 explicitly declines to engineer around
    that direction, so the tests exist to make the accepted cost visible, not
    to demand a fix. If real usage shows the interrogative-opening rule is too
    eager, the tuning knob is to require an inversion after the wh-word ("what
    *did* I") rather than accept a bare one — a behaviour change, so it belongs
    to real-usage data rather than a guess.

40. **Decision #23 was half a fix, and the missing half was a real bug.**
    Asking YAKE for its whole candidate set stopped *it* truncating in hash
    order — but the scores this crate then sorts on are not stable either.
    `yake-rust` accumulates its statistics through `HashMap`s, and Rust derives
    a fresh hash seed per map *instance* (`RandomState` bumps a thread-local
    counter), so summation order differs between two `extract` calls **in the
    same process**. The scores come back differing in their last bit or two.

    That is far below any meaningful difference in importance, but `total_cmp`
    respects it faithfully — which is enough to swap two adjacent tags, and at
    the `MAX_KEYWORDS` cut, to change which tag survives at all. Exactly the
    user-visible symptom #23 was written to eliminate, via a second mechanism
    it did not cover. `ordering_score` now rounds to 1e-9 before comparing, so
    genuinely-tied phrases compare equal and the existing text tiebreak decides
    deterministically. `Keyword::score` still carries the raw value, since
    Module 6 renders relative tag weight from it.

    Caught because `extraction_is_deterministic` compared two `Vec<Keyword>`
    with `assert_eq!`, and the derived `PartialEq` compares `f64` bit-for-bit —
    so the test was itself flaky, passing most runs. It now asserts what is
    actually guaranteed: identical tag *texts* in identical order, with scores
    compared to a tolerance. **The general lesson for this codebase: never sort
    user-visible output on a raw float that came out of a hash-ordered
    accumulation.** `search::rank` already avoids this by tiebreaking on
    `item_id` (decision #38); it was written before this was understood, and
    got there for the adjacent reason rather than this one.

41. **`llama-cpp-2`'s real API differs from its published docs in two ways that
    matter, and the verification gate is what caught them.** Checked against the
    vendored 0.1.156 source, not docs.rs — for a pinned version that is the code
    that will actually compile.

    First, `LlamaBackend::init()` is guarded by a process-wide `AtomicBool` and
    returns `BackendAlreadyInitialized` on a second call while one is alive
    (`Drop` resets it). So the **backend must be a process-wide singleton and
    the *model* is what loads and unloads** — which is the right split anyway,
    since `llama_backend_init` costs nothing while the weights are the
    multi-gigabyte part §2 cares about. A `ModelManager` that owned a backend
    could not be constructed twice, which would break tests before it broke
    anything else.

    Second, `LlamaSampler::sample` takes `&mut self`; the published example
    shows it called on an immutable binding. Also `LlamaContext<'a>` borrows the
    model, so the two cannot be stored in one struct — the context has to be
    built inside each generate call, which suits load/generate/unload exactly.

    Pinned with `=0.1.156` rather than caret. It is a 0.x crate, so caret would
    accept every 0.1.x, and those track upstream llama.cpp's own C API. An FFI
    crate whose patch bumps can carry an upstream API change is not one to
    float.

42. **`ModelManager` has a trait seam, and its unload is unconditional.**
    `TextGenerator`/`ModelLoader` exist so §3's policy — "load only on a
    classified question, unload immediately after, never resident" — is testable
    without a multi-gigabyte model. Every invariant in that file is about *when*
    the model is loaded and dropped, not about what it generates, so a fake
    generator tests all of them: `every_question_loads_again_because_this_is_not_a_cache`
    is the one that fails loudly if someone later "optimizes" it into a cache.

    The unload happens *before* the result is propagated, not after, so a failed
    generation still unloads. That early-return is the one mistake that would
    silently defeat §2's premise — gigabytes left resident precisely when
    something already went wrong — so
    `a_failed_generation_still_unloads` guards it rather than review.

    `std::sync::Mutex`, not `tokio`'s: generation is a long CPU burn, and
    callers in Phase 6 must wrap it in `spawn_blocking`. A `tokio::sync::Mutex`
    would invite holding it across an `.await` on an executor thread, which is
    the shape this is meant to prevent.

43. **§11's deferred top-N cap is resolved as a token budget, not a count**
    (`CONTEXT_TOKEN_BUDGET = 2400`), chosen by the user when asked. Blurts range
    from four words to several hundred, so any fixed N is either wasteful on
    short ones or over-long on a handful of real notes — and llama.cpp truncates
    an over-long prompt *without reporting it*, degrading answers in a way
    nothing in the app could observe. A budget cannot do that. Spent against an
    estimate of 1.4 tokens per English word, since the model is not loaded when
    the prompt is built and loading it to count tokens would invert Sleep-Mode's
    whole lifecycle. Erring high is the safe direction.

    Selection stops at the first result that does not fit rather than skipping
    it for a smaller one behind it: §4 established the ranking and length is not
    a relevance signal.

44. **There is deliberately no all-in-one `answer_question`.** Retrieval is
    async (LanceDB); generation is a long CPU burn behind a blocking mutex. One
    function spanning both would either stall the async executor for the length
    of an inference or force `tokio` into this crate's runtime dependencies
    purely to work around itself. So `synthesis::answer_from_sources` takes
    already-ranked results, and Phase 6 composes it with `hybrid_search` and
    `spawn_blocking` explicitly — the composition is written out in
    `synthesis.rs`'s module docs. This deviates from the roadmap, which asked
    for `answer_question`; shipping the convenience would have shipped a
    footgun. It also keeps the expensive half testable without a model, the same
    way `search::rank` is.

45. **`AnswerLabel` is an identifier, not display copy.** §11 assigns the exact
    wording of the "AI summary" label to Module 6, so putting the English string
    in a Rust crate would both contradict that and hardcode UI copy below the
    IPC boundary. But `CLAUDE.md`'s "transparency without interruption" requires
    generated content to be *marked* as generated, and a marker that crosses IPC
    as structured data is one the frontend must handle rather than one it might
    forget. So the enum carries `AiSummary` and Module 6 maps it to whatever it
    renders.

46. **An empty result set is its own variant, not an empty answer.**
    `SleepModeOutcome::NothingFound { window }` rather than a `SleepModeAnswer`
    with an empty string. §8 makes the empty state a different screen — paired
    with the widen-the-window action — and an empty answer string would invite
    Module 6 to render a blank summary card, which reads as "your notes say
    nothing" rather than "nothing matched". The window that came up empty
    travels with it so the UI can say what was searched and offer the next rung
    of §5's ladder. **The model is never loaded on this path**: there would be
    nothing to ground an answer in, and that is the exact setup for inventing a
    memory the user will read as their own.

47. **Prompt timestamps are relative ("3 days ago"), not calendar dates.**
    Avoids both a date-formatting dependency and a question the schema cannot
    answer: it stores UTC milliseconds, and rendering a local civil date needs
    an offset this crate has no business knowing. Relative age is also the form
    a recall question is usually asked in. A future timestamp reads as "today"
    rather than a negative day count, for the same Module 5 clock-skew reason as
    decision #35.

48. **Two corrections to the llama.cpp API notes recorded in #41, both found by
    reading source rather than examples.**

    *Sampling index is `-1`, not `0`.* llama.cpp's `output_resolve_row` treats a
    negative index as "last output row" and a non-negative one as a **batch
    token index**, translated through `output_ids` and throwing
    `batch.logits[%d] != true` if that token was not configured to emit logits.
    A prompt batch sets `logits = true` only on its final token, so the
    published example's `sample(&context, 0)` would abort at runtime for any
    multi-token prompt — which is every real prompt. The resume-point note in
    this file previously repeated that example; it is wrong and is now fixed.

    *Detokenization goes through `token_to_piece_bytes`.* `token_to_str` and its
    `Special` argument are both `#[deprecated]`; the current entry point is
    `token_to_piece`, which requires an `encoding_rs::Decoder` for streaming.
    `llama.rs` collects raw bytes per token and decodes once at the end
    instead. That avoids adding `encoding_rs` as a direct dependency, and is
    strictly more correct for this use: a multi-byte character split across two
    tokens is reassembled by concatenation, whereas per-token decoding emits
    replacement characters. Nothing streams — Sleep-Mode returns a whole answer
    — so there is no reason to decode incrementally. The one subtlety is that
    `token_to_piece_bytes` reports "buffer too small" by returning the required
    length *negated*, so the retry uses that value and is exact.

49. **`LlamaLoader` checks the file exists before calling `load_from_file`.**
    That function opens with `debug_assert!(path.exists())`, so in a debug
    build — which is every test run — a missing GGUF would **abort the process**
    rather than return an error. A missing model is an ordinary condition until
    the file is bundled, so it has to be an error. Guarded by
    `a_missing_model_file_is_an_error_not_a_panic`, which fails loudly if the
    check is ever removed.

50. **Greedy sampling, a 4096-token context, and an over-long prompt is refused
    rather than truncated.** Greedy because §8 wants a faithful synthesis of the
    user's own notes rather than a creative one, and because determinism makes a
    given prompt reproducible when an answer looks wrong. `CONTEXT_TOKENS =
    4096` is sized from what `synthesis.rs` actually asks for — a 2400-token
    context budget plus a 400-token answer plus scaffolding — so **the two
    constants have to move together**; raising the budget alone would silently
    truncate. A prompt that will not fit returns an error instead of letting
    llama.cpp drop the tail, which is the same invisible-degradation failure
    decision #43 chose a token budget to avoid.

51. **Tauri v2's resource API is verified** (this was flagged as an open
    unknown). Bundling is `bundle.resources` in `tauri.conf.json`, which accepts
    files, directories and globs; at runtime Rust resolves them with
    `app.path().resolve("path/to/model.gguf", BaseDirectory::Resource)` (needs
    `tauri::Manager` in scope for `.path()`). That is the answer for **both**
    halves of the bundling problem — the GGUF and decision #29's `fastembed`
    cache directory — so `LlamaLoader::new` and `Embedder::new` both just take
    the resolved path. Neither model file is actually bundled yet; that is
    Phase 6 work in `blurt-app`, which is where an `AppHandle` exists.

52. **The recovery key is stored as its grouped display string, not raw bytes.**
    `repository/secrets.rs` is the new `app_secrets` accessor decision #1 always
    implied but never had. The grouped form is what `MODULE_06_UI_SHELL.md` §D2
    re-displays and what §B4 asks the user to verify two groups of, so storing
    it that way means no formatting on read — and `RecoveryKey::parse` validates
    on the way back, so a corrupted row surfaces as an error rather than a
    silently wrong key. The write is an upsert because rotating the key replaces
    the row, and `app_secrets.key` is a primary key that a plain insert would
    collide with the second time.

53. **`initialize_vault` writes the keyring *before* the database, and refuses
    to run twice.** The order is the whole point and it is not arbitrary.
    Interrupted after the keyring is saved, the next launch finds a vault whose
    passphrase already works and whose database is simply created on first
    unlock — recoverable. The reverse order would leave a database encrypted
    under a master key that was never wrapped into any slot, which is
    unrecoverable by construction, and unrecoverable in the specific way
    `BLUEPRINT.md` §1 says nobody can help with.

    Re-initializing an existing vault is refused outright
    (`AlreadyInitialized`) rather than merged into: a second run generates a
    *new* master key, which would orphan every byte already written under the
    old one. Guarded by
    `initializing_twice_is_refused_rather_than_clobbering_the_vault`.

54. **`CommandError` distinguishes "no vault yet" from "wrong passphrase".**
    New variants `NotInitialized`, `AlreadyInitialized`, `WrongSecret` and
    `Io`, and `SchemaError::WrongSecret` now maps to `CommandError::WrongSecret`
    rather than being flattened into `Schema(String)`. §B6's unlock screen and
    §E's onboarding are different screens, and the frontend has to pick between
    them — doing that by matching on an error *message* would break the moment
    anyone reworded it. `WrongSecret` still carries no detail, keeping
    `SchemaError::WrongSecret`'s "don't hand back an oracle" property.

55. **Migrations run on unlock, not only at creation.** A build that ships a new
    migration has to apply it to the vault that already exists, and unlock is
    the first moment a master key is available to open the file at all. Putting
    it only in `initialize_vault` would mean existing installs silently never
    upgrade their schema.

56. **The sensitive keyslot is created at setup, and nothing checks it yet.**
    §5 describes two deliberately different policies: app-level unlock, which
    may use a grace period, and sensitive-destination access, which requires a
    fresh uncached check *every time* and can never be satisfied by unlock
    state. `initialize_vault` creates the `Sensitive` slot so that gate has
    something to verify against later, but `unlock` deliberately **refuses** the
    sensitive passphrase — guarded by
    `the_sensitive_passphrase_is_a_separate_slot_that_does_not_unlock_the_app`,
    which exists so the two policies cannot quietly collapse into one. The
    passphrase may legitimately be the *same string* as the master one (§5
    leaves that to the user); each slot has its own salt, so reuse is not
    detectable from the keyring file.

    Also still absent, and deliberately: the idle timer that §5 says expires
    app-level unlock. `lock` is the mechanism it will call; owning the timer is
    Module 6's job.

57. **Voice is a normalize-only command; it does not capture.** The roadmap
    sketched `capture_item_via_voice(state, transcript)` as "normalize, then
    delegate to the same path", and that is wrong. §4 puts a **review step**
    between transcription and capture: every standalone "at" is highlighted and
    swapped to `@`, the user dismisses any that were not routing (§4.2), and
    only what survives gets resolved. Normalizing and capturing in one call
    deletes that step.

    It matters because §4 accepts a real false-positive rate to get there —
    "meet Alex at 9pm" *will* be flagged, which the doc calls "a known, accepted
    tradeoff... a one-time per-sentence dismissal cost, judged cheaper than
    silently mis-routing". A combined command produces exactly the silent
    mis-routing that tradeoff was made to avoid, and voice has no picker to
    correct it after the fact.

    So `normalize_voice_transcript` returns the rewritten text plus each
    replacement's **original substring verbatim**, which is what lets a
    dismissal restore capitalisation and spacing exactly. The reviewed text then
    goes to `capture_item_via_router` like any typed input — §4's "voice adds
    zero new parsing logic", honoured literally. A test caught this by failing.

58. **An unresolved `@` chain saves to Unsorted and reports what to create; it
    never creates the destination.** Both of the other options break a stated
    rule. Refusing the capture breaks "capture is never blocked"; silently
    creating breaks §2.5, where the `+` is a **tap in the live picker** that
    happens before submit — so by the time text reaches the command, an
    unresolved segment means the picker did *not* resolve it, and creating
    would turn a typo into a permanent list. The item lands in Unsorted (a real
    destination, per "nothing is ever unrouted") and
    `CaptureOutcomeDto::NeedsDestination` carries the parent, name, trigger and
    remaining segments so the UI can offer creation followed by an ordinary
    move.

    A chain with no body ("@weekly" alone) is refused with `EmptyCapture`
    rather than filed as an empty item. `blurt-router` explicitly leaves that
    call to the caller.

59. **`blurt-rag::search` is split into an async `retrieve_matches` and a public
    synchronous `rank`, because of a constraint that only appears at the IPC
    boundary.** A Tauri async command's future must be `Send`, and
    `rusqlite::Connection` is not `Sync` — so the database guard cannot be alive
    across the `.await` on the vector store. `hybrid_search` holds both at once
    by construction and therefore cannot be called from a command at all.

    Both async commands now await the vector half first (which takes no
    connection), then take the database lock and rank synchronously, and never
    hold one across the other. `hybrid_search` remains as the composition for
    callers that already have a connection, which is every test in that crate.
    Worth remembering as a general shape: an API that takes a `Connection` *and*
    is `async` cannot be used from a Tauri command.

60. **`RagResources` are opened lazily behind a `tokio` mutex and handed out as
    an `Arc`.** Lazily because `VectorStore::open` is `async` while `unlock` is
    not, and making unlock async would have rippled through every vault test for
    no benefit. Behind an `Arc` because both callers need to *stop* holding the
    state lock before they continue — `search` to `.await`, and `ask` to
    `spawn_blocking` — and an `Arc` is what lets the handles outlive the guard.
    The embedder sits behind its own `tokio::sync::Mutex` for the same
    `Send`-across-`.await` reason as #59.

61. **The two model files resolve from bundled resources; the vector store lives
    in the data directory.** Different roots on purpose: the models ship with
    the app and are read-only (`BLUEPRINT.md` §2), while LanceDB is derived data
    that is rebuilt if lost and must be writable.

    Resolving a resource path does **not** require the file to exist, and
    neither model is bundled yet. That is deliberate and load-bearing right now:
    capture and plain-search keep working, and only `ask` fails, as a
    `ModelLoad` error at the moment of loading rather than at startup.

62. **`index_item` is split into `prepare` / `embed_and_store` / `commit`, and
    `commit` re-checks eligibility.** Same constraint as #59: `index_item` is
    async and holds a `Connection` across its await, so it could not run in the
    background at all. `embed_and_store` takes no connection and a compile-time
    guard in its tests keeps its future `Send`. `index_item` survives as the
    composition, for callers that already hold a connection. The re-check in
    `commit` exists because the split puts the whole embedding step between the
    first eligibility read and the write — an item moved into a sensitive
    destination in that window would otherwise be indexed. Its vectors are left
    as orphans, which the write-order notes already treat as harmless.
    `remove_item` has the same async-plus-`Connection` shape and will need the
    same treatment before a command can call it.

63. **One serial worker, 1500 ms quiet period, keyed by item.** §3 says edits
    wait "~1–2 seconds"; 1500 ms is the midpoint and is a single constant,
    `EDIT_QUIET_PERIOD`. A newer edit to the same item replaces the waiting job
    and restarts the wait, which is how "only the settled version gets embedded"
    is enforced. Everything runs through one task, one job at a time: the
    debounce table needs no locks, and the non-idempotent `index_item` is never
    run twice for one version. Captures run immediately and in order. The worker
    is returned as a future rather than spawned, because `setup` must start it
    on Tauri's runtime, where `tokio::spawn` would panic.

64. **Catch-up on unlock, latest version only — chosen by the user
    2026-09-19.** No design doc covers lost jobs. The queue is in memory, so a
    quit, a crash, or an embedding model that fails to load (likely offline,
    while the model is still downloaded on first use) would leave an item
    unsearchable indefinitely. Unlock re-queues every indexable item whose latest
    version has no embeddings (`pending_index_versions`). Older edit versions are
    never backfilled: the debounce skips them on purpose, and from the database
    they are indistinguishable from lost ones. Retry-with-backoff while running
    was offered and declined — on a machine that cannot load the model it would
    retry forever. Lock clears the queue.

65. **A failed background job goes to stderr.** The command that queued it has
    already returned, so there is no caller to report to, and the codebase has no
    logging setup yet. `run_and_announce` is the one place a logger would slot
    in. The failure is not lost for good: the next unlock's catch-up retries it.
    A locked vault is `Skipped`, not an error.

## Environment / build gotchas

- **Rust builds need Strawberry Perl ahead of MSYS Perl on `PATH`**, or
  OpenSSL's `Configure` fails with a missing `Locale/Maketext/Simple.pm`. From
  Git Bash:

  ```bash
  export PATH="/c/Strawberry/perl/bin:/c/Strawberry/c/bin:/c/Users/adith/.cargo/bin:$PATH"
  ```

  PowerShell resolves Strawberry Perl correctly with no change.

- **`lancedb` needs `protoc` on `PATH`.** LanceDB pulls `lance` → `prost-build`,
  whose build script shells out to the Protocol Buffers compiler and fails with
  `Could not find protoc` if it is absent. Installed 2026-09-02 via
  `winget install --id Google.Protobuf --exact`, which puts `protoc.exe` at
  `%LOCALAPPDATA%\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwein`
  and adds it to the user `PATH` — but **only for shells started afterwards**.
  From Git Bash in an already-open session, prepend it explicitly:

  ```bash
  export PATH="/c/Users/adith/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin:$PATH"
  ```

  Setting `PROTOC` to the full binary path works too, and is what CI would want.

- **The `#[ignore]`d model tests must run with `--test-threads=1`.** Run in
  parallel, 7 of the 8 fail: they share one `fastembed` cache directory and
  several `TextEmbedding::try_new` calls racing on it conflict, so whichever
  test gets there first passes and the rest error. Serially all 8 pass in ~5s.
  This is a test-harness artifact, not a product bug — the app constructs one
  `Embedder` and holds it — but the parallel failure looks alarming and
  convincing enough to send a future session debugging the wrong thing:

  ```bash
  cargo test -p blurt-rag -- --ignored --test-threads=1
  ```

  Expect **2 failures** in that run on this machine: the two llama tests
  `panic!` on purpose when `BLURT_TEST_GGUF` is unset, rather than skipping,
  and no GGUF exists here yet. Not a regression. Everything else in the run
  should pass.

- **This repo is not `rustfmt`-formatted; do not run `cargo fmt`.** The code is
  hand-formatted to a wider line budget than rustfmt's default, so
  `cargo fmt -p <crate>` reformats existing untouched files and buries a real
  diff in noise. Match the surrounding style by hand instead. (`cargo fmt --
  --check` is still useful for *reading* what it would object to.)

- **`llama-cpp-2` needs CMake, a C++ toolchain, and libclang.** `llama-cpp-sys-2`
  runs `bindgen` (a non-optional build dependency), which will not run without
  libclang, and builds llama.cpp itself through CMake. CMake 3.29.2 and ninja
  were already present; **LLVM was not** and was installed 2026-09-10 via
  `winget install --id LLVM.LLVM` (22.1.8, to `C:\Program Files\LLVM`). As with
  `protoc`, winget only updates `PATH` for shells started afterwards, and
  bindgen looks for `LIBCLANG_PATH` rather than `PATH` anyway. From Git Bash:

  ```bash
  export LIBCLANG_PATH="C:\\Program Files\\LLVM\\bin"
  ```

  Expect the first build to be long — it compiles llama.cpp from source — and
  every subsequent link of the `blurt-rag` test binary to be noticeably slower
  than before, since llama.cpp is statically linked into it.

- **rust-analyzer's background `cargo check` shares the build lock.** A cargo
  command can sit on `Blocking waiting for file lock on build directory` for
  minutes with no other explanation, because the LSP runs
  `cargo check --workspace` against the same `target/`. Check with
  `Get-CimInstance Win32_Process -Filter "Name='cargo.exe'"` and look at the
  command lines before concluding a build is stuck or killing the wrong thing.
  Two cargo commands of your own will serialize the same way, so chain them
  (`a && b`) rather than backgrounding both.

- **Piping `cargo` into `tail`/`head` hides its exit code.** `cargo build | tail
  -40` reports *tail's* status, so a failed build looks like a success — this
  cost a wasted cycle on the `protoc` failure above, which was reported as
  "exited with code 0". Redirect to a file and check `$?`, or use
  `set -o pipefail`, whenever the exit code matters.

- **First build of `blurt-schema` compiles OpenSSL and SQLCipher from source**
  and takes several minutes. Incremental rebuilds after that are seconds.

- **LSP is set up — `rust-analyzer` 1.97.1 plus the
  `rust-analyzer@claude-code-lsps` plugin (user scope).** Install these from
  the CLI, not the `/plugin` panel, which the model cannot drive:

  ```bash
  claude plugin marketplace add boostvolt/claude-code-lsps
  claude plugin install rust-analyzer@claude-code-lsps
  ```

  Watch out: `~/.cargo/bin/rust-analyzer.exe` exists even when the component
  does not — it is a rustup shim that fails with "Unknown binary
  'rust-analyzer.exe' in official toolchain". Presence on `PATH` proves
  nothing; run `rust-analyzer --version` to actually check.

  `rust-src` is installed and resolves under the sysroot, but its files came
  from a pre-existing vendored source tree rather than rustup, so
  `rustup component add rust-src` may report a conflict on
  `library/.cargo/config.toml`. Harmless unless std completions ever disagree
  with the toolchain version; to make rustup own them, `rustup component
  remove rust-src`, delete `lib/rustlib/src`, then add it back.

- **`vtsls` is not installed.** Add it when Module 6 frontend work starts.

- Toolchain installed: Node 24.19.0, Rust 1.97.1, MSVC 14.44 + Windows 11 SDK,
  Strawberry Perl 5.42.2, NASM 2.16.01. WebView2 runtime was already present.

- **Icons in `src-tauri/icons/` are generated placeholders.** Replace with real
  artwork via `npm run tauri icon` — that also produces the macOS `.icns`,
  which the current set lacks.

- `src-tauri/target/` and `node_modules/` are gitignored and large; a fresh
  clone needs `npm install` plus the multi-minute first Rust build.

- **`cargo test` at the `src-tauri/` root only runs the `blurt` binary
  package's tests (0 of them) — it does NOT test the whole workspace.** Use
  `cargo test --workspace` to actually run `blurt-schema`'s and `blurt-app`'s
  tests. Easy to miss since both commands succeed silently.

- **`tauri::test`'s mock-app harness (`mock_builder`/`mock_context`/
  `noop_assets`) crashes the entire test binary at process startup on this
  dev machine**, `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139), before any test
  code runs — confirmed via a bisected minimal repro (even a bare
  `mock_builder().build(mock_context(noop_assets()))` with no `.manage()`,
  no commands, no window crashes identically). Ruled out: stale incremental
  build artifacts (full `cargo clean` + rebuild, same crash) and Strawberry
  Perl's MinGW toolchain (`c/bin`) polluting `wry`/`tao`/`webview2-com-sys`'s
  build scripts (rebuilt with Perl-only, no MinGW, on `PATH`, same crash).
  Root cause not identified — a real WebView2 Runtime is installed
  (151.0.4129.101) and `dumpbin /dependents` shows nothing unusual, so this
  looks like a genuine Tauri-2.11.5/Windows environment incompatibility, not
  a fixable code or PATH issue. **Workaround: don't use `tauri::test` at all.**
  Every `blurt-app` command is tested via its plain `_impl(&AppState, ...)`
  function instead (decision #13 above) — zero `tauri::test` dependency,
  fully reliable. The actual packaged app (`npm run tauri dev` /
  `target/debug/blurt.exe`) launches fine; this is specific to the `cargo
  test` mock-runtime path.

---

## Session log

Newest first. One short entry per session — what changed, not how.

### 2026-09-19 (session 6)
- **Background indexing landed; search now returns results on a real vault.**
  Until today nothing was ever embedded. Planned first (plan file linked in
  "Resume here"), then executed task by task, TDD throughout with Red confirmed
  against `todo!()` before every implementation.
- `blurt-schema`: `pending_index_versions`. `blurt-rag`: `index_item` split into
  `prepare`/`embed_and_store`/`commit` (#62). `blurt-app`: `indexer.rs` — the
  scheduler (#63), `run_job`, the `item-indexed` event — plus hooks in both
  capture commands, `append_edit`, unlock (catch-up, #64) and lock.
- **Asked rather than assumed** on the catch-up pass, which no doc specifies.
- **384 tests green** (77 + 128 + 65 + 114), 11 `#[ignore]`d. The new ignored
  end-to-end test passes against the real embedding model, as do the three
  `index_item` model tests through the recomposed path. Clippy clean on
  everything touched; the two known `blurt-schema` keyring warnings remain.
  `npm run tauri dev` opens and stays up with the worker running.
- Raised with the user, unanswered: `AppState.rag` survives a lock (see
  "Resume here").

### 2026-09-15 (session 5, continued x2)
- **The whole IPC surface landed.** `commands/router.rs` (capture + voice
  normalization) and `commands/search.rs` (`classify_input`, `search`, `ask`),
  plus the `AppState` extension holding Module 4's handles. Every domain crate
  is now reachable from the frontend.
- **Two designs changed because the docs said so, not because the code
  complained.** Reading §4 showed voice needs a review step, so
  `capture_item_via_voice` became `normalize_voice_transcript` (decision #57);
  reading §2.5 showed the `+` create is a picker tap, so an unresolved chain
  saves to Unsorted and reports rather than creating (decision #58).
- **One design changed because the compiler would have said so**: a Tauri async
  command's future must be `Send` and `rusqlite::Connection` is not `Sync`, so
  `hybrid_search` — async *and* taking a connection — cannot be called from a
  command at all. Split into `retrieve_matches` + public `rank` (decision #59).
- **360 tests green** (109 + 65 + 124 + 62), 10 `#[ignore]`d.
- **Known and important:** nothing indexes yet, so search returns nothing on a
  real vault. The debounce task is the next thing to build.

### 2026-09-15 (session 5, continued)
- **Phase 6 started: the vault lifecycle.** `blurt-app` gained
  `commands/vault.rs` (`initialize_vault`/`unlock`/`lock`/`is_unlocked`) and
  `blurt-schema` gained `repository/secrets.rs` for the `app_secrets` recovery
  key that decision #1 always implied. TDD throughout, Red confirmed against
  `todo!()` first. **The app can now create and open its own database** — until
  now there was no way to do either.
- Read `MODULE_02_SCHEMA.md` §3/§5 and `MODULE_06_UI_SHELL.md` §B4/§B6/§E before
  writing, which is where the three-slot setup, the grouped recovery-key format
  and the onboarding order came from rather than being invented.
- Decisions #52-#56. The load-bearing one is **#53**: the keyring is saved
  before the database, because the reverse order can produce a database
  encrypted under a master key wrapped nowhere — unrecoverable by construction.
- **338 tests green** (109 + 65 + 123 + 41), 10 `#[ignore]`d, clippy clean on
  `blurt-app`. The two `blurt-schema` keyring warnings remain untouched.

### 2026-09-15 (session 5)
- **Phase 5 is complete.** Added `llama.rs`: `LlamaLoader`/`LlamaGenerator`
  behind the existing `ModelLoader`/`TextGenerator` traits, so `ModelManager`
  drives a real GGUF unchanged. TDD, Red confirmed against `todo!()`.
- **Reading llama.cpp's C++ source caught two runtime bugs before they
  happened** (decision #48). The sampling index must be `-1`; the published
  example's `0` resolves to a batch token with no logits and aborts. And
  `token_to_str`/`Special` are deprecated, so detokenization now accumulates
  raw bytes and decodes once, which also avoids an `encoding_rs` dependency and
  handles characters split across tokens. The resume-point note in this file
  had repeated the wrong example; it is corrected.
- Also guarded a `debug_assert!` trap: `load_from_file` aborts a debug build on
  a missing path, so `LlamaLoader` checks first (decision #49).
- **Verified Tauri v2's resource API** (decision #51), closing the last open
  unknown in Phase 5. Neither model file is bundled yet — that is Phase 6.
- **322 tests green** (104 + 65 + 123 + 30), 10 `#[ignore]`d, and `cargo clippy
  -p blurt-rag` is clean. Clippy caught a manual loop counter in the generation
  loop, now a range. The two long-standing `blurt-schema` keyring warnings are
  still there and still untouched.
- The two `#[ignore]`d llama tests need a real GGUF via `BLURT_TEST_GGUF`; they
  have never been run, so real inference is **unproven** until Phase 6 bundles a
  model.

### 2026-09-10 (session 4, continued x2)
- Cleared the roadmap's **highest-risk item**: `llama-cpp-2` is added, pinned at
  `=0.1.156`, and **compiles on this machine**. Required installing LLVM for
  libclang (`llama-cpp-sys-2` runs bindgen) — see Environment.
- Read its API from the vendored source rather than docs.rs, which caught two
  differences that shape the implementation (decision #41): `LlamaBackend::init`
  is a process-wide singleton, and `LlamaSampler::sample` takes `&mut self`
  where the published example shows it on an immutable binding. **Nothing
  references the crate yet** — the loader impl is the next task.
- Built **`model_manager.rs`** (§3's load/unload service, decision #42) and
  **`synthesis.rs`** (grounded prompt, §8 answer shape, decisions #43-#47),
  TDD throughout with Red confirmed against `todo!()` first.
- **Asked rather than assumed** on §11's deferred top-N cap; the user chose a
  token budget over a fixed count, which is also the only option that cannot be
  silently truncated by llama.cpp.
- Deviated from the roadmap once, deliberately: no all-in-one `answer_question`
  (decision #44). Retrieval is async and generation is a blocking CPU burn, so
  the convenience would have been a footgun.
- Decisions #41-#47 recorded; two environment notes added.
- **317 tests green** (104 + 65 + 118 + 30), 8 `#[ignore]`d, verified in a full
  `cargo test --workspace` run on 2026-09-11 after the previous session's run
  was killed mid-build. `cargo clippy` was **not** rerun this round: adding
  `llama-cpp-2` shifted shared dependencies and a clippy pass would mean a
  second full rebuild of the LanceDB/DataFusion stack. Run it first next session.

### 2026-09-10 (session 4, continued)
- Started Phase 5: **`classify.rs`**, §9's statement-vs-question split. Pure
  local heuristics, no model — trailing `?`, interrogative opening, fronted
  auxiliary. TDD, Red confirmed against a `todo!()` first. 13 tests.
- **Two bugs, both real, both caught by tests rather than reasoning.** The
  contraction handling was wrong for "didn't"/"isn't" (decision #39) — the
  apostrophe split leaves `didn`, not `did`, and my doc comment had confidently
  claimed otherwise. And fixing it surfaced **decision #40**: `keywords.rs` has
  been nondeterministic since Phase 3 in a way decision #23 did not cover, with
  `extraction_is_deterministic` flaky all along because it compared `f64`s
  bit-for-bit. Fixed at the source (`ordering_score` rounds before comparing)
  rather than by loosening the assertion, then confirmed stable over five
  consecutive full runs.
- **298 tests green** (104 + 65 + 99 + 30), 8 `#[ignore]`d, `cargo clippy -p
  blurt-rag` clean.
- Still ahead in Phase 5: `model_manager.rs` and `synthesis.rs`. The
  `llama-cpp-2` verification gate has **not** been done yet — do it first.

### 2026-09-10 (session 4)
- Executed Phase 4: **Module 4's hybrid retrieval**. New `blurt-rag/src/search.rs`
  covers all of §4–§7 — `query_terms`, `RetrievalWindow`, `rank`, and the async
  `hybrid_search`. TDD throughout, every pass Red-confirmed against a `todo!()`
  before implementing. **283 tests green** (104 + 65 + 84 + 30), and the 8
  `#[ignore]`d model tests were run and confirmed green too, which had not been
  done before. `cargo clippy -p blurt-rag` clean.
- Set the vector store's distance metric to **cosine** (decision #31). Phase 4
  is the first code to read a distance as a similarity, and LanceDB's default
  squared L2 would have made an orthogonal pair score `-1`. The test was written
  first and failed with `got 2`, which is exactly the bug it was written to
  catch.
- **The first scoring formula was wrong and a test caught it** (decision #33): a
  linear keyword boost saturating at three hits gave a single exact-phrase match
  too little weight to close even a small semantic gap — leaving §4's
  "exact-term lookup" case unserved for the most common query shape. Replaced
  with a diminishing-returns curve where the first hit earns half the boost.
  Fixed the formula rather than the assertion; the test was encoding the intent
  correctly.
- Split ranking policy (`rank`) from the model-dependent shell
  (`hybrid_search`) so 21 of 24 new tests need no embedding model and run in
  milliseconds — the same testability-driven split as decision #13.
- Decisions #31-#38 recorded above; two new environment gotchas (the
  `--test-threads=1` requirement for the ignored tests, and "don't run
  `cargo fmt`").
- Left alone deliberately: two pre-existing `clippy` warnings in
  `blurt-schema/src/keyring.rs` (`&Vec` vs `&[_]`, `is_multiple_of`). They come
  from newer lints, not from this work, and are unrelated to Module 4.

### 2026-09-02 (session 3, continued)
- Executed Phase 3: **Module 4's indexing pipeline**. `blurt-rag` gained
  `chunking`, `embedding`, `keywords`, `vectorstore` and `indexing`;
  `blurt-schema` gained migration `0002`, `is_item_indexable`,
  `store_index_results`, `edits::get_by_id`, and the `embeddings`/`keywords`
  repositories. TDD throughout. **247 tests green** (104 + 65 + 48 + 30), 6
  `#[ignore]`d model-loading tests, `cargo clippy` clean on both crates.
- Verified `fastembed`, `lancedb` and `yake-rust` against docs.rs/crates.io
  before writing against them, per the roadmap's gate. That caught the
  `TextInitOptions` rename and, later, the `lancedb` 0.38.0 regression
  (decision #28).
- Two real problems found and fixed rather than papered over: YAKE's
  cross-process tag instability (decision #23) and the `lancedb` pin. Installed
  `protoc`, which LanceDB's build needs — see Environment.
- Decisions #21-#30 recorded above.

### 2026-09-02
- Executed Phase 2 of the roadmap: **Module 3 is complete**. Built
  `blurt-router` end to end, TDD throughout (`todo!()` stub + tests written and
  confirmed Red before each implementation): `chain`, `candidates`, `nl`,
  `voice`, `resolve`. Added the three things `blurt-schema` was missing for it
  — `destinations::list_children`, `destinations::list_all`, and the
  `UNSORTED_ID`/`RANDOM_THOUGHTS_ID` seed constants.
- Settled six details §2–§4 leave to the implementation (decisions #14-#20
  above), the load-bearing ones being the word-boundary rule for a
  chain-opening `@` and the two voice-normalization rules without which §4.4's
  multi-segment spoken chains could not work at all.
- **167 tests green** (72 + 65 new + 30), workspace `cargo build` clean,
  `cargo clippy -p blurt-router` clean. `blurt-router` is finished but not yet
  reachable over IPC — that wiring is Phase 6, after Module 4.

### 2026-08-20
- Scaffolded the repo per `MODULE_01_ARCHITECTURE.md` §2: workspace, five
  crates, Tauri config, React/TS frontend, all six nested `CLAUDE.md` pointer
  files (verified byte-identical to §4).
- Installed the full toolchain; verified `cargo build` clean and
  `npm run tauri dev` opens a blank window.
- `blurt-schema`: keyring + recovery implemented TDD, 28 tests green. DDL for
  all §2 tables written. `db.rs`/`migrations.rs` tests written and Red.
- `git init`, first commit, pushed to the public GitHub remote. Added
  `.gitattributes` to normalize line endings across platforms.
- Finished `db.rs` + `migrations.rs`. **46 tests green**, `cargo build` clean,
  `npm run tauri dev` still opens a window. Module 2 is complete except for the
  repository/CRUD layer.

### 2026-08-24 (session 2)
- Planned the roadmap through Module 3 and Module 4 (LLM-free + Sleep-Mode):
  saved at `C:\Users\adith\.claude\plans\dynamic-gliding-wolf.md`. Locked
  decisions during planning: Sleep-Mode's generative model is
  Qwen2.5-3B-Instruct (GGUF, Q4_K_M, Apache 2.0) via a Rust llama.cpp binding
  (`llama-cpp-2`), not an Ollama sidecar; embedding model is
  bge-small-en-v1.5 via `fastembed`.
- Executed Phase 1: wired `blurt-app` commands over `blurt-schema`'s
  repository layer — `AppState`, `CommandError`, DTOs, and a representative
  destinations/items/edits command slice, TDD throughout. Skipped
  `tauri-specta` (decision #12) after checking crates.io mid-session. Hit
  and worked around the `tauri::test` mock-harness crash (decision #13,
  environment gotcha above) by splitting every command into a testable
  `_impl` plus a thin wrapper. **97 tests green** (67 + 30 new), workspace
  `cargo build` clean, `npm run tauri dev` launches successfully with all 12
  commands registered.

### 2026-08-24 (session 1)
- Built the `blurt-schema` repository/CRUD layer, TDD throughout (Red
  confirmed via `todo!()` stubs before each implementation): `destinations`,
  `items`, `edits`, then `indexing::items_for_indexing` (the sensitive-safe
  accessor, tested directly against a seeded sensitive destination). **67
  tests green**, workspace `cargo build` clean. Module 2 is now fully
  complete — see decision #11 above for the `repository/` layout.
