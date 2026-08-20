# Module 2 — SQLCipher Storage Schema & Local E2EE

Status: **Design finalized.** Not yet implemented.

## 1. Design Principles

- Every destination (list or note) and every item gets a **stable UUID**
  at creation that never changes, even across renames or reparenting.
- **No path strings are ever stored.** A blurt references a destination by
  ID only. Human-readable paths (e.g. "Shopping > Weekly > Produce") are
  computed on read by walking the `parentId` chain. This makes drag-and-drop
  reorganization a single cheap field update, not a mass rewrite — which
  also keeps CRDT sync diffs small and merge-safe.
- Edits are **append-only**, never destructive. An item's current text is
  denormalized onto the item row for fast reads, but the full history lives
  in a separate `edits` table.
- Deletions are **tombstones** (`deletedAt` marker), never hard deletes —
  required for correct CRDT merge behavior (so a delete on one device isn't
  silently "revived" by a concurrent edit on another).
- "Unsorted" and "Random Thoughts" are **not special structures** — they're
  just rows in `destinations` (`isSystem = true` for Unsorted, ordinary
  user-created for Random Thoughts). Resolving an Unsorted item is just a
  `destinationId` update, identical to any other move.

## 2. Schema

### `destinations`
Lists and notes are the same table; `type` distinguishes rendering/behavior.

| Column | Type | Notes |
|---|---|---|
| `id` | UUID | Stable, primary key |
| `parentId` | UUID, nullable | FK → `destinations.id`. Null = top-level |
| `name` | text | Display name, editable |
| `trigger` | text, unique | User-defined at creation, editable |
| `type` | enum(`list`, `note`) | Lists support comma-split items + checkable state; notes are append-only prose logs |
| `isSystem` | bool | True for Unsorted (and any future system destinations). Hidden from the normal `@` picker |
| `isSensitive` | bool | See §4. Excludes destination's items from embedding/keyword indexing |
| `sortOrder` | int | Drag-and-drop ordering |
| `createdAt` | timestamp | |
| `deletedAt` | timestamp, nullable | Tombstone |

### `items`
List items and note entries — same shape.

| Column | Type | Notes |
|---|---|---|
| `id` | UUID | Stable, primary key |
| `destinationId` | UUID | FK → `destinations.id`. Reparenting, moving out of Unsorted, and dragging between lists are all just writes to this field |
| `originalText` | text | Immutable — the raw capture, set once |
| `currentText` | text | Denormalized latest version, for fast reads without joining `edits` |
| `checked` | bool, nullable | Only meaningful for `list`-type destinations; unused (`null`) for notes and Unsorted items |
| `createdAt` | timestamp | |
| `deletedAt` | timestamp, nullable | Tombstone |

### `edits`
Append-only edit history, one row per edit.

| Column | Type | Notes |
|---|---|---|
| `id` | UUID | |
| `itemId` | UUID | FK → `items.id` |
| `text` | text | The new text at this point in history |
| `editedAt` | timestamp | |

Timeline display convention: show `originalText` → latest edit only
(e.g. *"buy milk"* then italic *"edited to oat milk"*); full intermediate
chain is available in the item's detail view, not the main timeline.

### `embeddings`
Join layer between SQLCipher and the LanceDB vector store. **Every version**
of an item's text gets its own embedding (original + each edit), so
searching for either the old or new wording finds the item — this is a
deliberate product decision (see Module 3 discussion): Blurt's recall model
is closer to a mind map of everything ever typed than a "current state
only" search index.

| Column | Type | Notes |
|---|---|---|
| `id` | UUID | |
| `itemId` | UUID | FK → `items.id` |
| `vectorRef` | text/key | Pointer into LanceDB |
| `chunkIndex` | int | For long notes split into multiple ~256-token chunks (embedding model has a hard input limit) |
| `chunkStartOffset` | int | Character offset within the item's full text where this chunk begins |
| `chunkEndOffset` | int | Character offset where this chunk ends |

`chunkStartOffset`/`chunkEndOffset` exist specifically to support
**jump-to-chunk-with-highlight** navigation (see `MODULE_04_EMBEDDINGS_RAG.md`
§3): knowing *which* chunk matched isn't enough to scroll to and highlight
the right span in a long note — the exact character range is needed too.
Chunks are generated with ~15–20% overlap between consecutive chunks
(standard RAG practice, avoids losing meaning when a sentence falls on a
chunk boundary), so offset ranges will overlap slightly by design — this is
expected, not a bug.

