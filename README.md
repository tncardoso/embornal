# Embornal

A toolkit for agents. Embornal gives an agent a memory: a wiki of small facts
that it writes one at a time and finds again by word, by sense, or by how
strong a fact still is.

Everything lives in one SQLite file. No server, no network call at the time of
a search, and no key of another company.

## Install

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

See [the installation guide](docs/installation.md) for a fixed version,
another directory, and how to remove Embornal.

## Use

```console
$ embornal memory store /projects/embornal "The memory lives in SQLite."
01K19XQ7M2W8Z4YB3NAV6DTCE0 /projects/embornal

$ embornal memory recall "where do my notes live"
| Path                | Signal | Fact                        |
+---------------------+--------+-----------------------------+
| /projects/embornal  |  1.000 | The memory lives in SQLite. |

$ embornal memory tree /projects
/projects
└── embornal*

$ embornal memory serve
```

`serve` starts a wiki on `http://localhost:1337`, where each path is a page.

To teach an agent to use the memory, write the instructions to a skill file:

```bash
embornal skill > .claude/skills/memory/SKILL.md
```

## How it works

- **Facts, not documents.** A fact is short and immutable. A new fact
  supersedes an old one, and the old one stays in the history.
- **Two indexes.** SQLite FTS5 finds the facts that hold the words.
  EmbeddingGemma 300M, which runs in this process, finds the facts that hold
  the sense — so a fact answers a question that shares no word with it.
- **A signal that decreases.** Each fact loses strength with time and gains it
  back when somebody reads it. What the agent uses stays at the top.
- **Access control.** Tags on a path and on a fact, and Casbin rules, decide
  who reads what. A fact that a subject may not read does not appear at all.

## Documentation

| Guide | Content |
| ----- | ------- |
| [Installation](docs/installation.md) | How to install, update and remove Embornal |
| [The memory commands](docs/memory-cli.md) | Each command and each flag |
| [The memory model](docs/memory-model.md) | Paths, facts, signal strength and access rules |
| [Embeddings](docs/embedding.md) | The model, the weights, and how a search mixes the two indexes |
| [Releasing](docs/releasing.md) | How a new version reaches the users |

## License

MIT. See [LICENSE](LICENSE).
