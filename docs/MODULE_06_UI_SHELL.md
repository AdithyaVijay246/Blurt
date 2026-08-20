# Module 6 — Minimal UI/UX Shell

Status: **Design finalized.** Covers Sections A–E: Core Capture & Timeline,
Security & Sync UX, Search & AI Surfacing, Settings, and Onboarding. Not
yet implemented.

This module closes out a number of items other modules explicitly deferred
here: the drag-to-expand gesture's feel (Module 3), the "AI summary"/
"did you mean to ask this?" styling and the standalone search entry point
(Module 4), conflict-resolution visibility (Module 5), and the
device-pairing/sensitive-access/passphrase-recovery UI (Module 2, Module 5).
All are resolved inline below.

## Section A — Core Capture & Timeline

### 1. Screen Structure & Navigation

- **Home screen:** the input bar sits near the **top** of the screen, with
  a scrolling timeline of recent activity below it — a unified,
  chronological feed of everything captured across every destination, not
  a blank capture-only screen. Expanded by default; collapsible if the
  user wants the bar alone. This was a deliberate lean into the
  "everything log" framing over a pure minimal-capture-tool framing, per
  zero-friction philosophy (`BLUEPRINT.md` §4) — your own history should
  be visible by default, not hidden behind a tap.
- **Top-left:** app logo by default; swaps to a **back button** when
  navigating deeper into Browse, or into an expanded note composer (§4).
- **Top-right:** the Unsorted badge (§1 continued below) — notification-
  style icon + count, in the conventional badge position.
- **Bottom tab bar:** three icons — **Home/Timeline**, **Browse**,
  **Settings**. No separate Search tab — search lives inside the input
  bar itself (§3), so a dedicated nav entry would just be a second path to
  the same place. When the on-screen keyboard is up (user is typing), the
  keyboard naturally covers the tab bar, same as any app with bottom nav
  and a text field — no special handling needed, since the input bar
  itself lives at the top, not competing with the keyboard for space the
  way a bottom-docked compose bar would.
- **Unsorted badge placement rationale:** top-right matches the universal
  notification-badge convention (Instagram, Twitter, etc.), and keeps
  top-left free for logo/back-nav. Tapping it opens a dedicated Unsorted
  triage view.

### 2. Visual Language: Panels vs. Plain Content

A consistent rule applied throughout this module: **functional controls
are panelized (cards, elevated backgrounds, borders); actual content is
not.**

- The input bar itself, and any menu that pops out *from* it (the `@`
  picker dropdown, and by extension any similar control menu), are
  rendered as a distinct panel/card — similar to how Claude's own input
  box and its model-picker menu are styled. This gives capture controls
  clear visual affordance as *interactive chrome*. Security screens
  (Section B) and Settings (Section D) follow the same panel language,
  since they're controls too.
- Everything that is actual data — timeline rows, search results, list
  items, note entries, the AI summary answer (§C2) — renders as **plain
  text directly on the app background**, with blank-line spacing between
  entries, no card/panel wrapper. Long text truncates with a blur/gradient
  fade at the cutoff point rather than an ellipsis.
- Corollary: while the drag-to-expand note composer (§4) is mid-animation,
  its panel chrome dissolves away as part of the gesture, leaving plain
  text once fully expanded — the composer ends up matching the plain-text
  content language, not the panel language, once it's a full-screen
  writing surface. Toasts (the drag-and-drop undo notification, §9) and
  the quick-confirm chip (§5) stay on the panel side of the line, since
  they're transient controls, not content.
- All of this is themeable per Section D's Appearance settings (accent
  color, background color, font) — described here in relative terms
  ("panel," "muted," "background") specifically so the whole visual
  system sits on top of user-chosen colors rather than hardcoded ones.

### 3. The Input Bar & Capture Panel