Search results are **deduplicated by `itemId`** at the display layer — a
3-times-edited or multi-chunk item should surface as one result, not
several.

### `keywords`
Cheap, local keyword/keyphrase extraction (RAKE/YAKE/TF-IDF — no LLM) for
fast exact-match filtering, complementing (not replacing) semantic search.

| Column | Type | Notes |
|---|---|---|
| `itemId` | UUID | FK → `items.id` |
| `keyword` | text | Indexed |

## 3. Encryption Model

- **Whole-database encryption.** SQLCipher encrypts the entire local DB
  file with a single AES-256 **master key** — no per-field or per-table
  encryption for v1, revisit only if proven insufficient.
- **Key-wrapping, not direct derivation** (amended during Module 6 design
  work to support passphrase recovery, §5). The master key is generated
  **randomly** at setup, not derived directly from the passphrase. It's
  never stored in plaintext — only in **wrapped (encrypted) form, across
  one or more independent slots**, any of which can unwrap it:
  - **Passphrase slot:** the master passphrase, run through **Argon2id**
    (chosen for GPU/brute-force resistance), derives a wrapping key that
    encrypts the master key. This is the day-to-day unlock path.
  - **Recovery-key slot:** a separate, randomly-generated recovery key
    (shown once at onboarding — see `MODULE_06_UI_SHELL.md` §B)
    independently wraps the same master key. Used only for recovery
    (§5), never for routine unlock.
  - This is the same slot-based pattern used by disk-encryption systems
    with multiple unlock methods (e.g. LUKS keyslots). Adding, replacing,
    or revoking an unlock method (e.g. a cross-device passphrase reset,
    §5) only touches that slot's small wrapped-key blob — it never
    requires re-encrypting the database itself.
- Neither the passphrase nor the recovery key is ever stored — only their
  wrapped-key outputs, and the unwrapped master key transiently in memory
  while the app is unlocked.
- Primary keys are UUIDs — no special handling needed under whole-DB
  encryption.

## 4. Sensitive Destinations

Some destinations (e.g. a "Passwords" list) need behavior beyond what
at-rest encryption alone provides, since at-rest protection doesn't cover
the "app is unlocked and someone else has the device" scenario.

- `isSensitive` flag on `destinations`.
- If true:
  - Excluded from embedding generation and keyword extraction entirely —
    sensitive content is never indexed, never semantically searchable, and
    never eligible to be retrieved by the RAG pipeline.
  - Content is masked/hidden in timeline and search views. Per
    `MODULE_06_UI_SHELL.md` §B, a sensitive destination shows only its
    name and a lock icon everywhere — no content preview at all, not even
    a masked/blurred one.
  - Viewing requires a **fresh, uncached** biometric or passphrase check
    every time (see §5) — never satisfied by an already-unlocked app
    session.
- Users can mark their own destinations sensitive at any time
  (`MODULE_06_UI_SHELL.md` §D2 — Settings). No destination is
  sensitive-by-default; see §6 for the resolved premade-destinations
  question.

### Sensitive-query triage (for Sleep-Mode AI)

Since sensitive content is excluded from the index, a query like *"what's
my Gmail password"* can't be answered via normal RAG. Instead:

1. Embed the incoming query (same lightweight embedding model already used
   for every blurt — no LLM load required for this step).
2. Compare against small reference/prototype vectors built from sensitive
   destination names and common phrasings (e.g. "Gmail," "email login,"
   "email password," "google account").
3. If similarity clears a threshold → route to **direct DB lookup**,
   bypassing RAG and the LLM entirely. The secret value never enters any
   model's context, local or cloud.
4. Trigger the fresh biometric/passphrase gate before revealing content.
5. If similarity does *not* clear the threshold → falls through to normal
   RAG, which simply won't find anything (safe failure mode — content
   isn't indexed).

**Threshold is tuned to over-trigger, not under-trigger.** A false-positive
biometric prompt costs ~half a second (Face ID/fingerprint are near-instant);
a false negative risks a real secret being pulled into an LLM's context.
That asymmetry means "annoying but safe" always beats "smooth but risky."

