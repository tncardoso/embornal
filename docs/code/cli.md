---
icon: lucide/terminal
---

# The code commands

Each command works on one **collection**: an index of one repository.

Embornal finds the repository by itself. It walks up from where you are until
it finds a `.git`, and it stops before your home directory — some people keep
their dotfiles in a repository, and to take that as a root would index
everything you own. With no `.git` anywhere, the directory you are in is the
root.

The name of a collection is the path of the root, so a repository has one index
and nobody must name it. `--collection` gives another name over the same
repository, which is a fork: it keeps its own tree and shares every summary
that is written.

All of it lives in one `code.db` below `$XDG_DATA_HOME`. One file holds every
repository.

## `embornal code index`

Reads the repository and brings the index up to date.

``` console
$ embornal code index
collection /home/you/projects/thing: 47 files, 47 parsed, 0 removed, 1422 nodes, 1422 stale

$ embornal code index
collection /home/you/projects/thing: 47 files, 0 parsed, 0 removed, 1422 nodes, 1422 stale
```

The second pass reads no file: each hash agreed with what the index held.

| Flag | Job |
| ---- | --- |
| `--all` | Reads every file again, whatever its hash says. Use it after a change to a grammar. |
| `--path PATH` | Starts the walk somewhere else. |
| `--collection NAME` | Works on another index of the same repository. |

The walk follows `.gitignore`, passes over a hidden directory, and passes over
a file above `code.max_file_bytes` (512 KB by default).

The command runs only when you ask. Nothing else starts it.

## `embornal code status`

``` console
$ embornal code status
| Kind     | Nodes | Stale |
+----------+-------+-------+
| function |  1043 |  1043 |
| class    |   141 |   141 |
| file     |    47 |    47 |
```

## `embornal code next`

Gives the next file to describe. `--json` writes what an agent reads. See
[The harness](harness.md).

`--update-root` adds the root of the repository to the queue. Without it the
root stays out, because its hash follows every file and it would come back
after every commit.

## `embornal code describe`

Takes the summaries that an agent wrote.

``` console
$ embornal code describe --stdin < written.json
$ embornal code describe 01K7... --summary "..." --description "..."
```

`--as-subject` says who wrote them. The index keeps the name and the time.

## `embornal code tree`

``` console
$ embornal code tree src/code --depth 1
src/code*
├── api.rs*
├── db.rs*
└── parse.rs
```

A `*` marks a node that still waits for a summary.

## `embornal code cat`

``` console
$ embornal code cat 'src/cli/tree.rs::print_tree'
# src/cli/tree.rs::print_tree
function src/cli/tree.rs:36-39

Prints a tree.

Writes the top line, then one line for each node below it, with an elbow that
says whether a node is the last of its level.

-- default, 2026-09-03T18:50:41Z
```

The name is the qualified name, or the id that `next` gave.

## `embornal code recall`

``` console
$ embornal code recall "how a tree is drawn"
| Score | Kind     | Name                        | Summary          |
+-------+----------+-----------------------------+------------------+
| 1.842 | function | src/cli/tree.rs::print_tree | Prints a tree.   |
```

| Flag | Job |
| ---- | --- |
| `--limit N` | How many answers to give. |
| `--kind KIND` | Keep one kind: `function`, `class`, `impl`, `module`, `file`, `dir`. |

Two indexes answer. FTS5 finds the summaries that hold the words, and the
vector index finds the ones that hold the sense. What is absent here, and is
present in the memory, is a third term for age: a fact grows old, and a summary
of code does not. A summary is right until the code moves, and a moved hash
says that at once.

## `embornal code bootstrap`

Writes the instructions that teach an agent the loop.

``` bash
embornal code bootstrap >> ~/.claude/AGENTS.md
```

## Configuration

``` yaml
code:
  max_file_bytes: 524288
  limit: 20
  keyword_weight: 1.0
  keyword_ceiling: 0.5
  vector_weight: 1.0
  vector_floor: 0.15
  vector_share: 0.5
```
