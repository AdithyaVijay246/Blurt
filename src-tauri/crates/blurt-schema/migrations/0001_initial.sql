-- Migration 0001 — initial schema.
-- Tables and columns follow docs/MODULE_02_SCHEMA.md §2 exactly.
--
-- Conventions:
--   * UUIDs are TEXT (36-char hyphenated). Whole-database encryption means
--     there is no privacy cost to readable ids, and it keeps Module 5's
--     per-destination CRDT doc keys inspectable.
--   * Timestamps are INTEGER Unix milliseconds. Module 5 resolves conflicts
--     last-write-wins by comparing these directly, and the frontend can pass
--     them to `new Date(...)` untouched.
--   * Booleans are INTEGER 0/1 with CHECK constraints.
--   * deletedAt is a tombstone. Nothing in Blurt is ever hard-deleted.

-- ---------------------------------------------------------------------------
-- destinations — lists and notes share a table; `type` drives behavior.
-- ---------------------------------------------------------------------------
CREATE TABLE destinations (
    id          TEXT    PRIMARY KEY NOT NULL,
    parentId    TEXT             REFERENCES destinations(id),
    name        TEXT    NOT NULL,
    trigger     TEXT    NOT NULL,
    type        TEXT    NOT NULL CHECK (type IN ('list', 'note')),
    isSystem    INTEGER NOT NULL DEFAULT 0 CHECK (isSystem IN (0, 1)),
    isSensitive INTEGER NOT NULL DEFAULT 0 CHECK (isSensitive IN (0, 1)),
    sortOrder   INTEGER NOT NULL DEFAULT 0,
    createdAt   INTEGER NOT NULL,
    deletedAt   INTEGER
);

-- §2 specifies `trigger` as unique. It must be unique among *live*
-- destinations only: deletes are tombstones, so a plain UNIQUE constraint
-- would burn a trigger permanently the first time its destination was
-- deleted.
CREATE UNIQUE INDEX idx_destinations_trigger_live
    ON destinations(trigger) WHERE deletedAt IS NULL;

-- Walking the parentId chain to compute a display path, and scoping the
-- Module 3 `@` picker to one hierarchy depth.
CREATE INDEX idx_destinations_parent ON destinations(parentId, deletedAt);

-- ---------------------------------------------------------------------------
-- items — list items and note entries, same shape.
-- ---------------------------------------------------------------------------
CREATE TABLE items (
    id            TEXT    PRIMARY KEY NOT NULL,
    destinationId TEXT    NOT NULL REFERENCES destinations(id),
    -- Immutable: the raw capture, written once and never updated.
    originalText  TEXT    NOT NULL,
    -- Denormalized latest text, so the timeline reads without joining edits.
    currentText   TEXT    NOT NULL,
    -- Only meaningful for list-type destinations; NULL for notes and Unsorted.
    checked       INTEGER CHECK (checked IN (0, 1)),
    createdAt     INTEGER NOT NULL,
    deletedAt     INTEGER
);

-- "All items in a destination", and the Unsorted badge count.
CREATE INDEX idx_items_destination ON items(destinationId, deletedAt);

-- ---------------------------------------------------------------------------
-- edits — append-only history. One row per edit, never updated in place.
-- ---------------------------------------------------------------------------
CREATE TABLE edits (
    id       TEXT    PRIMARY KEY NOT NULL,
    itemId   TEXT    NOT NULL REFERENCES items(id),
    text     TEXT    NOT NULL,
    editedAt INTEGER NOT NULL
);

CREATE INDEX idx_edits_item ON edits(itemId, editedAt);

-- ---------------------------------------------------------------------------
-- embeddings — join layer between SQLCipher and the LanceDB vector store.
-- Every version of an item's text gets its own embedding (original + each
-- edit), so searching either the old or new wording finds the item.
-- Results are deduplicated by itemId at the display layer.
-- ---------------------------------------------------------------------------
CREATE TABLE embeddings (
    id               TEXT    PRIMARY KEY NOT NULL,
    itemId           TEXT    NOT NULL REFERENCES items(id),
    vectorRef        TEXT    NOT NULL,
    chunkIndex       INTEGER NOT NULL,
    -- Character range this chunk covers within the item's full text, so a
    -- search hit can scroll to and highlight the exact span in a long note.
    -- Consecutive chunks overlap by ~15-20% by design; ranges overlapping
    -- slightly is expected, not a bug.
    chunkStartOffset INTEGER NOT NULL,
    chunkEndOffset   INTEGER NOT NULL
);

CREATE INDEX idx_embeddings_item ON embeddings(itemId);

-- ---------------------------------------------------------------------------
-- keywords — cheap local keyword extraction (RAKE/YAKE/TF-IDF, no LLM) for
-- exact-match filtering alongside semantic search.
-- ---------------------------------------------------------------------------
CREATE TABLE keywords (
    itemId  TEXT NOT NULL REFERENCES items(id),
    keyword TEXT NOT NULL
);

CREATE INDEX idx_keywords_keyword ON keywords(keyword);
CREATE INDEX idx_keywords_item    ON keywords(itemId);

-- ---------------------------------------------------------------------------
-- app_secrets — secret material that must live inside the encrypted database.
--
-- Currently holds the recovery key, so Settings can re-display it
-- (MODULE_06_UI_SHELL.md §D2) behind a fresh auth check. It is plaintext
-- nowhere on disk: this table is inside the SQLCipher-encrypted file, and the
-- keyring file beside it holds only wrapped blobs.
--
-- Nothing in this table is ever embedded, keyword-extracted, fed to a model,
-- or synced.
-- ---------------------------------------------------------------------------
CREATE TABLE app_secrets (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- Seed rows.
--
-- The ids below are fixed constants rather than freshly generated per install,
-- and that is deliberate. Each device runs this migration on first launch,
-- before any pairing. If two devices generated different ids for their own
-- Unsorted, pairing them (MODULE_05_CRDT_SYNC.md §7) would merge two
-- separate Unsorted destinations into the account instead of one. Stable
-- well-known ids make the seeded destinations converge on first sync.
-- ---------------------------------------------------------------------------

-- §1: Unsorted is not a special structure — just a row with isSystem = 1,
-- hidden from the normal @ picker.
INSERT INTO destinations (id, parentId, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
VALUES (
    '00000000-b1c7-4000-8000-000000000001',
    NULL, 'Unsorted', 'unsorted', 'list', 1, 0, 0,
    CAST(strftime('%s', 'now') AS INTEGER) * 1000
);

-- §6: only "Random Thoughts" ships premade, and it is an ordinary
-- destination — not isSystem, not isSensitive. Module 3 treats it as a
-- permanent fixture, but the user can rename, retrigger, or delete it.
INSERT INTO destinations (id, parentId, name, trigger, type, isSystem, isSensitive, sortOrder, createdAt)
VALUES (
    '00000000-b1c7-4000-8000-000000000002',
    NULL, 'Random Thoughts', 'random', 'note', 0, 0, 1,
    CAST(strftime('%s', 'now') AS INTEGER) * 1000
);