## 5. Auth & Locking Model

Two distinct policies, deliberately different in strictness:

### App-level unlock (general access)
- Stays unlocked continuously while the app is running/foregrounded — no
  re-prompt on every switch back to the app.
- Locks on: full app close/force-quit, device reboot, **or** an idle timer
  expiring — whichever comes first.
- Idle timer is user-configurable (sensible default TBD, e.g. 5 minutes;
  can presumably be set to "never" or "immediately" for either extreme).
  Configured in Settings (`MODULE_06_UI_SHELL.md` §D2).
- Rationale: this app's core value is fast, zero-friction capture — strict
  re-auth on every foreground event would undermine that for low-stakes,
  everyday use (checking a shopping list, glancing at the timeline).
- UI: see `MODULE_06_UI_SHELL.md` §B for the actual unlock screen (native
  biometric prompt on mobile, full-screen passphrase entry on desktop).

### Sensitive-destination access
- **Zero grace period, always.** Every single access to a sensitive
  destination demands a fresh, uncached biometric or passphrase check —
  regardless of how recently the app itself was unlocked, even if that was
  seconds ago.
- Never satisfied by a cached OS-keychain key, cached session state, or a
  recent app-level auth event.
- Rationale: protects specifically against the "handed an unlocked phone
  to someone" scenario, which app-level unlock alone does not cover.
- **Exception:** syncing a sensitive destination's encrypted data between
  the user's own paired devices does *not* require this fresh check —
  only viewing it does. See `MODULE_05_CRDT_SYNC.md` §5 for the full
  rationale.

### Passphrase setup
- Master passphrase: set during onboarding (first step,
  `MODULE_06_UI_SHELL.md` §E), wraps the master key (§3). Day-to-day app
  unlock uses biometric where available, this passphrase as fallback.
- Sensitive-info passphrase: **user's choice** at setup — reuse the master
  passphrase, or set a distinct one. Either is valid; the security
  property comes from *forcing fresh re-entry every time*, not from the
  string being different. Users wanting defense-in-depth can choose a
  separate one.
- **Non-biometric platforms** (notably desktop, since Tauri targets
  machines without Face ID/Windows Hello/fingerprint hardware): the
  sensitive-info passphrase is the only gate, always prompted — no
  biometric shortcut exists to fall back from.

### Passphrase recovery — **resolved**
Two independent recovery paths, neither of which compromises the
zero-knowledge property (nothing is ever recoverable server-side, or by
Blurt itself):

1. **Cross-device reset.** If another of the user's own paired devices
   still holds the master key unwrapped (i.e. it's currently unlocked),
   that device can create a brand-new passphrase slot for the locked-out
   device without ever needing the old passphrase — it already has the
   master key, it's just wrapping it under a new passphrase. Covers the
   common case: locked out of one device, not all of them.
2. **Recovery key.** A randomly-generated key, independent of the
   passphrase, shown once during onboarding and never stored by the app.
   Wraps the master key in its own slot (§3), so it can unlock the data
   even if every device's passphrase and cached credentials are gone.
   Last-resort path, if every device is lost/reset simultaneously.
3. If **both** paths are unavailable (recovery key never saved, and no
   other device is reachable), the data is genuinely, permanently
   unrecoverable — the irreducible tradeoff of real zero-knowledge
   encryption (`BLUEPRINT.md` §1), not an oversight.

Full UI flow (onboarding recovery-key screen + verification, "forgot
passphrase" entry points, cross-device reset via the existing pairing
mechanism): see `MODULE_06_UI_SHELL.md` §B.

## 6. Open Questions / Deferred

- **Which destinations ship premade — resolved.** Only "Random Thoughts"
  ships premade (ordinary, not `isSensitive`), created before the user
  does anything else during onboarding — chosen because Module 3 treats
  it as a permanent, always-available fixture rather than something a
  user would think to create themselves. Nothing else ships premade or
  sensitive-by-default; any other starter destinations are user-initiated
  during onboarding's optional destination-creation step (tappable
  suggestions, not auto-created). See `MODULE_06_UI_SHELL.md` §E.
- Exact idle-timer default for app-level unlock.
- Index/query pattern optimization (e.g. explicit indexes for "all items
  in a destination," "all Unsorted items," "edit history for an item") —
  deferred until implementation.
