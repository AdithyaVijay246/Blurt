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

Last updated: 2026-08-24

---

## Resume here

**Next action:** wire `blurt-schema`'s repository layer into `blurt-app` as
Tauri commands. Module 2 itself (schema + keyring + repository/CRUD) is now
complete.

The repository layer (`src/repository/{destinations,items,edits,indexing}.rs`)
covers everything `MODULE_02_SCHEMA.md` specifies:

- **Destinations** — create, rename, reparent, tombstone, `path` (walks
  `parentId` on read per §1 — never stored).
- **Items** — capture, `move_to` (reparenting/resolving-Unsorted/dragging are
  all this one write), `set_checked`, tombstone, `list_for_destination`.
- **Edits** — `append` (one transaction: inserts the `edits` row and updates
  `items.currentText`; `originalText` is never touched), `history_for_item`.
- **Sensitive-safe accessor** — `indexing::items_for_indexing` joins
  `items`/`destinations` and filters `isSensitive = 0` structurally in the
  query itself. Covered directly:
  `sensitive_destinations_items_are_structurally_excluded`.

`blurt-app` still has no registered commands — that's the next session's
starting point. Modules 3-6 remain deferred beyond that.

---

## Status by component

| Component | State | Notes |
|---|---|---|
| Repo scaffold | **Done, verified** | Workspace builds clean; `npm run tauri dev` opens a blank window |
| Frontend skeleton (`src/`) | **Stub only** | Blank `App.tsx`; `npm run build` passes. No Module 6 work started |
| `blurt-schema` — keyring | **Done, green** | Key-wrapping, 3 slot kinds, recovery-key encoding |
| `blurt-schema` — DDL | **Done, green** | `migrations/0001_initial.sql`, all §2 tables + indexes + seeds |
| `blurt-schema` — db/migrations | **Done, green** | SQLCipher raw-key open + probe; `user_version` runner |
| `blurt-schema` — repository/CRUD | **Done, green** | `repository/{destinations,items,edits,indexing}.rs` |
| `blurt-router` (M3) | **Empty stub** | |
| `blurt-rag` (M4) | **Empty stub** | |
| `blurt-sync` (M5) | **Empty stub** | Will add its own migration for `yrs` update logs + paired devices |
| `blurt-app` | **Stub only** | `builder()` returns a bare `tauri::Builder`; no commands registered |

**67 tests green** across the workspace as of the last commit. Test command
(note the PATH requirement under Environment below):

```bash
cargo test -p blurt-schema
```

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

---

## Environment / build gotchas

- **Rust builds need Strawberry Perl ahead of MSYS Perl on `PATH`**, or
  OpenSSL's `Configure` fails with a missing `Locale/Maketext/Simple.pm`. From
  Git Bash:

  ```bash
  export PATH="/c/Strawberry/perl/bin:/c/Strawberry/c/bin:/c/Users/adith/.cargo/bin:$PATH"
  ```

  PowerShell resolves Strawberry Perl correctly with no change.

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

---

## Session log

Newest first. One short entry per session — what changed, not how.

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

### 2026-08-24
- Built the `blurt-schema` repository/CRUD layer, TDD throughout (Red
  confirmed via `todo!()` stubs before each implementation): `destinations`,
  `items`, `edits`, then `indexing::items_for_indexing` (the sensitive-safe
  accessor, tested directly against a seeded sensitive destination). **67
  tests green**, workspace `cargo build` clean. Module 2 is now fully
  complete — see decision #11 above for the `repository/` layout.
