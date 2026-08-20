# Module 5 — yrs (CRDT) + mDNS Local P2P Sync Engine

Status: **Design finalized.** Not yet implemented.

## 1. Core Model

- **CRDT library: `yrs`** (the Rust port of Yjs), used natively from the
  `blurt-sync` crate — no bridge, no JS runtime involved, consistent with
  Module 1's Rust-only backend decision. Chosen over Automerge's Rust
  crate (`automerge-rs`) for its more mature ecosystem, a built-in
  awareness-protocol equivalent, and because `yrs` stays wire-compatible
  with the original Yjs binary encoding — keeping the door open to a
  JS/WASM client (e.g. a future web companion) later without a format
  migration. `Map`/`Array` types map cleanly onto the destinations/items
  hierarchy. (This section originally framed the choice as "Yjs," written
  before Module 1 locked in a Rust-only backend — the underlying data
  model and update format are unchanged, only the runtime/language.)
- **Transport for v1: mDNS only** (same local network). BLE (same-room,
  no shared WiFi) was evaluated and explicitly deferred — see §9.
- **Sync trigger: one-shot on app open.** The app discovers peers, exchanges
  deltas once, then stops — not a continuous background sync while
  foregrounded. Predictable battery cost; matches `BLUEPRINT.md`'s original
  "on-app-open" framing.

## 2. Document Model

**One `yrs` document per destination**, not one document for the whole
database. Every list and note gets its own CRDT doc. This keeps merges,
sync payloads, and compaction (§4) scoped to what actually changed —
syncing a destination nobody has touched in months costs nothing, and a
merge conflict in one list can never touch another.

## 3. Field-Level Merge Semantics

This module uses **last-write-wins (LWW) by timestamp as the uniform
default** across every conflict type below. This is a deliberate,
project-wide choice, not a per-field improvisation: it keeps the
implementation predictable, and it's consistent with the cross-module
"zero-friction over correctness" principle (`BLUEPRINT.md` §4) — the most
recent action is treated as the best available signal of current user
intent.

- **Edits (`edits` table):** already conflict-free by construction —
  Module 2's append-only design means two devices' edits simply interleave
  as more rows, no merge logic needed. `yrs` just replicates the appends.
- **`currentText` / `checked` (denormalized "current state" fields):**
  LWW by timestamp. Whichever device wrote most recently wins the
  displayed value; the full edit history remains intact in `edits`
  regardless.
- **`destinationId` (reparenting):** LWW by timestamp. If the same item is
  moved to two different destinations on two offline devices, the most
  recent move wins.
- **`sortOrder` (drag-and-drop reordering):** LWW by timestamp, applied
  per item's individual `sortOrder` value — not a full ordered-list CRDT.
  **Known tradeoff:** if both devices reorder *different* items in the
  same list while offline, the merged order won't necessarily match either
  device's full intended sequence — each item's position is independently
  resolved to whichever device wrote it last. Accepted for v1 because
  reordering is low-stakes and reversible, unlike content. A proper
  ordered-list CRDT (fractional indexing, or `yrs`'s native array
  ordering) is the fix if this proves annoying in practice — see §10.
- **`deletedAt` (tombstones) vs. concurrent edits:** also resolved via LWW
  — `deletedAt` is treated as just another timestamped register, compared
  against the timestamp of the most recent edit to the same item. If the
  delete is more recent, the item stays deleted. If a concurrent edit is
  *more* recent than the delete, the edit wins and the item is effectively
  un-deleted. **This is a deliberate refinement of `MODULE_02_SCHEMA.md`'s
  original tombstone rationale**, which frames tombstones as preventing a
  delete from ever being "silently revived" by a concurrent edit. Module 5
  supersedes that with the same recency-wins philosophy used everywhere
  else in this doc, on the reasoning that the most recent action is still
  the best signal of what the user actually wants right now.
- **Known limitation — clock skew:** LWW resolution relies on each
  device's system clock. Skew between devices could in rare cases cause an
  actually-older change to be treated as "more recent." Not addressed with
  a hybrid logical clock or similar for v1 — flagged as a known limitation
  with low practical impact, since one-shot sync-on-open makes prolonged
  concurrent-edit windows between devices uncommon.

## 4. CRDT Operation-Log Compaction

Flagged in `BLUEPRINT.md` §5 as a hard requirement. Two distinct concerns,
both handled by `yrs`'s existing mechanisms rather than custom design:

- **Update-log growth:** every edit produces a small binary diff; storing
  every diff forever grows storage unboundedly even though the underlying
  data isn't growing that fast. Fix: after a successful sync round
  completes for a given destination, collapse its stored update sequence
  into a single state-vector snapshot (`encode_state_as_update` /
  equivalent) and discard the individual diffs. This snapshot is exactly
  as valid for diffing against an arbitrarily stale peer's state vector as
  the original increments were — compaction never breaks correctness for
  a peer that's been offline a long time.
- **Fallback trigger (sync-independent):** for a destination that rarely
  or never syncs (e.g. single-device usage), compact on a size/age
  threshold too — proposed defaults: log exceeds ~200–500KB, or hasn't
  been compacted in 30 days. Exact numbers are provisional (§10).
