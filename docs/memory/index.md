---
icon: lucide/brain
---

# Memory

The memory is the first tool of Embornal. It is a wiki of small facts. An
agent writes one fact at a time and finds it again by keyword, by meaning, or
by how strong the fact still is.

All of it lives in one SQLite file below `$XDG_DATA_HOME`. A memory works
alone, and a search asks the network nothing. A memory that many agents share
is in [The server](server.md).

## Why an agent needs it

An agent forgets everything at the end of a session. A file of notes does not
help much, because the agent must read the whole file to find one line, and
the file grows until it does not fit.

The memory answers with the few facts that the question needs:

``` console
$ embornal memory recall "where do my notes live"
| Path                | Signal | Fact                        |
+---------------------+--------+-----------------------------+
| /projects/embornal  |  1.000 | The memory lives in SQLite. |
```

## How it works

- **Facts, not documents.** A fact is short and it does not change. A new fact
  supersedes an old one, and the old one stays in the history.
- **A tree of paths.** A path such as `/projects/embornal` names one topic. A
  fact belongs to a path, and a fact can link to another path with `[[/path]]`.
- **Two indexes.** SQLite FTS5 finds the facts that hold the words.
  EmbeddingGemma 300M, which runs in this process, finds the facts that hold
  the sense. A fact thus answers a question that shares no word with it.
- **A signal that decreases.** Each fact loses strength with time and gains it
  back when somebody reads it. What the agent uses stays at the top.
- **Access control.** Tags on a path and on a fact, and Casbin rules, decide
  who reads what. A fact that a subject may not read does not appear at all.

## The commands

``` console
$ embornal memory store /work/acme "The build needs Rust 1.85."
$ embornal memory recall "which rust version"
$ embornal memory ls /work
$ embornal memory tree /projects
$ embornal memory cat /memory
$ embornal memory wiki
```

`wiki` starts the wiki on `http://localhost:1337`, where each path is a page.

Each command and each flag is in [The memory commands](cli.md).

## More than one machine

A memory can live on a server. The commands then do their work there, and the
machine that asks keeps no facts and needs no model. See
[The server](server.md).

## Teach an agent

``` bash
embornal skill > .claude/skills/memory/SKILL.md
```

The command writes the instructions that teach an agent to use the memory. It
touches no file of its own, so it answers before a memory exists.

!!! tip

    Tell the agent to read `/memory` at the start of a session. That path
    holds the facts about the memory itself, so the agent learns how to use
    the rest of the tree.

## Read more

| Page | Content |
| ---- | ------- |
| [The memory commands](cli.md) | Each command and each flag |
| [The memory model](model.md) | Paths, facts, signal strength and access rules |
| [Embeddings](embeddings.md) | The model, the weights, and how a search mixes the two indexes |
