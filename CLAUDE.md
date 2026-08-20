# CLAUDE.md

This file is project context for Claude Code. Read this and the docs it
references before making changes.

## Start here every session

Before any other doc or code, establish where the work actually stands, from
**both** of these — they answer different questions and can disagree:

1. **`git log --oneline -15`** — what changed most recently, in fact. If it
   disagrees with `PROGRESS.md`, git is right about *what happened* and
   `PROGRESS.md` is stale; say so and reconcile it.
2. **`docs/PROGRESS.md`** — the narrative git can't carry: the exact next
   action to resume from, current state per crate, implementation decisions
   that aren't in any design doc, and build gotchas.

Also run `git status` before starting — uncommitted work from an interrupted
session is invisible to `git log` and easy to clobber.

**Update `PROGRESS.md` and commit before the session ends.** A commit without
the `PROGRESS.md` update loses the "why" and the resume point; a
`PROGRESS.md` update without a commit loses the diff.

Design status and implementation status are different things. The module docs
and "Current module status" below describe what has been *designed*; all six
modules are design-finalized and that says nothing about what code exists.
Only `docs/PROGRESS.md` and git history track what exists.

Remote: `https://github.com/AdithyaVijay246/Blurt`. Push regularly. Ask before
pushing — don't assume a local commit is meant to be published yet.

## What this project is

Blurt — a local-first, E2EE, zero-friction "everything" log. Full vision
and stack: see `docs/BLUEPRINT.md`. All six modules are now fully
design-finalized:

- `docs/MODULE_01_ARCHITECTURE.md` — repo structure, Tauri + Rust core
  architecture, Claude Code context strategy
- `docs/MODULE_02_SCHEMA.md` — SQLCipher schema, key-wrapping encryption
- `docs/MODULE_03_ROUTER.md` — `@` parsing, NL routing
- `docs/MODULE_04_EMBEDDINGS_RAG.md` — embeddings, LanceDB, local LLM,
  Sleep-Mode
- `docs/MODULE_05_CRDT_SYNC.md` — `yrs`/CRDT sync engine, mDNS, device
  pairing
- `docs/MODULE_06_UI_SHELL.md` — full UI shell (capture & timeline,
  security & sync UX, search & AI surfacing, settings, onboarding)

Read the relevant one in full before implementing anything in that module.
Don't infer design intent from code comments or partial context alone —
the markdown docs are the source of truth, not any prior code.

Every crate under `src-tauri/crates/` and the `src/` UI directory has its
own nested `CLAUDE.md` pointing at the specific doc(s) that govern it — see
`docs/MODULE_01_ARCHITECTURE.md` §4 for the exact repo layout and the
verbatim contents of each pointer file.

## Non-negotiable constraints (see `docs/BLUEPRINT.md` §4 for full rationale)

- **Stable IDs only.** Never reference a destination or item by name/path
  string. Always by UUID. Human-readable paths are computed on read from
  `parentId` chains, never stored.
- **Append, don't overwrite.** Edits go in the `edits` table. Deletes are
  tombstones (`deletedAt`), never hard deletes.
- **Nothing is ever unrouted.** Every captured blurt is an `items` row from
  the moment of capture — either in a real destination or in the reserved
  `isSystem` Unsorted destination. There is no separate "raw blurt" object.
- **Capture is never blocked.** Default behavior always saves instantly;
  any confirmation/decision step is async (a dismissible chip, a badge to
  resolve later) unless the user has explicitly opted into a stricter
  setting. This applies even to the Module 6 drag-to-expand note composer —
  exiting it auto-saves silently, no "discard changes?" prompt, by
  deliberate design choice. The one narrow, deliberate exception is the
  recovery-key onboarding step (`docs/MODULE_06_UI_SHELL.md` §B4), which
  requires a light verification step before continuing — justified because
  getting it wrong is unrecoverable, not routine.
- **Corrections layer on top, never reverse destructively.** The
  quick-confirm chip reassigns rather than retypes; drag-drop moves get a
  toast, not a forced redo; "did you mean to ask this?" adds an answer
  without touching the original capture. Follow this pattern for any new
  correction/undo affordance in the UI.
