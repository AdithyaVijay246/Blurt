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

Last updated: 2026-09-02

A roadmap through the rest of Module 3 (router) and Module 4 (embeddings/RAG)
is saved at `C:\Users\adith\.claude\plans\dynamic-gliding-wolf.md` — Phases 1-3
below are done; Phases 4-6 (retrieval, Sleep-Mode, final wiring) are still
ahead. Read that plan file at the start of the next session
rather than re-deriving the sequencing here.

---

## Resume here

**Next action:** Phase 4 of the roadmap — Module 4's hybrid retrieval, in a new
`blurt-rag/src/search.rs`. `hybrid_search` combines the vector side
(`vectorstore::search`) with the keyword side
(`blurt_schema::repository::keywords::items_matching_any`, which already
excludes sensitive and tombstoned items structurally), dedupes by `itemId`,
and sorts by pure relevance with **no recency re-weighting** — §4 is explicit
that time-scoping at the window level already handles recency. Then `@`-scoping
as a post-query re-sort into in-scope/elsewhere groups (§6: soft
prioritization, never a `WHERE` filter), the `RetrievalWindow` filter on
`editedAt`, and path/timestamp attachment per §7.

Everything Phase 4 needs is in place: `embeddings::get_by_vector_ref` turns a
LanceDB hit into a displayable row, and `destinations::path` computes the
display path.

**Just finished:** Phase 3 — Module 4's LLM-free indexing pipeline. `blurt-rag`
now runs a capture or edit end to end into chunks, vectors and tags:

- **`chunking.rs`** — 180-word chunks with 30 words of overlap (~17%, inside
  §3's 15-20% band), each carrying the character range §7's jump-to-chunk
  needs. Word budget is deliberately *below* §3's 256-token limit; see decision
  #21.
- **`embedding.rs`** — `fastembed`/bge-small-en-v1.5, 384 dims, model loaded
  lazily on first real use.
- **`keywords.rs`** — YAKE via `yake-rust`, made deterministic across processes
  (decision #23 — this was a real bug).
- **`vectorstore.rs`** — LanceDB wrapper: open/create, add, nearest-neighbour
  search, delete-by-item.
- **`indexing.rs`** — `index_item` / `remove_item`, LanceDB first then one
  SQLCipher transaction.
- **`blurt-schema`** — migration `0002` (nullable `embeddings.editId`),
  `is_item_indexable`, `store_index_results`, `edits::get_by_id`, and two new
  repositories: `repository/embeddings.rs` and `repository/keywords.rs`.

**Known gap, deliberately left for Phase 6:** marking an *existing* destination
sensitive stops future indexing but does not retract what was already indexed.
§3 says sensitive content is invisible to the pipeline, so that transition has
to call `blurt_rag::indexing::remove_item` for its items. It is orchestration
and belongs with the command that flips the flag, which does not exist yet.
Noted in `indexing.rs`'s module docs too.

## Status by component

| Component | State | Notes |
|---|---|---|
| Repo scaffold | **Done, verified** | Workspace builds clean; `npm run tauri dev` opens a blank window |
| Frontend skeleton (`src/`) | **Stub only** | Blank `App.tsx`; `npm run build` passes. No Module 6 work started |
| `blurt-schema` — keyring | **Done, green** | Key-wrapping, 3 slot kinds, recovery-key encoding |
| `blurt-schema` — DDL | **Done, green** | `migrations/0001_initial.sql`, all §2 tables + indexes + seeds |
| `blurt-schema` — db/migrations | **Done, green** | SQLCipher raw-key open + probe; `user_version` runner |
| `blurt-schema` — repository/CRUD | **Done, green** | `repository/{destinations,items,edits,embeddings,keywords,indexing}.rs`; `list_children`/`list_all` + seed ids for M3, `is_item_indexable`/`store_index_results`/`edits::get_by_id` for M4 |
| `blurt-router` (M3) | **Done, green** | `chain`/`candidates`/`nl`/`voice`/`resolve` — all of `MODULE_03_ROUTER.md`. Decides only; never writes |
| `blurt-rag` (M4) | **Partial, green** | Indexing pipeline done: `chunking`/`embedding`/`keywords`/`vectorstore`/`indexing`. Retrieval (`search.rs`) and Sleep-Mode are Phases 4-5 |
| `blurt-sync` (M5) | **Empty stub** | Will add its own migration for `yrs` update logs + paired devices |
| `blurt-app` | **Partial, green** | Representative command slice over destinations/items/edits (§1 CRUD). No unlock command, and no router commands yet — `blurt-router` is finished but not yet reachable over IPC (Phase 6) |

**247 tests green** across the workspace as of the last commit (104
`blurt-schema` + 65 `blurt-router` + 48 `blurt-rag` + 30 `blurt-app`), plus 6
`#[ignore]`d. Test command (note the PATH requirements under Environment
below):

```bash
cargo test --workspace
cargo test -p blurt-rag -- --ignored   # loads the real ~100MB embedding model
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
