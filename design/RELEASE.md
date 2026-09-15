# Release design — versioned builds for the supported platform matrix

Authoritative for CLI-27 (release build system). Companion to
[VERSIONING.md](VERSIONING.md) (version/tag/changelog policy) and to the
sibling items it intentionally does not implement: CLI-28 (signing,
checksums as the integrity mechanism, SBOM, provenance), CLI-29
(package-manager/installer distribution), CLI-30 (release lifecycle,
release-notes automation).

## Scope

CLI-27 owns: the supported release target matrix, tag-triggered
versioned binary/archive builds, deterministic artifact naming, `hamstik
version` release/build metadata, artifact-level smoke tests, and the
fail-closed version/tag/changelog release gate. Everything downstream of
"a verified set of release archives on a GitHub Release" belongs to
CLI-28/29/30.

## Supported target matrix

The official release matrix is defined once, in `dist-workspace.toml`
(`targets`), and documented here. CI builds and smoke-tests **every**
target natively on GitHub-hosted runners; nothing ships without direct
execution of the released binary.

| Target triple                 | Platform                  | Runner (GitHub-hosted) | Status       |
| ----------------------------- | ------------------------- | ---------------------- | ------------ |
| `x86_64-unknown-linux-gnu`    | Linux, glibc, x64         | `ubuntu-22.04`         | supported    |
| `aarch64-unknown-linux-gnu`   | Linux, glibc, arm64       | `ubuntu-22.04-arm`     | supported    |
| `x86_64-apple-darwin`         | macOS, Intel              | `macos-15-intel`       | supported    |
| `aarch64-apple-darwin`        | macOS, Apple Silicon      | `macos-14`             | supported    |
| `x86_64-pc-windows-msvc`      | Windows, x64 (MSVC)       | `windows-2022`         | supported    |

Runtime baselines implied by the runners:

- Linux glibc targets build on Ubuntu 22.04, so the binaries require
  glibc ≥ 2.35. This is the compatibility floor; users on older glibc
  need a newer distro or a future musl build.
- Windows builds statically link the CRT (`msvc-crt-static`, dist's
  default), so no Visual C++ redistributable is required.
- macOS binaries target the default Xcode deployment target of the
  runner's toolchain at build time.

### Deliberate exclusions

- **`aarch64-pc-windows-msvc` (Windows ARM64)** — excluded. cargo-dist
  0.32.0 maps no GitHub-hosted runner for this target (`dist plan
  --target=aarch64-pc-windows-msvc` yields an empty build matrix), and
  the alternative — cross-compiling from an x64 Windows runner —
  provides no way to execute or smoke-test the result in CI. Per the
  CLI-27 rule that unsupported-for-verification targets must not be
  claimed, it stays out. Revisit when a native ARM64 Windows runner
  path is available in dist. Users on Windows ARM64 can run the x64
  build under Windows' x64 emulation.
- **`x86_64-unknown-linux-musl` / `aarch64-unknown-linux-musl`** —
  excluded. The glibc builds cover mainstream distributions; static
  musl builds add toolchain and keyring/crypto-dependency maintenance
  for no current demand, and arm64 musl cannot be executed on hosted
  runners for smoke testing. SPEC §66 lists musl as optional; a future
  need (e.g. Alpine packages via CLI-29) should reintroduce it with an
  explicit verification story.
- **`universal2-apple-darwin`** — not produced; per-target archives are
  what package-manager work (CLI-29) consumes, and a universal archive
  would double-download for no benefit.

Do not casually add targets: every entry in `targets` is a promise that
CI builds it *and executes the artifact* on a hosted runner.

## Release tooling: cargo-dist

