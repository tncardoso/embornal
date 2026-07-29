-- Migration 001: the memory tree, the facts and the access rules.
--
-- The vector index is not here. Its width comes from the configuration, so
-- the code builds it at runtime.

-- ---------------------------------------------------------------------------
-- Bookkeeping
-- ---------------------------------------------------------------------------

-- Values that the database must remember about itself, such as the width of
-- the embeddings that its vector index holds.
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;

-- ---------------------------------------------------------------------------
-- The tree
-- ---------------------------------------------------------------------------

-- One row for each node of the wiki. The row holds structure only. The
-- description of a path is a fact of that path.
--
-- The root is row 1. It is the only row with no parent, and its segment is
-- empty.
CREATE TABLE paths (
    id         INTEGER PRIMARY KEY,
    ulid       TEXT    NOT NULL UNIQUE,
    parent_id  INTEGER REFERENCES paths(id) ON DELETE RESTRICT,
    segment    TEXT    NOT NULL,
    full_path  TEXT    NOT NULL UNIQUE,
    created_at TEXT    NOT NULL,

    -- Row 1 is the root, and only the root has no parent.
    CHECK ((parent_id IS NULL) = (id = 1)),
    -- Only the root carries an empty segment.
    CHECK ((segment = '') = (id = 1)),
    -- Two children of one parent cannot share a name.
    UNIQUE (parent_id, segment)
);

CREATE INDEX paths_parent_idx ON paths(parent_id);

-- ---------------------------------------------------------------------------
-- The facts
-- ---------------------------------------------------------------------------

-- One small statement that belongs to a path.
--
-- A fact never changes. To correct a fact, write a new one and point
-- supersedes_id at the old one. To remove a fact, set deleted_at.
CREATE TABLE facts (
    id              INTEGER PRIMARY KEY,
    ulid            TEXT    NOT NULL UNIQUE,
    path_id         INTEGER NOT NULL REFERENCES paths(id) ON DELETE RESTRICT,
    content         TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,

    -- Recall state. The strength itself is a function of the clock, so the
    -- database does not hold it.
    last_recall_at  TEXT,
    recall_count    INTEGER NOT NULL DEFAULT 0,
    stability_days  REAL    NOT NULL DEFAULT 1.0,

    supersedes_id   INTEGER REFERENCES facts(id) ON DELETE SET NULL,
    deleted_at      TEXT,

    -- The embedding waits here until a provider fills it in. The vector index
    -- reads this column during the backfill.
    embedding       BLOB,
    embedding_model TEXT,

    CHECK (length(trim(content)) > 0),
    CHECK (recall_count >= 0),
    CHECK (stability_days > 0),
    CHECK (path_id <> 1),                    -- the root holds no facts
    CHECK (supersedes_id <> id),
    CHECK ((embedding IS NULL) = (embedding_model IS NULL))
);

-- The document view of one path, oldest first.
CREATE INDEX facts_path_date_idx ON facts(path_id, created_at) WHERE deleted_at IS NULL;

-- The queue that the embedding backfill reads.
CREATE INDEX facts_pending_embedding_idx ON facts(id) WHERE embedding IS NULL AND deleted_at IS NULL;

CREATE INDEX facts_supersedes_idx ON facts(supersedes_id) WHERE supersedes_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- The keyword index
-- ---------------------------------------------------------------------------