- **Internal tombstone GC:** `yrs`'s default garbage-collection behavior
  already reclaims the actual *content* of deleted items while permanently
  keeping a lightweight tombstone marker — which is exactly what preserves
  the delete-can't-be-silently-revived-by-an-older-peer guarantee. No
  custom GC design needed; just leave the default on.
- **No data loss anywhere:** the `yrs` update log is purely internal sync
  bookkeeping. It is not Blurt's source of truth for history — that's the
  `edits` table (Module 2), which compaction never touches. Compacting the
  CRDT layer only discards redundant internal diffs once sync has
  confirmed both sides agree on state; nothing user-visible is affected.

## 5. Sensitive-Destination Sync Policy

- **Sensitive destinations do sync** between the user's own paired
  devices, gated only by normal app-level unlock — **not** a fresh
  per-sync biometric/passphrase check.
- **Rationale:** the protection Module 2 actually cares about is
  view-time — stopping someone holding an unlocked phone from seeing
  sensitive content. Sync transmission is a different event: the payload
  is encrypted in transit (via the pairing-established channel key, §6)
  and at rest (SQLCipher) on both ends, and it only ever moves to a device
  the user already explicitly paired — never to an arbitrary new peer.
  Gating the transmission itself behind a biometric prompt on every app
  open (given one-shot-on-open sync) would tax the "zero-friction over
  correctness" principle for no real protection gain, since the content
  remains fully inert and unviewable on both devices regardless.
- **Unchanged:** viewing a sensitive destination, on either device, still
  always requires the fresh, uncached biometric/passphrase check with zero
  grace period, exactly as specified in `MODULE_02_SCHEMA.md` §5.

## 6. Device Pairing & Key Exchange

- A **one-time pairing step** (e.g. QR-code scan between two of the user's
  own devices) establishes a shared secret for the P2P sync channel.
- After initial pairing, sync is **fully automatic** — no repeated
  pairing prompts or manual steps on subsequent app opens.
- Exact pairing protocol (key exchange algorithm, QR payload format,
  session key rotation policy) is an implementation detail, not a product
  decision — deferred to build time (§10).

## 7. New-Device Bootstrap

- First-time pairing prioritizes **recent content first**: the most
  recently active destinations/items transfer immediately, so the new
  device is usable almost right away. This mirrors the recency-first
  philosophy already established in `MODULE_04_EMBEDDINGS_RAG.md` §5
  (Sleep-Mode's 30-day default retrieval window).
- Older history continues transferring in the background afterward, with
  a visible progress indicator — comparable to WhatsApp-style chat history
  transfer UX.
- No separate bootstrap mechanism needed — it uses the same mDNS/local
  transport as ongoing sync; a first-time pairing is just treated as "a
  very large initial delta."

## 8. Embeddings Sync

- **Vector embeddings are not synced.** Each device regenerates them
  locally from synced plaintext rather than transferring vector blobs.
- **Rationale:** cheaper than transferring vectors (plaintext is smaller
  than a 384-dim float vector for most short blurts), and safe to do
  because embedding generation is deterministic — the same input text run
  through the same model version always produces the same output vector,
  regardless of which device computes it.
- **Dependency this relies on:** the embedding model version must be
  pinned and shipped in lockstep with app releases, not silently
  auto-updated independently per device. If two devices ever ran different
  model versions, they'd compute slightly different vectors for the same
  synced text — not a hard bug, but it would quietly degrade cross-device
  search consistency until both devices reconverge on the same model
  version.

## 9. Transport & Discovery

- **v1 ships mDNS only** — local-network-scoped discovery and transport.
- **Architecture is transport-agnostic by design:** peer discovery and
  payload exchange sit behind a generic interface, so a second
  discovery/transport backend can be added later without redesigning the
  CRDT sync engine itself.
- **BLE was evaluated and explicitly deferred for v1**, not ruled out.
  Cross-platform libraries exist (Rust's `btleplug`, with Tauri wrappers
  like `tauri-plugin-blec`), and since Blurt only syncs in the foreground
  (§1), it avoids the worst of iOS's background-BLE restrictions. The
  remaining cost — dual central/peripheral role handling, extra permission
  prompts, and BLE's low throughput meaning mDNS would still be needed for
  bulk/bootstrap transfers anyway — was judged not worth taking on
  simultaneously with getting core CRDT merge correctness proven solid.
  Fast-follow candidate once v1 sync is stable.

## 10. Explicitly Deferred / Not Yet Designed

- **BLE (and any WAN-capable/libp2p) transport** — deferred to a
  post-v1 pass; the transport interface (§9) is reserved for it.
- **Exact pairing protocol** — key exchange algorithm, QR payload format,
  key rotation (§6).
- **Exact compaction thresholds** — the size/age numbers in §4 are
  proposed defaults, not final; tune against real usage data.
- **Ordered-list CRDT for `sortOrder`** — only worth the added complexity
  if per-item LWW reordering (§3) proves confusing in practice.
- **Multi-user / shared lists** — explicitly out of scope for v1, per
  `BLUEPRINT.md` §5.
- **Conflict UX** — whether/how the user is ever shown that a field was
  resolved via most-recent-wins (e.g. a subtle "updated elsewhere"
  indicator) is not yet decided.
