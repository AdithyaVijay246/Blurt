# Blurt — Project Blueprint

## 1. Core Vision

Blurt is a zero-friction, end-to-end encrypted "everything" log. Instead of
forcing users to organize, choose folders, or format text beforehand (like
Obsidian or Notion), Blurt provides a single, ultra-minimal input bar. Users
blurt anything — thoughts, reminders, passwords, or grocery items — and an
intelligent local system logs, routes, connects, and remembers everything.

**Key Principles:**

- **Zero-Friction Capture** — single input box interface, capture is never
  blocked on a decision the user hasn't chosen to make.
- **100% Local-First & E2EE** — SQLCipher database with zero-knowledge
  AES-256-GCM encryption at rest. Zero cloud required.
- **Serverless P2P Proximity Sync** — on-app-open local network sync
  (mDNS / BLE / WebRTC) with zero remote servers.
- **Lightweight AI Memory** — on-device vector search + sleep-mode local
  LLMs to keep battery drain near zero.
- **Open-Source & Pluggable** — modular provider interface allowing local
  engines (Ollama / llama.cpp) or optional cloud APIs (OpenAI / Claude).
- **Fully Customizable Shell** — accent color, background color, and font
  are all user-selectable (`MODULE_06_UI_SHELL.md` §D1), on top of a
  component system deliberately designed in relative/themeable terms
  rather than hardcoded values.

## 2. Technical Stack

| Layer | Technology Choice | Function |
|---|---|---|
| Frontend Framework | Tauri (Rust + React/TypeScript) | Ultra-light cross-platform desktop & mobile shell |
| Local Database | SQLCipher (SQLite + E2EE) | Encrypted storage at rest, random master key wrapped by passphrase (Argon2id) and recovery-key slots |
| Vector DB | LanceDB / SQLite-vss | Local file-based vector index for similarity search |
| Sync Engine | `yrs` (CRDT, Rust) + mDNS (v1); BLE and libp2p evaluated, deferred post-v1 | Serverless, P2P on-open proximity sync |
| Embedding Engine | FastEmbed (`all-MiniLM-L6-v2` or `bge-small`) | Local text-to-vector embedding (~100MB model, 256-token limit) |
| Local LLM Engine | Ollama API / `llama.cpp` wrapper — **ships bundled inside the app install**, not downloaded on demand | Sleep-mode local LLM (Qwen 2.5 3B / Llama 3.2 3B / Phi-3.5 — specific model still TBD) |

**Bundled-model tradeoff:** shipping the generative LLM inside the install
(several GB) means the app is a multi-GB download from day one, in exchange
for zero AI setup friction and full offline capability from first launch.
Deliberate, not reconsidered without a specific reason to — see
`MODULE_06_UI_SHELL.md` §D4–D5. Cloud providers remain available as an
explicitly power-user/advanced option via the pluggable `AIProvider`
interface (§1), not the default most users will ever touch.

**Backend language: Rust, decided.** Module 1 evaluated Tauri (Rust) against
Flutter (Dart) as the app framework, with backend language as part of that
call. Python had a genuinely viable path for the data/sync/AI layer alone
(SQLCipher, LanceDB, FastEmbed, and `pycrdt`/`y-py` all have first-class
Python bindings), but was dropped once Tauri was chosen for the shell —
running a Python backend process alongside a Tauri frontend is a workable
desktop-only pattern (sidecar process) but doesn't extend cleanly to the
mobile targets, and splitting the backend across two languages/runtimes
adds real maintenance cost for no corresponding benefit once Rust already
covers the domain crates natively. Every domain crate (schema, router, RAG,
sync, app orchestration) is Rust. Full rationale, including the
Tauri-vs-Flutter comparison itself: see `MODULE_01_ARCHITECTURE.md` §1.

## 3. Module Roadmap & Status

| # | Module | Status |
|---|---|---|
| 1 | Project repository structure & core architecture | **Design finalized** — see `MODULE_01_ARCHITECTURE.md` |
| 2 | SQLCipher storage schema + local E2EE setup | **Design finalized** — see `MODULE_02_SCHEMA.md` |
| 3 | Hierarchical List Router & Syntax Parser | **Design finalized** — see `MODULE_03_ROUTER.md` |
| 4 | Local embeddings & RAG pipeline (LanceDB + Ollama) | **Design finalized** — see `MODULE_04_EMBEDDINGS_RAG.md` |
| 5 | Yjs/`yrs` CRDT + mDNS/libp2p local P2P sync engine | **Design finalized** — see `MODULE_05_CRDT_SYNC.md` |
| 6 | Minimal UI/UX shell | **Design finalized** — Core Capture & Timeline, Security & Sync UX, Search & AI Surfacing, Settings, and Onboarding all specified — see `MODULE_06_UI_SHELL.md` |

