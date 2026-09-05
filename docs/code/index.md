---
icon: lucide/file-code
---

# Code

The code index is the second tool of Embornal. It holds a tree of one
repository — the directories, the files, and the definitions that a grammar
finds inside each file — and a summary of each of those.

**Embornal writes no summary.** It says which nodes have none. An outside
agent writes them and gives them back. No model, no key, and no provider goes
in the binary.

## Why an agent needs it

An agent that arrives in a repository has no map. It reads with `grep` until
it knows what each module does, and it does that work again in the next
session.

The index answers with the code that the question needs:

``` console
$ embornal code recall "where the token is checked"
| Score | Kind     | Name                              | Summary                              |
+-------+----------+-----------------------------------+--------------------------------------+
| 1.842 | function | src/memory/token.rs::Token::open  | Compares a secret with the stored... |
```

## How it works

- **A tree from the spans.** tree-sitter lists the definitions of a file. A
  definition whose bytes sit inside the bytes of another is its child. Every
  grammar agrees on that, so no language needs a rule of its own.
- **One hash for each node.** A file and everything below it hash their own
  bytes. A directory holds no bytes, so it hashes the hashes of its children.
- **A pass that costs almost nothing.** A file whose bytes hash to what the
  index holds is not read again, and nothing below it moves. Parsing is cheap;
  writing a summary is not.
- **A pool of summaries.** A summary belongs to the code, and not to the
  checkout that reached it first. Two branches of one repository share what is
  already written.
- **Two indexes.** SQLite FTS5 finds the summaries that hold the words.
  The embedding model finds the summaries that hold the sense.

## The languages

Rust, Python, Go, JavaScript, TypeScript and TSX. A file that no grammar reads
stays out of the index. A file that a grammar cannot read becomes one node with
no child, because an index must not claim a shape that it could not read.

## First commands

``` console
$ embornal code index
collection /home/you/projects/thing: 47 files, 47 parsed, 0 removed, 1422 nodes, 1422 stale

$ embornal code next --json
{ "kind": "file", "rel_path": "src/cli/tree.rs", "nodes": [ ... ] }

$ embornal code describe --stdin < written.json
described 17

$ embornal code recall "how a tree is drawn"
```

`embornal dashboard` shows the same tree, the same summaries, and the same
search in a browser, under a "Code" tab next to the wiki.

To teach an agent the loop, add the instructions to the global AGENTS.md:

``` bash
embornal code bootstrap >> AGENTS.md
```

## Where to go

- **Use it:** [The code commands](cli.md)
- **Write the summaries:** [The harness](harness.md)
- **Read the design:** [The code model](model.md)
