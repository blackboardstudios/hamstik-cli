# Contributing to Hamstik CLI

Thank you for contributing to the Hamstik CLI. This document explains how to set up a
development environment and what is expected from contributions.

## Development prerequisites

- Git
- A Rust toolchain (see below)
- Nothing else: the CLI is pure Rust, with no Node.js, Python, or other runtime

## Installing Rust

Install Rust with [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

The repository pins the toolchain through `rust-toolchain.toml`. Any reasonably current
stable Rust works; `cargo` installs the pinned toolchain automatically.

## Cloning and building

```bash
git clone https://github.com/blackboardstudios/hamstik-cli.git
cd hamstik-cli
cargo build
cargo run -p hamstik-cli -- --help
```

## Quality commands

These are the same checks CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

The optional helper `scripts/do-prechecks.py` runs the same gates in fail-fast order
plus `cargo deny check` and `git diff --check`. It needs Python 3 and the `rich`
package (`python3 -m pip install rich`); it is a convenience wrapper, not a build
requirement.

## Tests

Run the full workspace test suite with:

```bash
cargo test --workspace
```

Add Rust unit tests using the built-in test framework. Do not introduce a new test
framework for a small change.

## Design documents

The authoritative product and technical design live in:

- [design/PRD.md](design/PRD.md)
- [design/SPEC.md](design/SPEC.md)

Read them before meaningful implementation work. Do not make architectural decisions
that contradict them. If you believe a design decision should change, open an issue to
discuss it before implementing.

## Public API boundary

The CLI uses only Hamstik's documented Public API under `/api/v1`. Contributions MUST
NOT depend on private Hamstik browser routes or undocumented server behavior. The
frozen contract snapshot lives in [openapi/](openapi/README.md).

## Pull requests

- Keep pull requests focused and small.
- Describe what changed and why.
- Ensure all quality commands pass before requesting review.
- Update user-facing documentation when behavior changes.
- Never include secrets of any kind.

## Developer Certificate of Origin

This project uses the [Developer Certificate of Origin](https://developercertificate.org/)
(DCO) instead of a Contributor License Agreement. Every commit must be signed off:

```bash
git commit -s
```

The `Signed-off-by:` line certifies that you wrote the contribution or have the right to
submit it under the project's license.

## No-secret policy

Never commit Personal Access Tokens, `Authorization` headers, passwords, API keys, or
local credential files (including real `.env` files). If you accidentally commit a
secret, treat it as compromised and rotate it immediately.

## Licensing

By contributing, you agree that your contributions are licensed under the repository's
Apache-2.0 license (see [LICENSE](LICENSE)).