- Persistent panel near the top of the home screen (§1).
- **Search mode:** a search icon docked on the input bar (always visible)
  toggles the bar into **plain, deterministic search** — distinct from
  the AI-synthesized "ask" flow that's triggered implicitly by typing a
  question into normal capture mode (§C2, `MODULE_04_EMBEDDINGS_RAG.md`
  §9). Search mode is the "distinct entry point" for plain search that
  Module 4 §11 flagged as needed. Toggling it: placeholder text changes
  (e.g. "Search your blurts…"), typing no longer captures — it queries
  live, results replacing the timeline feed underneath, refining as you
  type (cheap since it's LLM-free embedding/keyword lookup, not a
  generative call). The same `@` scoping mechanic from capture applies
  here too. Tapping the icon again (or a clear/X) exits back to capture
  mode and restores the timeline.
- The `@` picker dropdown (`MODULE_03_ROUTER.md` §2) renders as its own
  panel per §2 above, appearing directly below the input bar, live-
  filtered as the user types.

### 4. Drag-to-Expand: Long-Form Notes

Resolves the "visual/interaction spec" Module 3 §7 explicitly left open.

- Because the input bar lives near the top with the timeline below it,
  the expand gesture is a **drag down** (not up) — there's room to grow
  into.
- A persistent horizontal handle sits at the bottom edge of the input
  bar at all times.
