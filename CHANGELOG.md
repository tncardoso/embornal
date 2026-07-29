# Changelog

All notable changes to Embornal are in this file.

The format is based on [Keep a Changelog][kac], and this project uses
[Semantic Versioning][semver].

## [Unreleased]

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