- **Secrets never touch a model.** If `destinations.isSensitive = true`,
  its items are excluded from embedding generation and keyword extraction,
  full stop. Sensitive queries are triaged via embedding similarity against
  destination *names only*, then resolved via direct DB lookup — never via
  RAG or LLM synthesis, local or cloud.
- **Sensitive access always requires a fresh, uncached auth check** —
  biometric or passphrase — regardless of app-level unlock state. App-level
  unlock itself may use a grace period / idle timer; sensitive access never
  does. (Exception: syncing a sensitive destination's encrypted data
  between the user's own paired devices does *not* require this fresh
  check — only viewing it does. See `docs/MODULE_05_CRDT_SYNC.md` §5.)
- **Encryption uses key-wrapping, not direct passphrase derivation.** The
  SQLCipher master key is random, wrapped independently by a passphrase
  slot and a recovery-key slot (`docs/MODULE_02_SCHEMA.md` §3) — this is
  what makes passphrase recovery and cross-device reset possible without
  re-encrypting the database. Don't reintroduce direct
  `key = Argon2id(passphrase)` derivation; it was deliberately superseded.
- **Transparency without interruption.** Automatic system decisions (an
  AI-synthesized answer, a sync merge that overwrote text) get a quiet,
  inspectable label or tag — never hidden, never a blocking prompt. See
  `docs/MODULE_06_UI_SHELL.md` §C2–C3.
- **The generative model ships bundled with the app**, not downloaded on
  demand — a multi-GB install, deliberately traded for zero AI setup
  friction and offline capability from first launch. Cloud providers are
  an explicit power-user/advanced option, never the default UI framing.
  See `docs/BLUEPRINT.md` §2 and `docs/MODULE_06_UI_SHELL.md` §D4.
- **The UI shell is fully themeable, not hardcoded.** Accent color,
  background color, and font are all user-selectable
  (`docs/MODULE_06_UI_SHELL.md` §D1); foreground/text color is always
  derived from background luminance, never picked directly, so no color
  combination the user picks can break legibility. Build components in
  relative terms (accent / background / muted-on-background), not
  hardcoded hex values.
- **Rust-only backend, Tauri shell.** Python was seriously evaluated for
  the data/sync/AI layer and dropped — see `docs/MODULE_01_ARCHITECTURE.md`
  §1 for the full rationale. Don't reintroduce a Python sidecar or backend
  process; every domain crate is Rust.

## Current module status

All six modules are now **design-finalized** — repo/core architecture,
schema/encryption, router/parser, embeddings/RAG, CRDT sync, and the full
UI shell. Nothing in the design roadmap remains undesigned; what's left is
implementation, plus a small list of explicitly-deferred tuning items (see
`docs/BLUEPRINT.md` §5 and each module doc's own "Explicitly Deferred"
section). Don't improvise architecture for anything not covered by a doc —
flag it back for a design discussion instead of guessing.

## Working conventions

- This is a large, multi-module project. Work module by module — don't
  try to hold the entire system in one pass. Reference the relevant design
  doc(s) for whatever module you're touching.
- When a design doc doesn't cover something you need (an open question,
  a deferred item, an implementation detail like exact thresholds/timers),
  check the doc's "Open Questions / Deferred" section first. If it's
  genuinely undecided, stop and ask rather than picking a default
  unilaterally — at this point that's limited to a handful of small tuning
  numbers (idle-timer default, note character threshold, which bundled
  model ships, same-level reorder interaction) explicitly left for
  real-usage data rather than more design discussion.
- Stack: Tauri (Rust + React/TypeScript), Rust-only backend — SQLCipher,
  LanceDB, `yrs` (mDNS transport for v1), FastEmbed, Ollama/llama.cpp
  (bundled). See `docs/BLUEPRINT.md` §2 for the full table and
  `docs/MODULE_01_ARCHITECTURE.md` §1 for the framework/language decision
  rationale.
- Repo layout, build conventions, and per-crate context files: see
  `docs/MODULE_01_ARCHITECTURE.md` §2–4.
