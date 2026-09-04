---
icon: lucide/network
---

# The code model

This page says why the index has the shape that it has. To use the tool, read
[The code commands](cli.md) first.

## One goal

**A pass after a commit must cost almost nothing.** Parsing a file takes
milliseconds. Writing a summary takes an agent, a model and money. Everything
below exists so that the second one happens as rarely as it can.

## The tree

```
repo → dir → file → module | class | impl | function
```

A grammar gives a flat list of definitions with their spans. What holds what
comes from the spans alone: a definition inside the bytes of another is its
child. This is true in every language, so the tree needs no rule per grammar
and no query for scope.

A node answers to a **qualified name**: the path of the file and the chain of
definitions, such as `src/memory/api.rs::Memory::recall`. Two nodes of one
collection cannot share one.

## One hash for each node

| Node | What it hashes |
| ---- | -------------- |
| `function`, `impl`, `class`, `module`, `file` | The bytes of its own span |
| `dir`, `repo` | The hashes of its children |

A directory and the root are the only nodes with no bytes of their own, and
they are therefore the only place where the tree is a Merkle tree. Below a
file, that structure is not needed: the bytes of a file already move when any
definition inside it moves.

The cost of this is small and known. A change that only moves whitespace — a
formatter, or moving functions into a new module — moves the hash of each node
that it touched, although the code does the same thing as before.

## The pool of summaries

**A summary belongs to the code, and not to the node.** It is filed under a
key:

```
pool_key = sha256(qualified_name ‖ content_hash)
```

The table that holds the summaries names no collection and no repository. A
node points at it with this key.

Both halves of the key are needed:

- The hash alone would let a body that says nothing on its own —
  `Self::default()`, and the fifty places that hold exactly that — take the
  description of whichever of them an agent read first.
- The name alone would let a summary outlive the code that it describes.

What this buys:

- To change branch and come back costs nothing.
- A second collection over the same code starts with nothing to do, and no row
  is copied to make that true.
- The same code in another checkout answers with the summary that is written.

What it costs: to rename or to move a definition changes its qualified name,
and its summary must be written again.

## Stale is a question, not a flag

A node is stale when, and only when, its `pool_key` is absent from the pool:

``` sql
SELECT n.* FROM nodes n
LEFT JOIN summaries s ON s.pool_key = n.pool_key
WHERE n.collection_id = ? AND s.id IS NULL
```

There is no column that says "this is old" and no date to compare. The index
therefore cannot reach the state where it says that a summary is current while
the summary describes other bytes.

## The queue

One batch is one file, and it holds every node of that file that waits, the
deepest first and the file last. An agent that described one function at a time
would open the file once for each function in it, and the sibling functions
would tell it nothing.

Files come before directories, because the summary of a directory is a summary
of what it holds.

**The root waits only when it is asked for.** The hash of the root follows
every file of the repository, so it comes back after every commit. In the queue
it would mean that the queue never empties, and a queue that never empties says
nothing.

## What the index does not do

- **No call graph.** The index computes no "where is it used". A grammar can
  list references, but the answer is a name that matches, and not a name that
  is resolved. An agent that searches the repository does it better.

    A description is asked to name the files that use the code it describes,
    and the agent finds those by searching before it writes. See
    [The harness](harness.md). What that costs is known: a node comes back
    into the queue when its own bytes move, and a new caller does not move
    them, so the call sites that a description names age in silence until the
    code itself changes.
- **No summary of its own.** Embornal calls no model. See
  [The harness](harness.md).
- **No pass of its own.** `index` runs when somebody asks. An index that moved
  under a command would change the queue while an agent reads it.
