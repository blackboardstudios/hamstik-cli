# Self Update (stable / prerelease channels) — Gated Design Stub

**Status:** Gated — design only. Not implemented, no network behavior, no command
surface. This document is the deliverable until the gate below opens.
**Work Item:** CLI-79 (`hamstik self update`, `--channel stable|prerelease`)
**Epic:** Packaging and release milestone.
**Gating model:** the same "do not build until the prerequisite milestone exists"
model used for CLI-5 (OAuth), CLI-17 (MCP), CLI-77 (workflow automation rules),
and CLI-78 (webhook management). Here the prerequisite is the packaging/release
milestone, not a Public API surface.

---

## 1. Gate condition

CLI-79 MUST NOT ship executable behavior until **all** of the following hold:

1. The packaging/release milestone is explicitly declared and its installable
   release channels are stable enough to update from: stable and prerelease
   releases are published with the integrity material required by
   [design/RELEASE.md](RELEASE.md) and [design/SIGNING.md](SIGNING.md)
   (per-artifact sha256, aggregate checksums, and build-provenance
   attestations).
2. A **machine-readable release/channel manifest** exists — one document per
   channel that names the current version, its channel, the artifact URL per
   platform, the checksum, and the attestation reference — so the CLI never has
   to scrape release notes or guess which asset is current.
3. Platform-specific **verified replacement** is designed and proven on
   Windows, macOS, and Linux, including the case where the running executable
   file is the one being replaced, and including detection of
   package-manager-owned installs that must not be overwritten.
4. Explicit **opt-in/opt-out** configuration and the offline `self status`
   surface described below are designed and implemented together with the
   update path — never as a follow-up.

Until then:

- No `hamstik self` command is added to the CLI, not even a hidden alias or a
  thin passthrough.
- No startup, background, scheduled, or on-demand version check makes a network
  request. The CLI MUST NOT phone home on invocation merely to discover a newer
  version.
- No "update available" banner, notice, or warning is emitted by any existing
  command.
- No placeholder that pretends to update but only prints instructions is shipped
  as a typed command; the documented channel upgrade steps in
  [design/INSTALL.md](INSTALL.md) remain the only supported upgrade path.

`hamstik version` and the offline diagnostics (`doctor --local-only`,
`context explain`) remain network-free and unchanged.

## 2. Outcome

A user can update the CLI in place instead of manually pinning versions:

```text
hamstik self update [--channel stable|prerelease]
```

The command replaces the installed binary with the verified artifact from the
selected channel, preserving the user's install location and permissions, and
leaving the previous binary intact (or recoverable) if anything fails. The
operation is explicit, reversible, and makes no network call the user did not
request.

## 3. Scope

- **Explicit opt-in / opt-out** for any automatic update behavior; manual
  `hamstik self update` is always an explicit user action.
- **Channel selection**: `stable` (default) and `prerelease`.
- **Verified package replacement**: checksum + provenance verification before
  any file is touched, with atomic replacement and rollback on failure.
- **Deterministic, scriptable output** (`--json`) and non-interactive semantics,
  consistent with every other command in SPEC §24–§30.

## 4. Proposed command surface (not implemented)

Sketch only; final spelling is settled when the gate opens and is subject to the
normal command-grammar and generated-docs process. The older SPEC §73
placeholder `hamstik update` is superseded by the namespaced `hamstik self`
family so that a future root-level `update` cannot collide with it.

```text
hamstik self status [--json]                  # offline; shows installed version,
                                              # install channel, configured channel,
                                              # and whether self-update is enabled
hamstik self update [--channel stable|prerelease]
                    [--check] [--dry-run] [--yes] [--json]
```

- `self status` is **offline** and prints the installed version, the detected
  install channel (direct, Homebrew, winget, npm wrapper, or unknown), the
  configured update channel, and the opt-in/opt-out state. It never contacts the
  network.
- `self update` performs the explicit update. `--check` reports whether a newer
  version exists on the selected channel without replacing anything;
  `--dry-run` prints the exact plan (source URL, expected checksum, target path)
  without writing. Both are explicit user actions and may make a network
  request only for the public release manifest.
- `--yes` satisfies the existing destructive-operation consent conventions for
  non-interactive/agent use (SPEC §30); it never changes which artifacts are
  accepted.
- `--channel` overrides the configured channel for that one invocation only; it
  is never persisted implicitly.

Configuration keys live in the existing config surface (SPEC §22) and follow its
precedence rules, e.g. `self_update.channel` (`stable` default) and
`self_update.check` (`off` default). An environment override such as
`HAMSTIK_SELF_UPDATE` exists only to force-disable automatic behavior for
scripted and CI use. Every value and its environmental escape hatch must be
documented in the same change that implements the command.

## 5. Channel semantics

```text
hamstik self update --channel stable
hamstik self update --channel prerelease
```

- **`stable`** resolves to the newest published release **without** a
  prerelease suffix. It MUST NOT offer a prerelease, even if that prerelease is
  numerically newer.
- **`prerelease`** resolves to the newest published release **including**
  prereleases (for example `vX.Y.Z-rc.1`) and MUST be chosen explicitly — it is
  never inferred from the currently installed version, and a prerelease install
  MUST NOT silently move a user onto `stable` or back without an explicit
  command.