-- An external content index: the text lives in `facts` only, and the triggers
-- below keep the index in step.
CREATE VIRTUAL TABLE facts_fts USING fts5(
    content,
    content='facts',
    content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER facts_fts_insert AFTER INSERT ON facts BEGIN
    INSERT INTO facts_fts(rowid, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER facts_fts_delete AFTER DELETE ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, content) VALUES ('delete', old.id, old.content);
END;

-- Facts do not change their text, but a repair job might. The trigger keeps
-- the index correct if that happens.
CREATE TRIGGER facts_fts_update AFTER UPDATE OF content ON facts BEGIN
    INSERT INTO facts_fts(facts_fts, rowid, content) VALUES ('delete', old.id, old.content);
    INSERT INTO facts_fts(rowid, content) VALUES (new.id, new.content);
END;

-- ---------------------------------------------------------------------------
-- The tags
-- ---------------------------------------------------------------------------

-- One key holds one value. To give a key two values, use two keys.
CREATE TABLE fact_tags (
    fact_id INTEGER NOT NULL REFERENCES facts(id) ON DELETE CASCADE,
    key     TEXT    NOT NULL,
    value   TEXT    NOT NULL,
    PRIMARY KEY (fact_id, key),
    CHECK (length(key) > 0 AND length(value) > 0)
) WITHOUT ROWID;

CREATE INDEX fact_tags_lookup_idx ON fact_tags(key, value);

-- Tags on a path apply to every fact below it.
CREATE TABLE path_tags (
    path_id INTEGER NOT NULL REFERENCES paths(id) ON DELETE CASCADE,
    key     TEXT    NOT NULL,
    value   TEXT    NOT NULL,
    PRIMARY KEY (path_id, key),
    CHECK (length(key) > 0 AND length(value) > 0)
) WITHOUT ROWID;

CREATE INDEX path_tags_lookup_idx ON path_tags(key, value);

-- Each path together with itself and every path above it.
CREATE VIEW path_ancestry (path_id, ancestor_id, distance) AS
WITH RECURSIVE walk(path_id, ancestor_id, distance) AS (
    SELECT id, id, 0 FROM paths
    UNION ALL
    SELECT w.path_id, p.parent_id, w.distance + 1
    FROM walk w
    JOIN paths p ON p.id = w.ancestor_id
    WHERE p.parent_id IS NOT NULL
)
SELECT path_id, ancestor_id, distance FROM walk;

-- The tags of a path after inheritance. The nearest path that sets a key
-- decides its value.
CREATE VIEW effective_path_tags (path_id, key, value) AS
SELECT a.path_id, t.key, t.value
FROM path_ancestry a
JOIN path_tags t ON t.path_id = a.ancestor_id
WHERE a.distance = (
    SELECT MIN(a2.distance)
    FROM path_ancestry a2
    JOIN path_tags t2 ON t2.path_id = a2.ancestor_id
    WHERE a2.path_id = a.path_id AND t2.key = t.key
);

-- The tags that decide access for one fact. A tag on the fact wins over a tag
-- that the fact takes from its paths.
CREATE VIEW effective_fact_tags (fact_id, key, value) AS
SELECT ft.fact_id, ft.key, ft.value
FROM fact_tags ft
UNION ALL
SELECT f.id, ep.key, ep.value
FROM facts f
JOIN effective_path_tags ep ON ep.path_id = f.path_id
WHERE NOT EXISTS (
    SELECT 1 FROM fact_tags ft WHERE ft.fact_id = f.id AND ft.key = ep.key
);

-- ---------------------------------------------------------------------------
-- The access rules
-- ---------------------------------------------------------------------------

-- The Casbin policy store. The column names follow the Casbin adapters, so
-- the standard tooling can read this table.
--
--   p, <subject>, <object>, <action>, <effect>
--   g, <subject>, <role>
--
-- The object is `path:/work/*` or `tag:visibility=public`.
CREATE TABLE casbin_rule (
    id    INTEGER PRIMARY KEY,
    ptype TEXT NOT NULL,
    v0    TEXT NOT NULL DEFAULT '',
    v1    TEXT NOT NULL DEFAULT '',
    v2    TEXT NOT NULL DEFAULT '',
    v3    TEXT NOT NULL DEFAULT '',
    v4    TEXT NOT NULL DEFAULT '',
    v5    TEXT NOT NULL DEFAULT '',
    UNIQUE (ptype, v0, v1, v2, v3, v4, v5)
);

CREATE INDEX casbin_rule_ptype_idx ON casbin_rule(ptype, v0);
