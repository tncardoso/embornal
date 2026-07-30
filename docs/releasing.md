---
icon: lucide/tag
---

# Releasing

A release starts with a tag. Everything after the tag is automatic: the CI
builds one binary for each platform, makes the GitHub release, and sends the
crate to crates.io.

## Once, before the first release

Write one secret in the repository, at Settings, Secrets and variables,
Actions:

| Secret | Where it comes from |
| ------ | ------------------- |
| `CARGO_REGISTRY_TOKEN` | crates.io, at Account Settings, API Tokens, with the scope `publish-update` |

`GITHUB_TOKEN` needs no work, because GitHub gives it to each workflow.

## The steps

1. Move the facts of the release into the changelog. In `CHANGELOG.md`, the
   heading `## [Unreleased]` becomes the version and the day:

   ```markdown
   ## [0.2.0] - 2026-08-14
   ```

   Then write a new empty `## [Unreleased]` above it. The release notes on
   GitHub are this section, so what it does not say, the release does not say.

2. Write the same version in `Cargo.toml`, and let Cargo write it in
   `Cargo.lock`:

   ```bash
   cargo check
   ```

3. Run what each change runs:

   ```bash
   cargo clippy --all-targets -- -D warnings
   cargo check
   cargo test
   ```

4. Commit, tag and push. The tag is the version with a `v` in front of it:

   ```bash
   git commit -am "release: 0.2.0"
   git tag v0.2.0
   git push && git push --tags
   ```

The number itself follows [Semantic Versioning](https://semver.org). A change
that makes an old command answer in a new way is a major release, even when the
code of the change is small.

## What the tag starts

| Workflow | What it does |
| -------- | ------------ |
| `release.yml` | Plans the release, builds each platform on its own runner, and makes the GitHub release with the archives, the checksums and the two installers. |
| `publish-crates.yml` | Waits for that release, and then sends the crate to crates.io. |

`publish-crates.yml` runs after the release exists, so a failure at crates.io
leaves the binaries where they are. To send the crate again after such a
failure, start `Publish to crates.io` by hand from the Actions page and give it
the tag.

The build of each platform compiles llama.cpp, so a release takes near half an
hour.

## The configuration of the release

`dist-workspace.toml` holds the platforms, the installers and the tools that
each runner installs. `.github/workflows/release.yml` comes from that file, so
no hand edits go in it. After a change:

```bash
dist generate
git add dist-workspace.toml .github/workflows/release.yml
```

To read what a release would hold, without a build and without a tag:

```bash
dist plan
```

To make the two installers on this machine, which is how to read what they do:

```bash
dist build --artifacts=global
```

To build the archive of this machine, which takes as long as one runner takes:

```bash
dist build --artifacts=local
```

Each of the three writes to `target/distrib`.

## A newer dist

`cargo-dist-version` in `dist-workspace.toml` says which version of `dist` the
CI uses. To move to a newer one, install it and let it write the file again:

```bash
cargo install cargo-dist --locked
dist init
dist generate
```

Read the difference in `release.yml` before the commit. That file decides which
runner builds each platform, and a new version of `dist` can move a build to
another image of the operating system.

## A release that must not go out yet

A tag such as `v0.2.0-rc.1` makes a pre-release on GitHub. `dist` marks it as
one, so the address `releases/latest/download/...` still gives the version
before it, and the installer of a user gives the stable release.
