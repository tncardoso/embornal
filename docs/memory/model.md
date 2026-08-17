# The memory model

The memory is a wiki of small facts. An agent writes one fact at a time and
finds it again by keyword, by meaning, or by how strong the fact still is.

The memory keeps its files in three directories, because the files are of
three kinds:

| File                              | Directory                  | Content |
| --------------------------------- | -------------------------- | ------- |
| `embornal/config.yaml`            | `$XDG_CONFIG_HOME`         | The settings |
| `embornal/memory.db`              | `$XDG_DATA_HOME`           | The tree, the facts, the two indexes and the access rules |
| `embornal/models/`                | `$XDG_CACHE_HOME`          | The weights of the embedding model |

A backup must hold `memory.db`. The weights are a download that the tool makes
again, so a backup can leave them out.

If the directories have no value, they fall back to `~/.config`, `~/.local/share`
and `~/.cache`.

macOS reads no `XDG` variable. It puts `config.yaml` and `memory.db` in
`~/Library/Application Support/embornal`, and the weights in
`~/Library/Caches/embornal`.

`EMBORNAL_HOME` puts the three in one directory.

An older build kept all the files in `$HOME/.embornal`. The first command of
this build moves `config.yaml` and `memory.db` out of that directory and says
which files it moved. It moves a file only if the new place is still empty, so
a memory that is already in the new place stays as it is. The weights stay
where they are.

## Paths

A path names one topic, for example `/projects/embornal`.

The memory folds each path to lowercase. A segment starts with a letter or a
digit, and it holds letters, digits, `.`, `_` and `-`. This rule stops
`/Projects` and `/projects` from becoming two nodes with one half of the same
knowledge each.

The `paths` table holds one row for each node, and each row points at its
parent. Row 1 is the root, and it is the only row with no parent. A list of
one level is thus a query on `parent_id`.

A path holds structure only. Text about the path is a fact of that path.

The migration writes `/memory`. That path holds the facts about the memory
itself, so an agent that reads it learns how to use the rest of the tree.

## Facts

A fact is one small statement that belongs to a path.

A fact does not change. To correct a fact, write a new fact and point
`supersedes_id` at the old one. To remove a fact, set `deleted_at`. The
history of what the memory believed stays readable.

Each fact holds the name of the subject that wrote it, in the `owner` column.
The memory writes that name. A writer cannot give it, because the access rules
read it: see [Who owns a fact](#who-owns-a-fact).

Content can hold a link in the `[[/path]]` form. The memory stores the text as
it comes and reads the links when it shows the fact.

## Signal strength

The strength of a fact falls with time:

```
strength = exp(-elapsed_days / stability_days)
```

A new fact has a stability of one day. Each recall lifts the stability, and it
lifts it more when the fact was almost lost:

```
stability = stability * (1 + 2.75 * (1 - strength))
```

A recall one minute after the last one adds almost nothing. A recall of a fact
that nobody read for a month multiplies the stability by 3.75. This is the
spacing effect, and it needs no grade from the reader: the recall itself is the
reinforcement.

The gain of 2.75 is calibrated on this scenario: a fact that the reader recalls
on day 2, day 8, day 30 and day 90 reaches a stability of about 120 days. Two
months after that last recall the fact still holds 60 percent of its strength.

The database holds `created_at`, `last_recall_at`, `recall_count` and
`stability_days`. It does not hold the strength, because the strength is a
function of the clock.

## Search

The memory holds two indexes:

- `facts_fts` is an FTS5 index for keywords. The text stays in `facts`, and
  triggers keep the index in step.
- `facts_vec` is a vector index. Its width comes from `config.yaml` and
  defaults to 768. The width is written into the `meta` table. If the two
  disagree later, the memory stops instead of giving wrong answers.

`store` fills the `embedding` column at the moment that it writes the fact. The
column stays empty when the memory has no model, or when the model failed. The
`facts_pending_embedding_idx` index holds that queue, and `embornal memory
reindex` reads it. See [embeddings](embeddings.md).

## Tags and access control

A tag is a `key=value` pair. Tags sit on facts and on paths. A fact takes the
tags of each path above it, and the nearest tag wins. A tag on the fact beats
every tag that the fact inherits. The `effective_fact_tags` view does this
work.

Casbin decides who can read, write and delete. The model is built into the
binary; the policies live in the `casbin_rule` table of the same file. A policy
object names a place in the tree or an attribute:

```
p, cli, path:/work/acme/*,     read,   allow
p, cli, tag:visibility=public, read,   allow
p, cli, path:/secrets/*,       read,   deny
g, cli, reader
```

A subtree pattern holds its own root: `path:/work/*` covers `/work` as well.
A deny beats each allow. No policy means no access.

A read does not test each fact. It asks Casbin for the permissions of the
subject, turns them into one `WHERE` fragment and lets the database drop what
the subject must not see.

A new database gives the `cli` subject full access to the whole tree, so a
memory on one machine works with no policy of its own.

### Who owns a fact

The `owner` column of a fact holds the subject that wrote it. The same name
goes into the `owner` tag of that fact, because the access rules read tags,
not columns. The column is the record; the tag is the form that Casbin reads.

The memory writes both. A `store` that carries an `owner` tag stops, and the
fact does not reach the memory. A tag with the key `owner` on a path does not
reach a fact either, because a tag on the fact beats a tag that it inherits.

This lets one rule give a subject its own facts and nothing more:

```
p, alice, tag:owner=alice, read, allow
```

The facts that the memory holds about itself belong to the subject `system`.
Every subject reads them through the `everyone` role:

```
g, alice,    everyone
p, everyone, tag:owner=system, read, allow
```

A subject name becomes the value of a tag, so it holds no space, no `=` and
no `,`.