- **Character threshold** (Module 3's deferred exact number): proposed
  starting point of **~150–250 characters**, expected to be tuned after
  real usage testing rather than treated as final.
- Once the threshold is crossed, the handle highlights/pulses and a small
  popup suggests expanding into a note.
- The gesture itself is **fully fluid and reversible**: dragging the
  handle down drives the animation directly (not a triggered/committed
  transition) — the input bar's panel chrome fades away proportionally to
  drag distance, while the text itself stays visually pinned in place so
  nothing jumps. Releasing mid-drag reverses the animation back to the
  compact bar. Nothing commits until the handle is released past the
  completion point.
- Fully expanded, the composer is a full-screen writing surface with the
  panel chrome gone (per §2's plain-text-for-content rule) and the top-
  left logo swapped for a back button.
- **Exit behavior:** tapping back **auto-saves whatever was typed**,
  silently, with no discard-confirmation prompt. This was a deliberate
  choice — an earlier draft of this spec considered a "confirm before
  losing unsaved changes" pattern (standard in most editors), but that
  would have created a scoped exception to the project's non-negotiable
  "capture is never blocked, saves instantly" principle
  (`BLUEPRINT.md` §4, `CLAUDE.md`). Decided against it: notes behave
  exactly like every other capture — instant, no confirmation — and if
  an in-progress note isn't wanted, the user just deletes it afterward via
  the normal tombstone-delete flow (`MODULE_02_SCHEMA.md`), same as any
  other item.

### 5. Quick-Confirm Chip & Reassignment

- After a natural-language blurt gets auto-routed (`MODULE_03_ROUTER.md`
  §3), a small pill-shaped chip appears briefly below the input bar,
  above the timeline — e.g. "→ Shopping" — panel-styled per §2.
- **Fades automatically after a few seconds** (revised from Module 3's
  original "persists until next blurt" framing — shortened for a lighter
  footprint).
- The chip's correction action is labeled **"change," not "undo"**
  — nothing is actually being reversed, since the capture itself already
  succeeded the moment it saved (zero-friction principle). Tapping it
  opens the same `@` picker used during capture, pre-scoped to reassign
  just that one already-saved item to a different destination — no
  retyping.
- This reassignment interaction (open the `@` picker, pick a new
  destination for an already-saved item) is the same mechanism used to
  resolve an item out of Unsorted (`MODULE_03_ROUTER.md` §3) — one
  reusable "reassign destination" component, two entry points into it.

### 6. Timeline (Home Feed) Rendering

- Mixed, chronological feed of everything captured, across all
  destinations and both item types (list items and note entries).
- Each row: the text (blur-truncated if long, per §2), a subtle
  persistent destination tag (e.g. "→ Shopping" — same visual language as
  the quick-confirm chip but not fading), and a relative timestamp
  ("2m ago").
- **Inline checkboxes:** for items belonging to a list-type destination,
  the checkbox renders directly on the timeline row — checking something
  off doesn't require opening that list. A deliberate zero-friction win,
  not just a default.
- **Unsorted items** get a distinct visual treatment (dashed border, or
  an "unsorted" tag in place of a destination name) so they're flagged as
  needing attention even before checking the badge count.
- **Sync tag (§C3):** an item whose text was overwritten by a concurrent
  edit from another device during a CRDT merge shows a small, subtle
  "synced" tag, tappable to open its full edit history.
- No cards/panels on rows — plain text per §2, blank-line separated.

### 7. Destination Detail Views

- **List view:** opening a list-type destination shows its items as a
  straightforward checklist — same row component as the timeline, scoped
  to just that destination, minus the "where did this go" tag (implied by
  context). Drag handles for manual same-level reordering.
- **Note view:** opening a note-type destination shows its entries
  (`MODULE_03_ROUTER.md` §5 — notes are append-only chronological logs,
  same underlying `items`/`edits` shape as list items) as timestamped
  blocks in a scrolling journal, no checkboxes.
- Both reuse the same underlying row/entry component as the timeline,
  just filtered and re-contextualized.

### 8. Browse Screen & Hierarchy Navigation

- Top-level destinations render as plain text rows (name + muted item
  count), no cards — consistent with §2.
- Tapping a destination drills into it, showing its children and its own
  items, with a breadcrumb pinned at top working alongside the top-left
  back button (§1).
- **Creating a destination:** a simple "+" as the last row in the list
  (not a floating action button) — tapping it drops an inline row to name
  the new destination, created at whatever depth is currently being
  browsed. Mirrors the `@` picker's existing "create at current depth"
  logic (`MODULE_03_ROUTER.md` §2) via a different physical entry point.
  This exact interaction is reused during onboarding (§E) for initial
  destination creation.

### 9. Drag-and-Drop Reorganization

Resolves `MODULE_03_ROUTER.md` §2 point 7 ("reorganizing destinations
after the fact... a manual, separate action via drag-and-drop in list
management") and applies identically to both items and whole
destinations.

- **Always live** — no separate "Edit"/"Manage" mode toggle. Long-
  pressing any row (item or destination) picks it up into a floating,
  held state immediately.
- **The "elevator" gesture:** while holding a floating item/destination,
  dragging it onto the screen's title/breadcrumb area swaps what's
  displayed underneath — from the current destination's contents to *its
  siblings* (i.e. the view steps up one level in the hierarchy) — while
  the dragged item stays floating throughout. The user can drop it on a
  sibling, keep dragging onto the title to climb further up, or drop it
  loose to file it directly under the level just navigated to. This maps
  directly onto the schema: it's the same `destinationId` (for items) or
  `parentId` (for destinations) update the `@` picker performs, just
  reached spatially instead of by typing.
- **Undo:** a lightweight toast appears immediately after any drop —
  e.g. "Moved to Shopping — Undo" — for a few seconds, one tap reverts
  the move exactly. No picker re-opens; this is a straightforward action
  reversal, not an ambiguity to resolve (contrast with §5's reassignment
  flow, which corrects a *guess*, not a deliberate action).

## Section B — Security & Sync UX

### 1. Sensitive Destination Masking

- A sensitive destination shows only its **name and a lock icon** —
  everywhere (Browse, timeline references, search). No content preview
  at all, not even a masked/blurred one. Chosen over a fully-hidden
  destination (which would break the app's "nothing is ever truly
  unrouted, everything is at least visible" spirit) and over a
  dots/blur-masked preview (unnecessary — the name and lock icon already
  signal "this exists, this is protected" without hinting at content).
  Amends `MODULE_02_SCHEMA.md` §4, which previously described this as
  generic "masked/hidden."

### 2. The Fresh-Auth Prompt

- **Mobile:** the native OS biometric sheet (Face ID/Touch ID), with
  passphrase as fallback if it fails or is unavailable. No custom UI —
  this is native OS chrome, appearing the instant a sensitive destination
  is opened.
- **Desktop:** no biometric hardware exists (`MODULE_02_SCHEMA.md` §5), so
  this is always an in-app passphrase entry — a **centered modal popup**
  with the background behind it blurred, not an inline popover anchored
  to whatever was tapped. Consistent with the panel treatment for all
  control surfaces (§A2).
- Reused for other security-critical actions in Settings: viewing the
  recovery key again, and changing the passphrase (§D3).

### 3. Encryption Key Architecture (reference)

Passphrase recovery required amending Module 2's key model from direct
passphrase-derivation to a key-wrapping scheme (random master key, wrapped
independently by a passphrase slot and a recovery-key slot) — see
`MODULE_02_SCHEMA.md` §3 for the full mechanism. This section covers only
the resulting screens.

### 4. Onboarding — Recovery Key

- Immediately after the user sets their initial master passphrase, a
  panel displays the recovery key, formatted in readable groups (e.g.
  `XXXX-XXXX-XXXX-XXXX`), with a copy button and a share-sheet option
  (save to Photos, a password manager, print, etc.).
- **Verification gate before continuing:** re-enter two of the groups
  (not the whole key) to confirm it was actually saved correctly. A
  bigger ask than routine capture flows get, but justified — this is a
  one-time, security-critical step where getting it wrong is
  unrecoverable, not a routine action the zero-friction principle is
  meant to protect.
- Full onboarding sequencing (where this step falls relative to others):
  see §E.

### 5. Forgot Passphrase

- A "Forgot your passphrase?" link on the app-level unlock screen (§6)
  offers two paths:
  - **Reset from another device:** reuses Module 5's existing QR-pairing
    mechanism, since the two devices are already paired. The locked-out
    device shows a QR/numeric code; the still-unlocked device scans or
    enters it, confirms "reset passphrase for [device]," and the
    locked-out device then shows a normal new-passphrase screen. Per
    `MODULE_02_SCHEMA.md` §5, this doesn't touch the master key at all —
    it just adds a new passphrase-wrap slot from the device that already
    holds the key unwrapped.
  - **Use recovery key:** the same grouped-input entry screen as §4's
    display, validated against the recovery-key slot; success drops into
    the same new-passphrase screen.
- If neither is available, the data is genuinely unrecoverable — see
  `MODULE_02_SCHEMA.md` §5.

### 6. App-Level Unlock Screen

- **Mobile:** the OS biometric prompt fires automatically on launch/
  resume-past-idle-timer — no custom screen. On failure/cancellation/
  unavailability, falls back to the same in-app passphrase screen as
  desktop.
- **Desktop:** always goes straight to the in-app passphrase screen (no
  biometric attempt). Full screen, not a popover — there's no existing
  app content to blur behind it yet. Logo top, passphrase field centered,
  panel-styled per §A2. Hosts the "Forgot your passphrase?" link (§5).

### 7. Device Pairing Flow

- Initiated from **Settings** ("Pair a device," §D4), and also offered as
  a step during onboarding (§E) — not Settings-only.
- Initiating device: generates and shows a QR code in a centered panel
  (same treatment as the passphrase popup, §2), plus a short numeric
  fallback code beneath it for situations where scanning isn't
  convenient (e.g. two desktops).
- Joining device: the equivalent Settings option opens a camera scanner,
  or a field to type the numeric code.
- On match: both devices show a brief "Paired!" confirmation, then move
  into the sync-progress state from `MODULE_05_CRDT_SYNC.md` §7 — a
  progress indicator with a status line ("Syncing recent activity…" →
  "Syncing older history…" once the recency-first chunk completes),
  non-blocking throughout so the device is usable immediately.

## Section C — Search & AI Surfacing

### 1. "Did You Mean to Ask This?" Affordance

Resolves `MODULE_04_EMBEDDINGS_RAG.md` §9's misclassification handling —
follows the same pattern as the quick-confirm chip (§A5).

- When a question gets misclassified as a statement and routed as a normal
  capture, this chip does **not** undo or replace that capture — the item
  stays exactly where it landed. Tapping the chip **additionally** runs
  the Sleep-Mode ask flow on the same text, surfacing an answer (§C2) as
  a separate result.
- Consistent with the app-wide pattern established by §A5 (reassign,
  don't retype) and §A9 (toast, don't force a redo): corrections layer on
  top of what already happened rather than reversing it. If the original
  capture turns out to be junk, it's deleted separately through the
  normal tombstone-delete flow, same as any other unwanted item.

### 2. "AI Summary" Label & Answer Layout

Resolves `MODULE_04_EMBEDDINGS_RAG.md` §11's deferred label wording/
styling.

- Typing a question directly into the normal capture bar (no toggle
  needed — distinct from Section A's explicit search-mode toggle)
  triggers Sleep-Mode retrieval + synthesis (`MODULE_04_EMBEDDINGS_RAG.md`
  §9). The answer takes over the same space search results already use —
  replacing the timeline feed below the input bar.
- **Label wording: "AI summary."** Rendered as small muted plain text
  (not a boxed badge) directly above the synthesized answer — it's
  content, just content that has to visibly announce what it is, per
  Module 4 §8's requirement that the user always know they're looking at
  a generated synthesis rather than a direct quote.
- **Sources list** renders below the answer using the same row styling as
  everything else (destination path, timestamp) — tapping a source jumps
  straight to the matching chunk in its original context
  (`MODULE_04_EMBEDDINGS_RAG.md` §7).
- The **"load more"** window-expansion action (§5) and the **empty-result
  message** (§8) both render as a simple action/row at the bottom of this
  same space — no separate screen or modal.
- The generative model powering this is the **bundled local model** —
  see §D5 for the ships-installed decision and its implications.

### 3. Sync Conflict Visibility

Resolves `MODULE_05_CRDT_SYNC.md` §10's open question on whether
last-write-wins resolutions are ever surfaced to the user.

- **Silent by default.** Routine LWW merges — reordering, checkbox state,
  reparenting — are never surfaced. Nothing actionable would come of
  showing them, and doing so would be noise against the zero-friction
  principle.
- **Exception — text conflicts.** When an item's `currentText`
  specifically gets overwritten by a concurrent edit from another device
  (`MODULE_05_CRDT_SYNC.md` §3), the item shows a small, subtle **"synced"
  tag** (§A6) — quiet, not interruptive, but honest and inspectable.
  Tapping it opens the item's full edit history, where nothing is
  actually lost regardless of which version is "current," since
  `MODULE_02_SCHEMA.md`'s append-only `edits` table retains every version
  either way.
- This mirrors the same transparency-without-interruption principle as
  the "AI summary" label (§C2): the system is honest about what it did
  automatically, without demanding the user's attention to do so.

## Section D — Settings

Five categories, each a list of plain-text rows (§A2) — no cards.

### 1. Appearance

- **Accent color:** open color picker, full freedom.
- **Background color:** open color picker, full freedom, with "Light" /
  "Dark" quick-select presets as starting points rather than forcing
  everyone to start from a blank picker.
- **Text/foreground color is never picked directly** — it's automatically
  computed from the chosen background's luminance (light background →
  dark text, dark background → light text). This is what makes "complete"
  color freedom safe: no combination the user picks can ever render text
  unreadable, because legibility isn't something they're asked to manage
  themselves.
- **Font:** a curated preset list (a clean default sans, a serif, a
  monospace, something rounder/friendlier) rather than custom font-file
  upload — avoids real cross-platform complexity (licensing, rendering
  consistency across Tauri's desktop and mobile targets, mobile OS
  restrictions on loading arbitrary fonts) for a feature most users would
  only use to pick "the one that feels right" anyway.
- **Live preview pane:** shows a sample input bar, a timeline row, and a
  button, updating in real time as color/font choices change, so nothing
  is committed blind.
- This entire visual system sits on top of these choices by construction
  — §A2's "panel vs. plain content" rules are expressed in relative terms
  (accent, background, muted-on-background) specifically so theming
  doesn't require touching component-level design later.

### 2. Security

- **Idle Timer:** current value shown, tap to adjust
  (`MODULE_02_SCHEMA.md` §5).
- **Sensitive Destinations:** tap opens the full destination list with an
  `isSensitive` toggle per row.
- **Recovery Key:** tap re-triggers the fresh-auth check (§B2), then
  re-displays the key using the same panel as onboarding (§B4).
- **Change Passphrase:** also gated behind a fresh-auth check first,
  consistent with how the app treats anything security-critical.

### 3. Sync

- **Paired Devices:** list of paired devices — name, last-synced relative
  time, an unpair action per row.
- **Pair a Device:** row at the bottom, reuses the QR/code flow from §B7.

### 4. AI

- Shows **"Local (bundled) — active"** as the default and only thing most
  users will ever see here — not a neutral "pick one" toggle. The local
  generative model ships **bundled inside the app install**, not
  downloaded on first use (see §5 below for the tradeoff this implies).
- A clearly secondary **"Advanced: use a cloud provider instead"**
  option, per the pluggable `AIProvider` interface (`BLUEPRINT.md` §1) —
  this is explicitly a power-user path for people who want to make use of
  the project's open-source pluggability, not a mainstream setting. An
  API key field only appears once a cloud provider is selected.
- Retrieval always stays local regardless of this setting
  (`MODULE_04_EMBEDDINGS_RAG.md` §10) — this toggle only affects the
  synthesis step.

### 5. General

- **Take a Tour:** an interactive, contextual walkthrough over the real
  UI — highlighting actual elements (the input bar, the drag-to-expand
  handle, the Unsorted badge, the search icon, etc.) with short tooltips
  at each stop, rather than a generic screenshot slideshow. Chosen
  specifically because so much of this app is gesture-driven (drag-down
  note expansion, the elevator drag-and-drop, search-mode toggle) —
  showing the actual gesture in context teaches it far better than
  describing it. Reachable any time from Settings; not part of onboarding
  (§E) at all.
- Home for any future misc items (About/version info, etc.).

**Bundled-model tradeoff (§4):** shipping the generative LLM inside the
app install (several GB, per `MODULE_04_EMBEDDINGS_RAG.md` §2's model
candidates) means the app itself is a multi-GB download from day one,
rather than a lightweight install that fetches the model later. Deliberate
tradeoff: zero AI setup friction and full offline capability from first
launch, at the cost of install size. Not reconsidered without a specific
reason to.

## Section E — Onboarding

Deliberately minimal — only what's genuinely mandatory or lightweight-
optional makes the cut. No appearance-customization step (Settings is
right there whenever the user wants it) and no forced tutorial (§D5
instead, entirely opt-in).

1. **Master passphrase setup.** Cannot be skipped — nothing works without
   it (`MODULE_02_SCHEMA.md` §3).
2. **Recovery key display + verification** (§B4) — immediately after.
3. **Sensitive-info passphrase choice** — reuse the master passphrase, or
   set a distinct one (`MODULE_02_SCHEMA.md` §5).
4. **Biometric enrollment** (mobile only) — native OS prompt, not custom
   UI.
5. **Pair a device — skippable.** Offered here ("got another device? pair
   it now" / "skip, I'll do this later"), reusing §B7's flow, not forced.
6. **Destination creation.** "Random Thoughts" is shown already created
   (the one destination that ships premade, resolving
   `MODULE_02_SCHEMA.md` §6's open question — chosen because Module 3
   treats it as a permanent, always-available fixture rather than
   something a user would think to create themselves). Below it, the same
   inline "+" row creation interaction from Browse (§A8), plus a handful
   of tappable suggested starter names (Shopping, Work, Ideas, Journal)
   for anyone who draws a blank — tap to instantly create, or type a
   custom name, or ignore entirely. A "Skip for now" action means none of
   this is required.
7. **Home screen.** Nothing else — no walkthrough, no tutorial, no
   appearance step.

## 10. Explicitly Deferred / Not Yet Designed

Everything originally scoped into Module 6 is now finalized. Only small,
already-scoped loose ends remain:

- **Exact character-count threshold** for note conversion (§A4) — the
  150–250 range is a starting point pending real-usage tuning.
- **Simple same-level reordering** (§A7's drag handles within a single
  list) — the elevator gesture (§A9) covers cross-level moves; plain
  reordering among siblings is assumed to work via standard drag-handle
  reordering but hasn't been spec'd in detail.
- **Which specific bundled model ships** (Qwen 2.5 3B / Llama 3.2 3B /
  Phi-3.5, per `MODULE_04_EMBEDDINGS_RAG.md` §2) — still an open choice
  among the three; an implementation-time decision, not a UI concern.
