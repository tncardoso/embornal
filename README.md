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

More tools come later. Each one is a subcommand of the same binary and keeps
its data in `$HOME/.embornal`.

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

$ embornal memory serve
```

`serve` starts a wiki on `http://localhost:1337`, where each path is a page.

To teach an agent to use the memory, write the instructions to a skill file:

```bash
embornal skill > .claude/skills/memory/SKILL.md
```

## Docs

- **Start here:** [Overview](docs/index.md), [Installation](docs/installation.md)
- **Use the memory:** [Memory](docs/memory/index.md), [The memory commands](docs/memory/cli.md)
- **Read the design:** [The memory model](docs/memory/model.md), [Embeddings](docs/memory/embeddings.md)
- **Ship a version:** [Releasing](docs/releasing.md), [CHANGELOG.md](CHANGELOG.md)

## License

MIT — see [LICENSE](LICENSE).
