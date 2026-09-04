---
icon: lucide/bot
---

# The harness

Embornal holds the index and says what waits. An outside agent writes the
summaries. This page says how the two meet.

There is no model in the binary, no key in the configuration, and no provider
to choose. The agent that already reads your code writes the text.

## The loop

``` bash
embornal code index

# until `next` gives nothing:
#   1. embornal code next --json
#   2. open the file that it names and read the lines
#   3. embornal code describe --stdin
```

## What `next` gives

``` json
{
  "kind": "file",
  "collection": "/home/you/projects/thing",
  "rel_path": "src/memory/api.rs",
  "language": "rust",
  "nodes": [
    {
      "id": "01K7...",
      "kind": "function",
      "name": "recall",
      "qualified_name": "src/memory/api.rs::Memory::recall",
      "lines": [473, 786]
    }
  ]
}
```

**The payload holds no source.** It says which file and which lines, and the
agent opens the file with the tools that it has. To put the source in the JSON
would send it through the context twice.

A batch of a directory carries no lines. There is no file to open: the
summaries of the children are the whole of the material.

``` json
{
  "kind": "dir",
  "rel_path": "src/memory",
  "nodes": [
    {
      "id": "01K8...",
      "kind": "dir",
      "name": "memory",
      "children": [
        { "name": "api.rs", "kind": "file", "summary": "..." }
      ]
    }
  ]
}
```

## What `describe` takes

A JSON array on standard input. Multi-line text does not survive the quoting
of a shell, so a batch comes this way.

``` console
$ embornal code describe --stdin <<'JSON'
[
  {
    "id": "01K7...",
    "summary": "Finds the facts that answer a question.",
    "description": "The search of the Memory type, which every read of the memory goes through: the CLI (src/cli/memory.rs) and the HTTP server (src/api.rs) both call it. It asks the keyword index and the vector index, mixes what they give with the strength of each fact, and gives back the best of them. Use it to answer a question; use cat to read one path."
  }
]
JSON
described 1
```

## What to write

| Field | Shape |
| ----- | ----- |
| `summary` | One line, about 140 characters. What the code does. |
| `description` | A paragraph that stands on its own. |

Write in English. One language in the vector index keeps the search good.

### The description

Write it for a reader who has not opened the file and will not open it. That
reader met the node in the answer to a search, and this paragraph is all that
they get. It must answer three things.

**What it belongs to.** Name the type, the module or the flow that holds it. A
description of the line alone tells the reader nothing that the name did not.

**Where it is used.** Name the files that use it. The index keeps no call
graph, so it cannot give you this: search the repository before you write.

**When to reach for it.** Say which problem it solves, so that the reader
knows whether this is the one that they want.

Do not write:

> function that adds nodes to a reusable linked list

Write:

> This function is part of the generic linked list type. The type is used for
> user lists (`users.rs`) and for processes (`process.rs`). Use this type when
> you need a linked list.

The second one names what holds the function, where the type is used, and when
to reach for it. The first one says the name again in more words.

Use concrete facts from the body: names, defaults, limits, the file that a
default path points at.

!!! note "Call sites go stale in silence"

    A node comes back into the queue when its own bytes move. A file that
    starts to use the linked list does not move the bytes of the linked list,
    so a description that names the callers is right when it is written and
    ages after that. This is the price of naming them, and it is worth paying:
    a description with no call site helps nobody, and the index reopens the
    node the next time the code itself changes.

## The guard

The `id` of a node is the token of the work.

A pass of `index` replaces every node of a file that changed, and the ids of
the old nodes go with them. A `describe` that arrives against one of those is
refused:

``` console
$ embornal code describe --stdin < written.json
error: there is no node 01K7... in this collection
```

Read the file again and take a new batch. The index never writes a summary
against bytes that the summary did not describe.

## To teach an agent

``` bash
embornal code bootstrap >> ~/.claude/AGENTS.md
```

`embornal bootstrap` writes the instructions of every tool. `embornal memory
bootstrap` writes the memory alone.
