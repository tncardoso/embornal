# Embornal

Embornal is a toolkit for agents.

## Documentation

Write all documentation in `docs/` with ASD-STE100 (Simplified Technical
English).

## Instructions

At each code change:

- Run lint, check and test
    - `cargo clippy -- -D warnings`
    - `cargo check`
    - `cargo test`
- Update `docs/` with updated behavior
- Do not skip those steps, even in simple changes
- Clippy warnings should be treated as errors: fix them, do not ignore or silence

