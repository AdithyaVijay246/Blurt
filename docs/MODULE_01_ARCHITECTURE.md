# Module 1 — Project Repository Structure & Core Architecture

Status: **Design finalized.** Not yet implemented. This is the last of the
six modules — all of Blurt's design is now complete.

## 1. Framework & Backend Language

- **Frontend framework: Tauri** (Rust core + React/TypeScript shell) —
  resolves `BLUEPRINT.md` §2's long-standing "Tauri or Flutter" question.
- **Backend language: Rust, exclusively.** The Python-backend option
  raised during Module 6 design (SQLCipher/LanceDB/`pycrdt` all have
  Python bindings) is explicitly **dropped**. It only made sense as an
  alternative to writing Rust; once Tauri was chosen specifically because
  Rust already has native, bridge-free support for every backend
  dependency (LanceDB, `yrs`/CRDT, FastEmbed), introducing Python would
  reintroduce the exact translation-layer complexity that choice was
  meant to avoid — and worse, a Python sidecar process doesn't work
  cleanly on mobile at all (Tauri's sidecar pattern is desktop-only).
- **Why Tauri over Flutter** (full comparison discussed in design, key
  points captured here since the reasoning matters for future
  maintainers):
  - Every backend library Blurt depends on (LanceDB, `yrs`, FastEmbed)
    has a first-class native Rust crate — zero bridge needed.
  - Flutter would need the same Rust backend regardless (no native Dart
    equivalents exist for any of these), reached via `flutter_rust_bridge`
    — so Flutter doesn't avoid Rust, it adds Dart + an FFI bridge on top
    of a Rust core you'd be writing either way.
  - Dart FFI is actually lower-overhead than Tauri's own Rust↔WebView IPC
    (no JS-engine round-trip), so "translation layer tax" isn't a point
    in Flutter's favor once you look closely — both frameworks have a
    boundary between the UI and the Rust data layer, just implemented
    differently.
  - Sensitive-data safety is a near-wash between the two: the
    "decrypted plaintext must briefly exist in UI-layer memory to render"
    problem is identical whether that's Dart's GC heap or a WebView's JS
    heap — neither can deterministically zero memory the way Rust's
    `zeroize` crate can inside the Rust layer itself. The real safety
    discipline (raw key material never crosses the boundary, only
    high-level results do) is framework-agnostic and applies either way.
  - Genuine costs of choosing Tauri, acknowledged rather than glossed
    over: webview-based mobile rendering is generally harder to get
    perfectly fluid for complex custom gestures (drag-to-expand, the
    elevator drag-and-drop from `MODULE_06_UI_SHELL.md`) than Flutter's
    own rendering engine; Tauri's mobile support, while stable, has a
    shorter production track record than Flutter's; and there's no
    established package like `home_widget` yet for bridging into native
    iOS/Android home-screen widgets — that scaffolding would need to be
    built mostly from scratch when the time comes.
  - Deciding factor: avoiding a second language and its bridge, plus the
    much larger open-source contributor pool for React/TypeScript versus
    Dart/Flutter, outweighed those costs.

## 2. Repository Structure

A Cargo workspace with one crate per design module, so the module
boundaries already established in design carry through to the code —
"work module by module" (`CLAUDE.md`'s existing working convention) holds
at the repo level too.

```
blurt/
├── CLAUDE.md
├── docs/
│   ├── BLUEPRINT.md
│   ├── MODULE_01_ARCHITECTURE.md
│   ├── MODULE_02_SCHEMA.md
│   ├── MODULE_03_ROUTER.md
│   ├── MODULE_04_EMBEDDINGS_RAG.md
│   ├── MODULE_05_CRDT_SYNC.md
│   └── MODULE_06_UI_SHELL.md
├── src-tauri/
│   ├── crates/
│   │   ├── blurt-schema/       (Module 2: SQLCipher, key-wrapping encryption)
│   │   ├── blurt-router/       (Module 3: @ parsing, NL routing)
│   │   ├── blurt-rag/          (Module 4: embeddings, LanceDB, local LLM)
│   │   ├── blurt-sync/         (Module 5: yrs/CRDT, mDNS, pairing)
│   │   └── blurt-app/          (orchestration + Tauri commands — thin)
│   └── Cargo.toml              (workspace root)
├── src/                        (React/TS frontend, Module 6)
│   ├── components/             (one folder per Section A–E)
│   ├── theme/                  (Appearance system — accent/background/font)
│   └── ...
└── package.json
```

- `blurt-app` is deliberately thin: it's the seam where Tauri commands
  call into the domain crates, not where business logic lives. Keeping
  logic out of the IPC layer means each domain crate stays independently
  testable and the command layer stays a simple adapter.
- Each domain crate maps 1:1 to a design doc. Someone implementing Module
  4 only ever touches `blurt-rag`.

## 3. Core Architecture

- **Single process, direct calls.** Everything runs in one Rust binary —
  cross-crate calls are ordinary Rust function calls, no internal RPC.
  That complexity only exists in a cross-language-bridge world, which
  choosing Tauri specifically avoided.
- **Async runtime: Tokio** (Tauri's default) for anything touching disk,
  network (mDNS/sync), or the local LLM.
- **Local LLM lifecycle.** A single "model manager" service inside
  `blurt-rag` owns load/unload state behind a mutex — the one place in
  the codebase responsible for enforcing Module 4's battery-saving
  premise (load only on a classified question, unload immediately after,
  never resident "just in case").
- **IPC contract: `tauri-specta`** (or equivalent) auto-generates
  TypeScript types from Rust command signatures, so the frontend/backend
  contract can't silently drift the way hand-duplicated types would.
- **Frontend updates: event-driven, not polled.** Rust pushes events to
  the frontend when something changes in the background — a sync merge
  landing, an embedding finishing indexing — matching Module 5's
  non-blocking sync and Module 4's async indexing, both of which already
  assume the UI reacts rather than asks.

## 4. Claude Code Context & Memory Strategy

Since this project is built with Claude Code working directly in the repo,
the doc placement is deliberately structured around how Claude Code
actually loads context (verified against current Claude Code docs, not
assumed):

- **Root `CLAUDE.md` loads in full at the start of every single session**,
  regardless of what's being worked on. It should therefore stay the lean
  index it already is (non-negotiable constraints, module status, working
  conventions) — not balloon with every module's full content.
- Claude Code supports an `@path` import syntax that force-loads a file's
  full content into every session. **Deliberately not used for the module
  docs** — importing all six into root `CLAUDE.md` would mean every
  session pays the context cost of all of them, even a session that only
  touches `blurt-rag`.
- Instead: **nested `CLAUDE.md` files, one per crate/folder.** Claude Code
  doesn't load these at launch — it loads them automatically the moment
  it reads a file in that subdirectory. A one-line pointer is enough to
  get the real module doc read on demand, at zero context cost to
  sessions that never touch that crate.

Exact contents for each nested pointer file:

`src-tauri/crates/blurt-schema/CLAUDE.md`:
```
This crate implements Module 2 (SQLCipher schema, key-wrapping encryption).
Read ../../../docs/MODULE_02_SCHEMA.md in full before making any change here.
```

`src-tauri/crates/blurt-router/CLAUDE.md`:
```
This crate implements Module 3 (@ parsing, NL routing).
Read ../../../docs/MODULE_03_ROUTER.md in full before making any change here.
```

`src-tauri/crates/blurt-rag/CLAUDE.md`:
```
This crate implements Module 4 (embeddings, LanceDB, local LLM, Sleep-Mode).
Read ../../../docs/MODULE_04_EMBEDDINGS_RAG.md in full before making any change here.
```

`src-tauri/crates/blurt-sync/CLAUDE.md`:
```
This crate implements Module 5 (yrs/CRDT sync engine, mDNS, device pairing).
Read ../../../docs/MODULE_05_CRDT_SYNC.md in full before making any change here.
```

`src-tauri/crates/blurt-app/CLAUDE.md`:
```
This crate is the orchestration layer — Tauri commands calling into the
domain crates. Keep it thin; business logic belongs in the domain crates,
not here. Read ../../../docs/MODULE_01_ARCHITECTURE.md §3 for the intended
wiring (async runtime, LLM lifecycle manager, IPC contract, event-driven
frontend updates) before adding a command.
```

`src/CLAUDE.md`:
```
This is the UI shell (Module 6). Read ../docs/MODULE_06_UI_SHELL.md in full
before implementing any screen — it covers all five sections (Capture &
Timeline, Security & Sync UX, Search & AI Surfacing, Settings, Onboarding)
in detail, including exact interaction/animation specs. Don't improvise
gesture or layout behavior the doc already specifies.
```

## 10. Explicitly Deferred / Not Yet Designed

- **Frontend state-management library** (React Context, Zustand, Redux,
  or similar) — not decided; an implementation-time choice rather than an
  architectural one.
- **Testing strategy specifics** — per-crate unit test conventions,
  integration test coverage for cross-crate flows (capture → route →
  embed → sync), and end-to-end UI testing approach are all unaddressed.
- **CI/build pipeline** — cross-compilation matrix for Rust across
  iOS/Android/Windows/Mac, release/signing process, and how the bundled
  local LLM model file is packaged into each platform's build artifact.
- **Exact Cargo dependency versions** and crate choices beyond what's
  already named in Modules 2–5 (e.g. which SQLCipher Rust binding
  specifically, exact `yrs` version) — implementation-time detail.
- **iOS/Android widget scaffolding** — flagged in the Tauri-vs-Flutter
  comparison as real, unsolved work (no `home_widget`-equivalent exists
  for Tauri yet); not designed here, revisit when that feature is
  actually prioritized.
