# 🧰 Embornal

![Embornal banner](docs/assets/banner.png)

[![CI](https://github.com/tncardoso/embornal/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/tncardoso/embornal/actions/workflows/ci.yml)
[![Release](https://github.com/tncardoso/embornal/actions/workflows/release.yml/badge.svg)](https://github.com/tncardoso/embornal/actions/workflows/release.yml)
[![Latest release](https://badgen.net/github/release/tncardoso/embornal/stable)](https://github.com/tncardoso/embornal/releases/latest)

Embornal is a set of tools for agents. Tools are for
the people who build agents, and for the agents themselves: a coding assistant,
a background worker, or a script that calls a model in a loop.

## The tools

| Tool | Command | Job |
| ---- | ------- | --- |
| [Memory](docs/memory/index.md) | `embornal memory` | A wiki of small facts that an agent writes one at a time and finds again by word, by sense, or by how strong a fact still is. |
| [Code](docs/code/index.md) | `embornal code` | A map of a repository: a tree of directories, files and definitions, with a summary of each that an outside agent writes. |

More tools come later. Each one is a subcommand of the same binary and keeps
its data below `$XDG_DATA_HOME`.

## Memory

An agent forgets everything at the end of a session. A file of notes does not
help much, because the agent must read the whole file to find one line, and the
file grows until it does not fit.

The memory answers with the few facts that the question needs:

```console
$ embornal memory store /projects/embornal "The memory lives in SQLite."
01K19XQ7M2W8Z4YB3NAV6DTCE0 /projects/embornal

$ embornal memory recall "where do my notes live"
| Path                | Signal | Fact                        |
+---------------------+--------+-----------------------------+
| /projects/embornal  |  1.000 | The memory lives in SQLite. |
```

- **Facts, not documents.** A fact is short and it does not change. A new fact
  supersedes an old one, and the old one stays in the history.
- **A tree of paths.** A path such as `/projects/embornal` names one topic. A
  fact belongs to a path, and a fact can link to another path with `[[/path]]`.
- **Two indexes.** SQLite FTS5 finds the facts that hold the words.
  EmbeddingGemma 300M, which runs in this process, finds the facts that hold
  the sense.
- **A signal that decreases.** Each fact loses strength with time and gains it
  back when somebody reads it. What the agent uses stays at the top.
- **Access control.** Tags on a path and on a fact, and Casbin rules, decide
  who reads what. A fact that a subject may not read does not appear at all.

Read [Memory](docs/memory/index.md) for the whole tool, and
[The memory model](docs/memory/model.md) for the design behind it.

## Get started

Install the binary:

```bash
curl -LsSf https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.sh | sh
```

On Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.ps1 | iex"
```

Or with Cargo, which builds llama.cpp and thus needs cmake and a C++ compiler:

```bash
cargo install embornal
```

The installer writes the binary to `~/.local/bin` and adds that directory to
the `PATH` of the shell. Open a new shell, then check it:

```console
$ embornal --version
embornal 0.1.0
```

See [Installation](docs/installation.md) for a fixed version, another
directory, the platforms with a binary, and how to remove Embornal.

Then write a fact, read the tree, and open the wiki:

```console
$ embornal memory store /work/acme "The build needs Rust 1.85."
$ embornal memory tree /work
/work
└── acme*

$ embornal memory wiki
```

`wiki` starts a wiki on `http://localhost:1337`, where each path is a page.

To share one memory between machines, put it on a server and point the others
at it:

```console
$ embornal token add alice --name laptop   # on the machine that holds it
$ embornal serve
```

The client then needs only the address and the token in its `config.yaml`, and
no embedding model at all. See [The server](docs/memory/server.md).

To teach an agent to use the memory, add the instructions to the global AGENTS.md:

```bash
embornal bootstrap >> AGENTS.md
```

## Code

An agent that arrives in a repository has no map. It reads with `grep` until it
knows what each module does, and it does that work again in the next session.

`embornal code` builds the map with tree-sitter and keeps it current:

```console
$ embornal code index
collection /home/you/projects/thing: 47 files, 47 parsed, 1422 nodes, 1422 stale

$ embornal code recall "where the token is checked"
| Score | Kind     | Name                             | Summary                          |
+-------+----------+----------------------------------+----------------------------------+
| 1.842 | function | src/memory/token.rs::Token::open | Compares a secret with the stored |
```

- **Embornal writes no summary.** It says which nodes have none; an outside
  agent writes them with `code next` and `code describe`. No model, no key and
  no provider goes in the binary.
- **A pass after a commit is almost free.** A file whose bytes hash to what the
  index holds is not read again. Parsing is cheap; writing a summary is not.
- **A summary belongs to the code, not to the checkout.** To change branch and
  come back costs nothing, and a second index over the same code starts with
  nothing to do.
- **Six grammars.** Rust, Python, Go, JavaScript, TypeScript and TSX.

Read [Code](docs/code/index.md) for the whole tool, [The harness](docs/code/harness.md)
for the loop that writes the summaries, and [The code model](docs/code/model.md)
for the design behind it.

## Docs

- **Start here:** [Overview](docs/index.md), [Installation](docs/installation.md)
- **Use the memory:** [Memory](docs/memory/index.md), [The memory commands](docs/memory/cli.md), [The server](docs/memory/server.md)
- **Use the code index:** [Code](docs/code/index.md), [The code commands](docs/code/cli.md), [The harness](docs/code/harness.md)
- **Read the design:** [The memory model](docs/memory/model.md), [Embeddings](docs/memory/embeddings.md), [The code model](docs/code/model.md)
- **Ship a version:** [Releasing](docs/releasing.md), [CHANGELOG.md](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).