- Version comparison follows semantic versioning as defined in
  [design/VERSIONING.md](VERSIONING.md); the CLI MUST never downgrade
  automatically. A downgrade, if ever supported, requires an explicit request.
- The configured channel is independent of the install channel. Updating a
  package-manager-owned install (Homebrew, winget, the optional npm wrapper)
  through `self update` is **refused** with guidance to use that manager's
  `upgrade` command; only direct download/installer installs are eligible for
  in-place replacement.
- Running `self update` when already current is an idempotent no-op that exits
  successfully and reports the current version.

## 6. Verified package replacement

Replacement is where a self-updater can corrupt a working install, so the
following are hard requirements, not best-effort behavior:

1. **Source is fixed.** Artifacts are fetched only from the official release
   URLs referenced by the channel manifest for
   `blackboardstudios/hamstik-cli`. No mirrors, no redirects to third parties,
   no `http`, and no user-supplied URL in the normal path.
2. **Verify before writing.** The downloaded artifact's sha256 MUST match the
   manifest, and its build-provenance attestation MUST verify against this
   repository, exactly as documented in [design/SIGNING.md](SIGNING.md), before
   any byte of the installed binary is replaced. A missing or unverifiable
   attestation fails closed.
3. **Fail closed and leave the install untouched.** Any network, checksum,
   attestation, filesystem, or permission failure aborts with a non-zero exit
   code and the original executable still runnable. A partially written binary
   MUST never be left behind.
4. **Atomic replacement.** On Unix-like systems the verified binary is staged in
   the target directory and atomically renamed over the destination, preserving
   executable permissions. On Windows, where a running executable cannot be
   overwritten in place, the design must use the platform's supported
   replace-on-runtime mechanism (stage + rename, or a self-deleting helper) and
   never leave the old and new binary in an ambiguous state.
5. **Recoverable.** The previous binary is retained (for example as a
   timestamped sibling) until the new binary has been confirmed to start and
   report the expected version, so a bad-but-verified build is still
   recoverable without re-downloading.
6. **No elevation.** Updates happen in the user's own install location. The CLI
   MUST NOT request administrator/root privileges and MUST NOT write outside the
   directory the current binary lives in.
7. **Positive version confirmation.** After a successful replacement the new
   binary's embedded version (SPEC §version surface) must equal the manifest's
   version; a mismatch is reported as an error rather than silently accepted.

## 7. Privacy, opt-in/opt-out, and "no silent network calls"

- There is **no telemetry**, ever. No usage counters, no install IDs, no
  anonymized callbacks.
- The CLI makes **no network request unless the user explicitly asked for it**.
  `self update` is such a request; `self status` is offline. Automatic update
  checks are **off by default** and require an explicit opt-in that the user can
  revoke at any time.
- When a check is explicitly opted into, the only thing fetched is the public
  release/channel manifest — no identifying headers, no credentials, no local
  state beyond the version and channel already needed to interpret the result.
- `doctor` and every other existing command stay network-free with respect to
  updates; they never gain a hidden update ping.
- Both opt-in and opt-out are first-class: disabling automatic checks is
  immediate, honored on the next invocation, and never "re-enabled" by an
  upgrade.
- Output is deterministic and scriptable (`--json`), including a stable
  machine-readable envelope for success, "already current", verification
  failure, and refused package-manager ownership, consistent with SPEC §24–§25.

## 8. Non-goals

- No implementation, command surface, or network behavior before the gate
  opens.
- No silent self-update, ever. Updates happen only when the user runs
  `hamstik self update` (or an explicitly opted-in check reports an available
  version).
- No background daemon, scheduled task, login item, shell hook, or automatic
  check outside an explicit invocation.
- No telemetry, phone-home, or unique install/device identifier.
- No use of Hamstik private browser `/api/*` routes; update metadata comes from
  the public release channel, not from the Hamstik server.
- No client-side reimplementation of server business logic; self-update is a
  release-distribution concern and does not touch Work Item, Organization, or
  API semantics.
- No overwriting package-manager-owned installs; no elevation/sudo; no writes
  outside the install directory.
- No delta/patch updating, no torrents/mirrors, no plugin or dependency update
  manager, and no "update all" behavior — the binary updates itself and nothing
  else.
- No cross-channel migration that silently changes a user's release channel.

## 9. Activation checklist (when the gate opens)

1. Publish the machine-readable per-channel release manifest described in §1 as
   part of the release pipeline, referencing the existing checksums and
   attestations; add a release-gate/CI check that it is complete and internally
   consistent.
2. Add the `hamstik self status` offline command and the opt-in/opt-out
   configuration first, with mock/fixture tests and no network dependency.
3. Implement `self update --check` / `--dry-run` against the manifest with
   verification logic fully unit-tested against tampered fixtures (bad sha256,
   missing attestation, wrong target, prerelease-on-stable).
4. Implement the atomic, recoverable replacement behind per-platform tests,
   including the Windows running-executable case and the package-manager-refusal
   path.
5. Add deterministic `--json` envelopes and non-interactive `--yes` consent,
   following the same conventions as every other mutation.
6. Update [design/INSTALL.md](INSTALL.md), [design/RELEASE.md](RELEASE.md), the
   README, the Agent Skill, generated command docs, and the changelog in the
   same change; remove this gate guard test only when the shipped behavior is
   real and documented.