The release pipeline is [cargo-dist](https://opensource.axo.dev/cargo-dist/)
(`dist`), version-pinned to `0.32.0` by `dist-workspace.toml`
(`cargo-dist-version`). cargo-dist owns the per-platform build/archive
machinery (SPEC §67 recommends it); the repository owns only:

- `dist-workspace.toml` — the declarative release configuration;
- the release gate (`crates/release-gate`) and smoke test
  (`scripts/release_smoke.py`) wired into the generated workflow;
- `.github/workflows/release.yml` — dist-generated plus the marked,
  locally maintained modifications listed in its header (see below).

Notable configuration decisions:

- `installers = []` — CLI-27 ships archives only. Shell/PowerShell
  installers, Homebrew, Winget, and npm belong to CLI-29.
- `unix-archive = ".tar.gz"`, `windows-archive = ".zip"` — SPEC §68–§70
  formats (dist's default for unix is `.tar.xz`; overridden here).
- `hosting = "github"` — artifacts publish to GitHub Releases.
- `pr-run-mode = "plan"` — pull requests run `dist plan` only: the
  release configuration is validated on every PR, but nothing is built
  or published outside a version tag.
- `allow-dirty = ["ci"]` — see "Regenerating the release workflow".
- `[dist.github-action-commits]` — all third-party GitHub Actions used
  by the generated workflow are pinned to immutable commit SHAs
  (repository policy), including their exact versions.

The `dist` profile (`[profile.dist]` in the root `Cargo.toml`) inherits
the release profile exactly, so release archives are built with the same
settings (`lto = "thin"`, etc.) that `cargo build --release` and the CI
quality gates exercise.

### Regenerating the release workflow

`release.yml` is dist output plus five clearly marked modifications
(header note, narrowed tag trigger, least-privilege permissions, release
gate steps, artifact smoke-test step). Because of `allow-dirty = ["ci"]`,
dist will not regenerate or freshness-check the file. To update after a
cargo-dist upgrade:

1. Temporarily remove `allow-dirty = ["ci"]` from `dist-workspace.toml`
   (with it set, `dist generate` silently skips the CI task).
2. Run `dist generate`, verify the diff.
3. Re-apply the five marked modifications (each has a
   `Hamstik modification/addition (CLI-27)` comment in place).
4. Restore `allow-dirty = ["ci"]` and run `dist plan` to confirm dist
   accepts the tree.

## Version and tag contract (fail closed)

One version source: `[workspace.package]` `version` in the root
`Cargo.toml` (VERSIONING.md §1). The contract is enforced in layers, so
a mismatch can never publish:

1. **Trigger** — the workflow fires only on tags matching
   `v[0-9]+.[0-9]+.[0-9]+*` (VERSIONING.md §9: `vMAJOR.MINOR.PATCH`).
2. **Release gate** — the `plan` job runs
   `cargo run --locked -p release-gate -- --tag "$GITHUB_REF_NAME"`
   before creating the release announcement or building anything. The
   gate (dependency-free, unit-tested) requires the tag to be exactly
   `v<version>` (optional prerelease suffix allowed, build metadata
   rejected), the version to equal the workspace `Cargo.toml` version,
   and `CHANGELOG.md` to contain the `## [<version>]` release section
   (i.e. `Unreleased` was renamed per VERSIONING.md §9). Any mismatch
   fails the workflow with an actionable message.
3. **cargo-dist** — `dist host --tag=...`/`dist build --tag=...` fail
   closed on tag/version mismatch on their own (belt and braces).
4. **Artifact smoke test** — the built archive's binary must report the
   same version as the build manifest (see below), catching a
   stale/embedded-version binary even if everything above passed.

There is currently no published release, so the first real tag `v0.1.0`
requires the VERSIONING.md §9 preparation first: rename
`## [Unreleased]` to `## [0.1.0] - <date>`, bump the workspace version
if needed, then tag. Until then the gate correctly refuses a local
`release-gate --tag v0.1.0` run.

## Release trigger and flow

- **Tag push** (`vX.Y.Z`): plan job → release gate → `dist host
  --steps=create` (creates the release announcement) → per-platform
  build jobs (build, package, **smoke test**, upload) → global artifact
  jobs (checksums, source tarball) → host job (uploads artifacts,
  publishes the GitHub Release) → announce.
- **Pull requests**: the `plan` job runs `dist plan` only — release
  configuration is validated, nothing is built, nothing is published.
- Normal CI (`ci.yml`) is unchanged and never publishes; only this
  workflow reacts to version tags.
- No manual local platform builds are required from a release engineer.

## Artifact naming and contents

cargo-dist's conventional naming (`hamstik-cli` is the releasing Cargo
package; the version lives in the release path/URL and the archive
metadata):

- `hamstik-cli-<target>.tar.gz` — Linux/macOS archives
- `hamstik-cli-<target>.zip` — Windows archives
- one `.sha256` checksum per archive (cargo-dist default; CLI-28 owns
  checksums/signatures as the final integrity mechanism)
- `source.tar.gz` and `sha256.sum`

CLI-28 adds two SBOM assets per release — `hamstik-cli.cdx.json` and
`hamstik-api-client.cdx.json` (CycloneDX 1.5 JSON, generated from the
committed `Cargo.lock` by a version-pinned `cargo-cyclonedx`) — plus
Sigstore build-provenance attestations for every downloadable asset,
verified before publication (see [design/SIGNING.md](SIGNING.md)).

Each archive contains:

```text
hamstik-cli-<target>/
├── hamstik            (hamstik.exe on Windows)
├── LICENSE
├── README.md
└── CHANGELOG.md
```

The version is unambiguous: the GitHub Release is `v<version>`, the
binary reports it via `hamstik version`, and the smoke test asserts the
match. `hamstik-cli-<target>` names stay stable across releases, which
is what CLI-29 consumers expect.

## Artifact smoke tests

Every build job, immediately after `dist build`, runs
`scripts/release_smoke.py` against **the archives it just produced**
(stdlib-only Python, invoked via bash so Windows/macOS/Linux behave
identically). For each archive it:

1. extracts it to a scratch directory (tar.gz/zip, whatever the target
   produced);
2. locates the `hamstik` executable and makes it executable;
3. runs it in a hermetic environment (`HAMSTIK_*` scrubbed, fresh
   `HOME`) with:
   - `hamstik --help` — binary starts (exit 0, `Usage` on stdout);
   - `hamstik version` — banner reports the release version;
   - `hamstik --json --no-input version` — JSON `version` equals the
     manifest's `app_version` (catches version/tag mismatch, wrong
     executable in the archive);
   - `hamstik --json --no-input doctor --local-only` — offline-safe
     diagnostic passes with zero failed checks (verified to make no
     network calls and to exit 0 on headless runners);