"Design finalized" means the module has been fully spec'd through
discussion. It says nothing about whether the module has been built — the
table above tracks design only. **All six modules are now design-finalized.**
This completes the design roadmap; what remains is implementation, plus a
short list of explicitly-deferred tuning items (§5 below and each module
doc's own "Explicitly Deferred" section).

For implementation status — what actually exists in code, and where to pick
up — see `PROGRESS.md`, which is the source of truth for that.

## 4. Cross-Module Decisions (apply everywhere)

These were established during design and should be treated as binding
constraints on every module, not just the one where they came up:

- **Stable IDs everywhere.** Every destination and every item gets a
  permanent UUID at creation. Nothing is ever referenced by name or path
  string — always by ID. This is what makes drag-and-drop reparenting,
  renaming, and CRDT sync cheap and conflict-safe.
- **Append, don't overwrite.** Edits, list items, and note entries are
  never destructively replaced — they're appended to a history and the
  "current" state is derived from it. This applies uniformly to `items`
  (via the `edits` table) and to notes (which are just append-only logs of
  entries, same shape as list items).
- **Nothing is ever truly unrouted.** Every blurt lands somewhere the
  instant it's captured — either a real destination, or the reserved
  system "Unsorted" destination. There is no separate "raw blurt" concept
  outside of `items` — the item *is* the blurt from the moment of capture.
- **Zero-friction over correctness.** Where there's a tradeoff between
  interrupting the user for a decision vs. saving instantly and letting
  them resolve ambiguity later, default to saving instantly. Confirmation
  prompts are opt-in settings, not the default behavior. (Module 6's
  drag-to-expand note composer deliberately upholds this: exiting
  auto-saves silently, no discard-confirmation prompt, even though most
  editors would show one. The recovery-key onboarding step is a
  deliberate, narrow exception. Onboarding as a whole follows this too —
  only passphrase setup and recovery-key verification are mandatory;
  pairing, destination creation, appearance, and the tutorial are all
  skippable or entirely absent from onboarding.)
- **Corrections layer on top, never reverse destructively.** The
  quick-confirm chip reassigns rather than retypes; drag-drop moves get a
  toast, not a forced redo; "did you mean to ask this?" adds an answer
  without touching the original capture.
- **Secrets never touch a model.** Sensitive content (passwords, etc.) is
  excluded from embedding/keyword indexing and is never fed into the local
  or cloud LLM context. Sensitive queries are triaged via lightweight
  embedding similarity against destination *names*, not content, and
  resolved via direct DB lookup — never via RAG/LLM synthesis.
- **Transparency without interruption.** Where the system makes an
  automatic decision on the user's behalf (an AI-synthesized answer, a
  sync merge that overwrote text), it says so quietly — a small label or
  tag, inspectable if you look — rather than either hiding it or
  interrupting with a prompt. See `MODULE_06_UI_SHELL.md` §C2–C3.
- **Rust-only backend, Tauri shell.** See §2's backend-language note — this
  applies to every domain crate, not just the ones considered during the
  Python evaluation.

## 5. Open Threads (deliberately tabled, not forgotten)

All Module 1–6 design threads are now resolved. What remains is
implementation-time tuning, not further design discussion:

- **Which specific bundled model ships** (Qwen 2.5 3B / Llama 3.2 3B /
  Phi-3.5) — an implementation-time decision, not a design one.
- **Exact character-count threshold** for note conversion
  (`MODULE_06_UI_SHELL.md` §A4) and **exact idle-timer default**
  (`MODULE_02_SCHEMA.md` §5) — both need real-usage tuning, not further
  design discussion.
- **Simple same-level reordering** interaction detail
  (`MODULE_06_UI_SHELL.md` §A7/§10) — minor, not yet spec'd in full.
- **Multi-user / shared lists** — explicitly out of scope for v1.
- **Tombstone purge policy** — currently keep-indefinitely; revisit only
  if real-world usage proves that wrong.
- **Frontend state-management library, testing strategy, CI/build
  pipeline, exact Cargo dependency versions, iOS/Android widget
  scaffolding** — deliberately left to implementation time, see
  `MODULE_01_ARCHITECTURE.md` §10.
