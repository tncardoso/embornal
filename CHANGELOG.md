# Changelog

All notable changes to Embornal are in this file.

The format is based on [Keep a Changelog][kac], and this project uses
[Semantic Versioning][semver].

## [Unreleased]

## [0.2.0] - 2026-08-19

### Added

- A memory can live on a server. `embornal serve` puts the memory of a machine
  behind HTTP, and a `server` section in `config.yaml` turns the command line
  into a client of it. The server runs the same code that a memory on one
  machine runs.
- A client needs no embedding model, because the server makes the vectors.
  Build it with `--no-default-features`.
- The commands `embornal token add`, `ls` and `revoke`, which make and stop
  the tokens that let a client reach a server. The memory keeps the SHA-256 of
  a token, never the token.
- Each fact now holds the subject that wrote it, in an `owner` column and in
  an `owner` tag. One rule then gives a subject its own facts and nothing more.
  A memory on one machine is unchanged: its subject still reads everything.

### Changed

- **Breaking.** `embornal memory serve` is now `embornal memory wiki`. The name
  `serve` is the server that many people share.
- **Breaking.** The files move out of `$HOME/.embornal` into the three XDG
  directories: `config.yaml` in `$XDG_CONFIG_HOME/embornal`, `memory.db` in
  `$XDG_DATA_HOME/embornal`, and the weights in `$XDG_CACHE_HOME/embornal`.
  The first command of this build moves `config.yaml` and `memory.db` for you
  and says what it moved. It never writes over a memory that is already in the
  new place. The weights stay where they are, because a download can be made
  again.
- A subject name must hold no space, no `=` and no `,`, because the name
  becomes the value of an access tag.
- The database schema is at version 2. An older file migrates when it opens.

### Fixed

- A policy such as `path:/*` that covered every fact left the values of the
  other policies behind, so a subject that held both a path rule and a tag rule
  could not read anything at all.
- A tag value could hold the character that separates the fields of an access
  check, and so name a tag that its fact did not carry.
- A tag that went out through `serde` could not come back, because it was
  written as a pair of fields and read as one text.
- `--order-by` with an unknown value said that the server had failed.

## [0.1.1] - 2026-07-30

- Removing arm and windows from relase.

## [0.1.0] - 2026-07-30

### Added

- A memory of facts, in a tree of paths. Each fact is immutable, keeps its
  history, and has a signal strength that decreases with time.
- The commands `embornal memory store`, `ls`, `tree`, `cat`, `recall` and
  `serve`, and the command `embornal skill`.
- Search by word, with SQLite FTS5, and search by sense, with the
  EmbeddingGemma 300M model that runs in this process.
- Tags and access control with Casbin.
- A web UI, which `embornal memory serve` starts on port 1337.
- Packaged binaries for Linux, macOS and Windows, a shell installer and a
  PowerShell installer.

[kac]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/spec/v2.0.0.html
[Unreleased]: https://github.com/tncardoso/embornal/commits/main
