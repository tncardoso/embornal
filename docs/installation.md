---
icon: lucide/download
---

# Installation

Embornal is one binary. It holds the memory, the wiki and the embedding model,
so an installation adds one file and nothing more.

## The installer

On Linux and macOS:

```bash
curl -LsSf https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.sh | sh
```

On Windows:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.ps1 | iex"
```

The installer reads the operating system and the processor, fetches the correct
archive, and writes the binary to `~/.local/bin`. It then adds that directory
to the `PATH` of the shell. Open a new shell, or read the profile again, before
the first command:

```console
$ embornal --version
embornal 0.1.0
```

## A fixed version

Put the tag in the address of the installer:

```bash
curl -LsSf https://github.com/tncardoso/embornal/releases/download/v0.1.0/embornal-installer.sh | sh
```

Use this on a machine that must keep one version, and in a script that must
give the same result at a later date.

## What the installer obeys

| Variable | Effect |
| -------- | ------ |
| `EMBORNAL_INSTALL_DIR` | Writes the binary to this directory. |
| `EMBORNAL_NO_MODIFY_PATH=1` | Leaves the profile of the shell as it is. |
| `EMBORNAL_DISABLE_UPDATE=1` | Leaves out `embornal-update`. |
| `EMBORNAL_PRINT_QUIET=1` | Writes nothing but an error. |

```bash
EMBORNAL_INSTALL_DIR=/usr/local/bin curl -LsSf https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.sh | sh
```

The flags `--no-modify-path`, `--quiet` and `--verbose` do the same as the
variables. To read them all:

```bash
curl -LsSf https://github.com/tncardoso/embornal/releases/latest/download/embornal-installer.sh | sh -s -- --help
```

## The platforms

| System | Processor | Archive |
| ------ | --------- | ------- |
| Linux, glibc | x86-64 | `embornal-x86_64-unknown-linux-gnu.tar.xz` |
| Linux, glibc | ARM64 | `embornal-aarch64-unknown-linux-gnu.tar.xz` |
| macOS | Apple silicon | `embornal-aarch64-apple-darwin.tar.xz` |
| macOS | Intel | `embornal-x86_64-apple-darwin.tar.xz` |
| Windows | x86-64 | `embornal-x86_64-pc-windows-msvc.zip` |

There is no binary for musl, and thus none for Alpine Linux or for a
`distroless` image. Build it with Cargo on such a system.

## With Cargo

```bash
cargo install embornal
```

This build compiles llama.cpp, so the machine needs cmake, a C++ compiler and
libclang. On Debian and Ubuntu:

```bash
sudo apt-get install cmake clang libclang-dev
```

A machine with no C++ toolchain builds the memory without the model:

```bash
cargo install embornal --no-default-features
```

Such a build searches by keyword alone. See [embeddings](memory/embeddings.md).

## By hand

Each release holds the archives, one `sha256` file for each archive, and one
`sha256.sum` file for all of them. To fetch and to prove one archive:

```bash
tag=v0.1.0
file=embornal-x86_64-unknown-linux-gnu.tar.xz
base=https://github.com/tncardoso/embornal/releases/download/$tag

curl -LO $base/$file
curl -LO $base/$file.sha256
sha256sum --check $file.sha256

tar -xf $file
sudo install embornal-x86_64-unknown-linux-gnu/embornal /usr/local/bin
```

## The weights of the model

The binary holds the model but not its weights, which are near 330 MB. The
first command that searches by sense fetches them to `$EMBORNAL_HOME/models`,
and each later command reads the file that is already there.

To fetch them at a moment of your choice:

```bash
embornal memory reindex
```

See [embeddings](memory/embeddings.md) for a memory that must ask the network nothing.

## Update

The installer adds `embornal-update` next to the binary:

```bash
embornal-update
```

It reads the newest release, compares it with the version on this machine, and
replaces the binary when the release is newer. To go to a version of your
choice, run the installer again with that tag.

## Remove

Delete the binary, the receipt of the installation, and the memory itself:

```bash
rm ~/.local/bin/embornal ~/.local/bin/embornal-update
rm -r ~/.config/embornal
rm -r ~/.embornal
```

The last line deletes the facts. Keep `~/.embornal` to install Embornal again
later with the memory that it had.

The installer also wrote one line in the profile of the shell, for example in
`~/.bashrc` or in `~/.zshenv`. Delete that line if no other program uses
`~/.local/bin`.
