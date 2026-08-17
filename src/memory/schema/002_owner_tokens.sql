-- Migration 002: who wrote a fact, and who may ask.
--
-- Until this migration a memory held one subject, so no fact needed to say
-- who wrote it. A memory that many people share needs that name, and it needs
-- a way to tell one caller from another.

-- ---------------------------------------------------------------------------
-- The owner of a fact
-- ---------------------------------------------------------------------------

-- The subject that wrote the fact. The column is the truth, and the `owner`
-- tag below is the same name in the form that the access rules read.
--
-- The column accepts NULL, because a table with a fact in it cannot take a
-- NOT NULL column. The value below fills every row that this migration finds,
-- and `store` writes the value for each row that comes after it.
ALTER TABLE facts ADD COLUMN owner TEXT;

CREATE INDEX facts_owner_idx ON facts(owner) WHERE deleted_at IS NULL;

-- The facts that the memory holds about itself belong to the memory. Every
-- subject reads them, through the `everyone` role.
UPDATE facts
   SET owner = 'system'
 WHERE path_id IN (SELECT id FROM paths WHERE full_path = '/memory');

-- Every other fact comes from the one subject that could write before this
-- migration.
UPDATE facts SET owner = 'cli' WHERE owner IS NULL;

-- The access rules read tags, not columns, so each fact carries its owner as
-- a tag as well. A tag on the fact wins over a tag that the fact takes from a
-- path, so no path tag can take a fact from its owner.
INSERT OR REPLACE INTO fact_tags(fact_id, key, value)
SELECT id, 'owner', owner FROM facts WHERE owner IS NOT NULL;

-- Every subject that this memory already knows joins the role that reads the
-- facts of the memory itself.
INSERT OR IGNORE INTO casbin_rule(ptype, v0, v1)
SELECT DISTINCT 'g', v0, 'everyone' FROM casbin_rule WHERE ptype = 'p' AND v0 <> '';

INSERT OR IGNORE INTO casbin_rule(ptype, v0, v1, v2, v3)
VALUES ('p', 'everyone', 'tag:owner=system', 'read', 'allow');

-- ---------------------------------------------------------------------------
-- The tokens
-- ---------------------------------------------------------------------------

-- One token lets one subject reach the server. The secret itself is never
-- here: the table holds its SHA-256, so a copy of this file gives nobody a
-- way in.
--
-- The secret is 32 random bytes, so a hash with no salt and no cost is
-- enough. A password needs more because a person chooses it.
CREATE TABLE tokens (
    id           INTEGER PRIMARY KEY,
    -- The public name of the token. `token ls` shows it, and `token revoke`
    -- reads it.
    ulid         TEXT NOT NULL UNIQUE,
    subject      TEXT NOT NULL,
    hash         TEXT NOT NULL UNIQUE,
    name         TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL,
    expires_at   TEXT,
    last_used_at TEXT,
    revoked_at   TEXT,

    CHECK (length(subject) > 0),
    CHECK (length(hash) = 64)
);

CREATE INDEX tokens_subject_idx ON tokens(subject);
