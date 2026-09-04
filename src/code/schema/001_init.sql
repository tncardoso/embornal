-- Migration 001: the collections, the tree of nodes and the pool of summaries.
--
-- The vector index is not here. Its width comes from the configuration, so
-- the code builds it at run time.

-- ---------------------------------------------------------------------------
-- Bookkeeping
-- ---------------------------------------------------------------------------

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- The collections
-- ---------------------------------------------------------------------------

-- One index of one repository.
--
-- The name of a collection is the canonical path of the root, so a repository
-- has one index and nobody must name it. A second collection over the same
-- root is a fork: it carries another name, keeps its own tree, and shares the
-- summaries through the pool below.
CREATE TABLE collections (
    id         INTEGER PRIMARY KEY,
    ulid       TEXT NOT NULL UNIQUE,
    name       TEXT NOT NULL UNIQUE,
    root       TEXT NOT NULL,
    created_at TEXT NOT NULL,
    indexed_at TEXT
);

CREATE INDEX collections_root_idx ON collections(root);

-- ---------------------------------------------------------------------------
-- The tree
-- ---------------------------------------------------------------------------

-- One row for each directory, file and definition of a collection.
--
-- The row holds structure and hashes only. What the code does is a summary,
-- and a summary belongs to the pool, not to the row.
--
-- `content_hash` has two sources. A file and everything below it hash the
-- bytes of their own span. A directory and the root have no bytes of their
-- own, so they hash the hashes of their children: that is the one place where
-- the tree is a Merkle tree, and it is enough, because the bytes of a file
-- already move when any definition inside it moves.
CREATE TABLE nodes (
    id             INTEGER PRIMARY KEY,
    ulid           TEXT    NOT NULL UNIQUE,
    collection_id  INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    parent_id      INTEGER REFERENCES nodes(id) ON DELETE CASCADE,
    kind           TEXT    NOT NULL,
    name           TEXT    NOT NULL,
    qualified_name TEXT    NOT NULL,
    rel_path       TEXT    NOT NULL,
    language       TEXT,
    start_byte     INTEGER,
    end_byte       INTEGER,
    start_line     INTEGER,
    end_line       INTEGER,
    depth          INTEGER NOT NULL,
    content_hash   TEXT    NOT NULL,
    pool_key       TEXT    NOT NULL,
    parse_errors   INTEGER NOT NULL DEFAULT 0,

    -- Only the root of a collection has no parent.
    CHECK ((parent_id IS NULL) = (kind = 'repo')),
    -- Two nodes of one collection cannot answer to the same name.
    UNIQUE (collection_id, qualified_name)
);

CREATE INDEX nodes_parent_idx   ON nodes(parent_id);
CREATE INDEX nodes_file_idx     ON nodes(collection_id, rel_path);
CREATE INDEX nodes_pool_key_idx ON nodes(pool_key);
CREATE INDEX nodes_queue_idx    ON nodes(collection_id, depth DESC);

-- ---------------------------------------------------------------------------
-- The pool of summaries
-- ---------------------------------------------------------------------------

-- What an agent wrote about one piece of code.
--
-- The table carries no collection and no node, on purpose. A summary belongs
-- to `pool_key`, which is the qualified name and the content hash together,
-- so the same code answers with the same summary in every collection and in
-- every repository. A new collection over code that is already described
-- therefore starts with nothing to do, and no row is copied to make that
-- happen.
--
-- A node is stale when, and only when, its `pool_key` is absent here. There
-- is no flag to fall out of step with the tree.
--
-- The integer key exists because the vector index needs an INTEGER PRIMARY
-- KEY to point at.
CREATE TABLE summaries (
    id          INTEGER PRIMARY KEY,
    pool_key    TEXT NOT NULL UNIQUE,
    summary     TEXT NOT NULL,
    description TEXT NOT NULL,
    author      TEXT NOT NULL,
    written_at  TEXT NOT NULL,

    CHECK (summary <> '' AND description <> '')
);

-- ---------------------------------------------------------------------------
-- The keyword index
-- ---------------------------------------------------------------------------

-- The index holds no copy of the text: `content` points it at the table, and
-- the triggers below keep the two together.
CREATE VIRTUAL TABLE summaries_fts USING fts5(
    summary,
    description,
    content='summaries',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER summaries_fts_insert AFTER INSERT ON summaries BEGIN
    INSERT INTO summaries_fts(rowid, summary, description)
    VALUES (new.id, new.summary, new.description);
END;

CREATE TRIGGER summaries_fts_delete AFTER DELETE ON summaries BEGIN
    INSERT INTO summaries_fts(summaries_fts, rowid, summary, description)
    VALUES ('delete', old.id, old.summary, old.description);
END;

CREATE TRIGGER summaries_fts_update AFTER UPDATE ON summaries BEGIN
    INSERT INTO summaries_fts(summaries_fts, rowid, summary, description)
    VALUES ('delete', old.id, old.summary, old.description);
    INSERT INTO summaries_fts(rowid, summary, description)
    VALUES (new.id, new.summary, new.description);
END;