4. fails the job on any mismatch, which blocks publishing.

Because every matrix entry runs on a native runner for its target,
every released binary is directly executed as part of the release; no
cross-compilation caveats exist in the supported matrix.

## `hamstik version` build metadata

`crates/hamstik-cli/build.rs` emits two compile-time constants:

- `HAMSTIK_BUILD_TARGET` — the Rust target triple being compiled for
  (from cargo's `TARGET` build-script environment);
- `HAMSTIK_BUILD_COMMIT` — the full source commit: the
  `HAMSTIK_BUILD_COMMIT` env override if set (release automation), else
  `git rev-parse HEAD`, else the literal `unknown` (e.g. a source
  tarball build). A failed lookup never fails the build.

`hamstik version` renders these:

- human: the identity banner, then
  `commit    <12-char prefix>` and `target    <triple>` lines;
- `--json`: `{"version": ..., "commit": ..., "target": ...}` — the
  `version` field is unchanged; `commit`/`target` are additive fields
  (VERSIONING.md §3: consumers tolerate new fields). `commit` carries
  the full sha or `unknown`.
- `hamstik --version` / `-V` stays the terse `hamstik <version>` line;
  the `User-Agent` stays `hamstik-cli/<version>` (SPEC API contract
  identifies the CLI by semantic version, not build identity).

No timestamps or hostnames are embedded: identical sources and target
produce identical identity strings (see Reproducibility).

## Reproducibility

Release artifacts are built from the tagged commit by CI with a pinned
dist version and the repository's `rust-toolchain.toml` (stable).
Choices made to keep builds reproducible:

- embedded build identity is only the source commit and target triple —
  deterministic functions of the source tree; no timestamps, no host
  names, no environment-dependent release names;
- `rerun-if-changed` watching of the git database means the embedded
  commit updates with the checkout, while cargo's fingerprinting avoids
  recompiles when the value is unchanged;
- the release profile matches the ordinary release profile exactly.

Known, documented limitations (not bit-for-bit reproducibility, which
CLI-1 treats as "where practical"):

- rustc may embed absolute toolchain/workspace paths in some
  debug/metadata sections depending on toolchain version; `--remap-path-prefix`
  hardening is deliberately deferred.
- The commit hash differs between a git-checkout build and the same
  sources built from a tarball (`unknown`) — by design, since the
  release pipeline always builds from the git tag.
- GitHub runner images update over time; identical archives across
  *different* release runs are not guaranteed. Within one release run,
  each artifact is built once.

## Security posture (CLI-27; authenticity is CLI-28)

- Workflow permissions are least-privilege: top-level
  `contents: read`; `contents: write` only on the two jobs that
  create/publish the release; `actions: write` only on jobs that
  upload/download workflow artifacts.
- All third-party actions are pinned to immutable commit SHAs (config
  in `[dist.github-action-commits]`, plus the pinned
  `dtolnay/rust-toolchain` in the gate step).
- `persist-credentials: false` on every checkout; no PATs; the only
  token is the ephemeral `GITHUB_TOKEN`.
- The dist installer script is fetched from the official cargo-dist
  GitHub release with the version pinned in the URL
  (`cargo-dist-version`); the plan job also re-uses the binary from a
  workflow artifact when available.
- No secrets are printed or dumped; the release gate and smoke test
  read repository files only.
- No signing keys, no signing services, no attestations: intentionally
  absent (CLI-28). cargo-dist's incidental per-archive `.sha256` files
  are kept as useful defaults, not as the final integrity mechanism.

CLI-28 adds the supply-chain layer on top of this pipeline: CycloneDX
SBOMs from the committed `Cargo.lock` and Sigstore build-provenance
attestations, both verified fail-closed before the release is published.
The integrity/authenticity design, threat model, verification procedure,
and the deferred platform-signing decision are documented in
[design/SIGNING.md](SIGNING.md).

## Local maintenance workflow

```bash
cargo build -p release-gate                 # build the gate
cargo run -p release-gate -- --tag v0.2.0   # expect fail-closed until release prep
cargo test -p release-gate                  # gate unit tests
dist plan                                   # validate release config/workflow
dist build --artifacts=local                # build local-target artifacts
python3 scripts/release_smoke.py --manifest dist-manifest.json
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml'))"
```

`dist build` writes `dist-manifest.json` to the working directory and
archives to `target/distrib/`.
