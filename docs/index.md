---
icon: lucide/toolbox
---

# Embornal

![Embornal](assets/banner.png)

Embornal is a set of tools for agents. Each tool is a command of one binary,
and each tool does one job that an agent cannot do on its own.

Everything stays on the machine. There is no server, no network call at the
time of a search, and no key of another company.

## The tools

| Tool | Command | Job |
| ---- | ------- | --- |
| [Memory](memory/index.md) | `embornal memory` | A wiki of small facts that an agent writes one at a time and finds again by word, by sense, or by how strong a fact still is. |

## Install

=== "Linux, macOS"

    ``` bash
    curl -LsSf https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.sh | sh
    ```

=== "Windows"

    ``` powershell
    powershell -ExecutionPolicy Bypass -c "irm https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.ps1 | iex"
    ```

=== "Cargo"

    ``` bash
    cargo install embornal
    ```

The Cargo build compiles llama.cpp, so the machine needs cmake and a C++
compiler. See [Installation](installation.md) for a fixed version, another
directory, and how to remove Embornal.

## First commands

``` console
$ embornal memory store /projects/embornal "The memory lives in SQLite."
01K19XQ7M2W8Z4YB3NAV6DTCE0 /projects/embornal

$ embornal memory recall "where do my notes live"
| Path                | Signal | Fact                        |
+---------------------+--------+-----------------------------+
| /projects/embornal  |  1.000 | The memory lives in SQLite. |
```

To teach an agent to use the memory, write the instructions to a skill file:

``` bash
embornal skill > .claude/skills/memory/SKILL.md
```

## Where to go

- **Install it:** [Installation](installation.md)
- **Use the memory:** [Memory](memory/index.md), [The memory commands](memory/cli.md)
- **Read the design:** [The memory model](memory/model.md), [Embeddings](memory/embeddings.md)
- **Ship a version:** [Releasing](releasing.md)
