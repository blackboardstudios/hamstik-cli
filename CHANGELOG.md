# Changelog

All notable changes to the Hamstik CLI are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/) as detailed in
[`design/VERSIONING.md`](design/VERSIONING.md). While the CLI is `0.x`,
MINOR releases may contain breaking changes — read the *Breaking* notes
before upgrading.

## [Unreleased]

### Breaking

- Destructive commands now require explicit per-process consent before any
  request is sent. `work delete`, `project archive`, and completing a Sprint
  (`sprint transition <ID> done`) fail with a usage error (exit 2) unless the
  invocation passes the new global `--confirm-destructive` flag or the `--yes`
  scripting override, including under `--no-input`. Interactive terminal
  sessions may confirm at a `y/N` prompt; `--dry-run` is unaffected because it
  sends no mutation. Update automation that deletes Work Items, archives
  Projects, or completes Sprints to pass one of the flags.

### Added

- `work tree <KEY>` renders a Work Item's parent/child hierarchy from existing
  Public API v1 reads with a bounded `--depth` (default 3, maximum 10) and
  `--max-nodes` (default 200, maximum 2000), plus optional `--links`
  annotations of the server-reported `blocks` / `blocked_by` / `relates`
  relationships. Deep or cyclic structures terminate with an explicit
  truncation marker in both the human tree and the stable versioned `--json`
  document (`treeVersion: 1`), whose per-node `status`/`type`/`priority` are
  the server's values verbatim. No client-side readiness or blocking is
  computed.

- Global `--confirm-destructive` and `--yes` flags express local consent for
  destructive commands (`work delete`, `project archive`, completing a Sprint)
  without changing server-side authorization. Consent failures name the flag
  and never contact the server.

- Configuration-drift advisory for resolved Organization context. When a
  command already fetches the authenticated identity from `GET /api/v1/me`
  (`hamstik me`, `hamstik auth login`/`status`, the ephemeral-token profile
  bootstrap used by `hamstik org use`/`project use`, or `hamstik work
  create`/`edit --assignee me`), the CLI compares the resolved Organization
  against the memberships in that response. If the resolved Organization is
  not a member, a non-blocking warning is written to stderr naming both the
  configured value and the actual memberships; the exit code is unchanged and
  the context is never switched automatically. The check makes no extra
  network request — commands that do not already hold membership data do not
  check — and is skipped when `organizations` is empty (offline or a token
  without membership scope). Because the Public API's `/me` exposes
  Organization memberships only, Project drift is reported only through its
  Organization.

- `work bulk from-csv <FILE> --op create|update --project <KEY>` converts a
  local CSV file into the exact `work bulk` operations JSON array, written to
  `--output <FILE>` or stdout for piping into
  `work bulk create|update --operations-file -`. The dialect is RFC 4180
  (header row, comma separation, `"` quoting, `""` escapes; empty cell means
  omitted). `--op create` maps `title`; `--op update` maps `workItemKey`,
  `revision`, `title`, `description`, `type`, `priority`, `assignee`, `sprint`,
  `parent`, `storyPoints`, and `dueDate` into `changes`. The project key comes
  from `--project`, the global `--project`, or normal project selection
  (`HAMSTIK_PROJECT`, the context file, `hamstik project use`). The converter
  re-runs the generated array through the existing bulk preflight so it can
  never accept an envelope the JSON path rejects; unknown columns, mis-spelled
  enums, and malformed rows fail locally with the CSV row number and column
  name before any network call. Status and labels are not part of either bulk
  envelope and are rejected as unknown columns. Existing JSON-file bulk
  workflows are unaffected.

- `work create` convenience sources: `--from <KEY>` seeds a new Work Item from
  an existing one and `--template <FILE>` reads a local Markdown file with YAML
  frontmatter (`-` for stdin). Both copy the documented
  title/type/priority/description/labels allow-list only — identity, ownership,
  workflow state, revision, sprint, parent, story points, and due date are
  never carried over, and unknown frontmatter keys are rejected locally. Any
  explicit `create` flag overrides the copied value, the two flags are mutually
  exclusive (usage error before any request), and `--dry-run` previews the
  fully resolved create payload. Labels are attached through the existing
  label endpoint after create because the Public API v1 create body does not
  accept them.

- Composed daily triage view: `hamstik work triage` merges three existing
  Public API v1 reads for the resolved Organization in one invocation — open
  Work Items assigned to the authenticated user (`assignedOpen`, the
  `work mine --scope open` read), overdue Work Items assigned to the user
  (`overdue`, the `work mine --overdue true` read), and recent Project activity
  (`activity`, the `project activity` read, included only when a Project is
  resolved). Public API v1 exposes no unread/mention/watched-items read, so no
  local "seen" state is kept and the activity section is the Project feed. The
  Work Item sections honor the shared `--limit`/`--cursor`/`--all` pagination
  flags; `--activity N` bounds the feed (0 skips it) and `--since` windows it.
  A failed section is reported without aborting the others: the command exits
  0, the human view labels the section unavailable and warns on stderr, and
  `--json` always emits every section plus a top-level `failedSections` array.
  The stable JSON schema is `triageVersion`, `sections`
  (`assignedOpen`/`overdue`/`activity`), and `failedSections`; each section is
  `{status: "ok", items, page}`, `{status: "error", error}` (the standard CLI
  error object), or `{status: "skipped", reason}`. `--quiet` prints Work Item
  keys only.

- Client-side aggregate summaries: `hamstik project stats <KEY>` and
  `hamstik sprint stats <SPRINT_ID>` page through the Work Item list endpoint
  (same filters as `work list`) and compute counts by status, type, priority,
  and assignee plus story-point totals purely from returned fields. The output
  is explicitly labeled a client-side computed summary, not a server-reported
  metric. Without `--all` exactly one page is aggregated and a truncation
  marker (human note, `summary.truncated: true` in JSON) is reported whenever
  the server has more pages; `--all` follows every page and `--limit` caps the
  total items aggregated (the wire page size stays within the endpoint
  maximum). The `--json` document always carries `summary.total`,
  `summary.byStatus`, `summary.byType`, `summary.byPriority`,
  `summary.byAssignee`, `summary.totalStoryPoints`, `summary.estimatedItems`,
  `summary.truncated`, `computed`, and `itemsFetched`.

### Added

- Git-style external subcommand plugins: an executable named `hamstik-<name>`
  on `PATH` is invoked for `hamstik <name> [args...]` when `<name>` is not a
  built-in command. The resolved context (`HAMSTIK_HOST`, `HAMSTIK_PROFILE`,
  `HAMSTIK_ORG`, `HAMSTIK_PROJECT`, `HAMSTIK_CONTEXT_PATH`), output mode
  (`HAMSTIK_FORMAT`), global flags, and plugin identity (`HAMSTIK_PLUGIN`) are
  passed as environment variables — never via argv, and never with the raw
  credential token. Plugins appear in `hamstik commands --json` and in the
  root help as external (with `"external": true`), and are never executed
  during introspection. Missing executables now exit 2 (usage).

- Dynamic shell completion for Bash and fish: live Organization slugs, Project
  keys, labels, and Bash Work Item key positions use the hidden
  `_hamstik_dyn_complete` helper. It performs read-only Public API GETs only,
  fetches a single page without retries, and silently degrades to no candidates
  offline or without credentials.
- Date and time flags accept human-friendly local expressions. `--since`,
  `--updated-after`, `--due-before`, `--due-after`, `--due-date`, and sprint
  `--start-date`/`--end-date` now take forms such as `7d`, `2w`, `1mo`, `+3h`,
  `today`, `yesterday`, `tomorrow`, `2026-09-01`, or `2026-09-01 14:30` in
  addition to RFC 3339. Anything without an explicit UTC offset is read in the
  host's local time zone (a bare date is local midnight, a clock time that
  happens twice takes the earliest instant, and one the clock skipped moves past
  the shift), `mo`/`y` offsets use calendar arithmetic clamped to the end of the
  month, and every resolved value is sent to the Public API as canonical
  RFC 3339, reported by `--verbose`, and shown in a `--dry-run` preview body.
  Invalid expressions fail locally with a usage error before any request;
  `--json` output still carries canonical timestamps only.
- The `editor` configuration setting is honored: `hamstik config set editor
  <command>` is the editor `--description-editor`/`--body-editor` authoring
  launches when neither `$VISUAL` nor `$EDITOR` is set, so a stored default
  behaves as a default (precedence: environment > config > platform default).

### Fixed

- `hamstik config set` validates values before writing them. Empty values (use
  `hamstik config unset <key>` instead), embedded newlines or control
  characters, values longer than 1024 characters, and `output` spellings
  outside the modes the CLI implements (`human`, `json`, `jsonl`, `tsv`,
  `quiet`) now fail with exit code 10 and leave the configuration file
  untouched, instead of persisting a value nothing can use.
- `hamstik config unset <key>` no longer claims success for a key that was
  never set, and no longer rewrites the configuration file (or appends a
  mutation audit record) for a no-op.
- The credential-like-value rejection points at `hamstik auth login` and
  `HAMSTIK_TOKEN`; it previously suggested a `hamstik credential` command that
  does not exist.
- `hamstik config get|set|unset --help` (and the generated reference pages)
  list every accepted key instead of an example subset.

## [0.2.0] - 2026-09-19

### Breaking

- `--limit` on list commands is now the total number of items the command emits
  instead of the size of the page requested from the server (the wire page size
  is `min(limit, endpoint maximum)`: 200 items by default, 100 on the
  organization, project, sprint, label, and link endpoints the contract caps
  lower). A cap larger than one page — which was previously sent as an oversized
  page and rejected by the API with a 400 — is now clamped to that maximum: with
  `--all` the cap is met by paging and `--all --limit N` stops at exactly `N`
  items, without `--all` a single page is returned. When a cap cuts through a
  server page the result reports `page.nextCursor: null` with
  `page.hasMore: true`, because the Public API has no cursor for a position
  inside a page. (CLI-58)

### Added

- Line-oriented collection output for shell pipelines: `--jsonl` emits one
  compact, server-shaped resource per line; `--tsv` emits escaped table rows;
  `--columns NAME...` selects and orders human/TSV columns; and `--no-header`
  suppresses table headers. `--jq EXPR` embeds jq-compatible filtering for
  `--json`, `--jsonl`, and `--tsv` without requiring an external jq binary.
  Structured modes are mutually exclusive with each other and with `--quiet`,
  and invalid filters or column names fail explicitly (exit 2) before any
  network call. A single-resource command emits its document as one compact
  JSON line in `--jsonl`/`--tsv` mode, and `--jq` filters that document too.
  `--columns` and `--no-header` are table features: they apply to human and
  `--tsv` output and are a usage error with `--json`, `--jsonl`, or `--quiet`.
  Every list table column now carries a header name so no TSV column is unnamed
  and none is unreachable by `--columns`; the `auth list` profile marker column
  is now `ACTIVE` (the `--json` field `active` is unchanged).

- Typed server report commands (CLI-63): `hamstik project report <type>` wraps
  `GET /projects/{key}/reports/{type}` (for example `velocity`,
  `cumulative-flow`, `ageing-wip`, `epic-progress`) and `hamstik sprint report
  <id>` wraps `GET /sprints/{id}/report`, so reports no longer require raw
  `hamstik api request` calls. Human output renders the report rows as a table
  and the Sprint burndown as a scaled bar per sample (with the ideal line
  marked), plus commitment, completion, scope changes, carryover, status
  distribution, the change feed, and any server-reported limitations. `--json`
  echoes the server payload verbatim in the standard envelope, identical to the
  raw passthrough of the same endpoint apart from the `api request` `data`
  envelope. Report type names, filters, and window options are forwarded
  unchanged — the server owns report semantics, so an unknown type fails with
  the server's own message and request id and no client-side allowlist — and the
  CLI computes no metric of its own (client-side aggregation remains CLI-34).
  Pagination is `--limit`/`--cursor` (`--since-cursor`) over the report's `items`
  collection; `--all` is not offered because a report's series and rollups are
  one document, not a paged collection.
  The checked-in OpenAPI snapshot is refreshed to the live 67-operation,
  48-path contract, and every operation is registered in
  `openapi/api-parity.json` with its client method, CLI command, and tests.
- First-class Advanced Reporting commands for all ten newly documented Public
  API operations: `hamstik report list|view|create|edit|delete|run|selection-items`
  and `hamstik dashboard list|view|run`. List commands support visibility and
  standard cursor traversal. Report create/edit read the complete typed outer
  `AdvancedReportInput` JSON document from a file or stdin; edit/delete fetch
  and send the current ETag (with explicit `--force` support), generate and
  reuse idempotency keys, support mutation previews, and write the local audit
  record. Report/Dashboard runs fetch and submit the current revision
  automatically, with optional Dashboard filter JSON, and selection-item reads
  preserve the server's captured-result metadata. The CLI surfaces Premium,
  App enablement, Advanced capability, membership, and PAT-scope decisions from
  the server and never recomputes report datasets, dashboard widgets, or
  selection facts. The OpenAPI snapshot, typed client, parity manifest, schema
  guards, generated reference, README, and Agent Skill are synchronized.
- Shell pipeline ergonomics for every list command (CLI-58): `--since-cursor
  <CURSOR>` is an accepted alias of `--cursor` and is now honored as the start of
  a traversal even when combined with `--all`, so a pipeline that crashed
  mid-consumption resumes from the checkpoint it recorded without duplicates or
  gaps (within the server's own ordering consistency); `--sort` accepts an
  explicit direction (`--sort dueDate:desc`, `--sort priority:asc`) for the
  documented keys `updated`, `dueDate`, `priority`, and `rank`, and the CLI
  breaks ties on the Work Item key in both directions so a repeated query is
  byte-identical. The REST `sort` parameter carries no direction, so the CLI
  orders the result it fetched — pair a direction with `--all` to order a whole
  collection — and `rank`/`rank:asc` keep the server order while `rank:desc`
  reverses it. Unknown keys and directions are usage errors. `work comment list`
  and `work attachment list` honor the same three flags instead of treating
  `--limit` as a page size.

- `hamstik work view` now accepts multiple keys and a `--file` flag (`-` for
  stdin) for batch lookups, up to 500 keys per invocation. A single key keeps
  the existing single-item output; two or more keys fetch in one invocation
  with bounded concurrency and emit exactly one JSON envelope in input order:
  `{"items": [{"key", "status": "ok", "item", plus requested
  "comments"/"activity"/"links" sections} | {"key", "status": "error",
  "error"}], "failures": N, "total": N}`. A missing or forbidden item never
  aborts the batch; the exit code is the most severe per-item exit code
  (0 when every item succeeds). Sections are selected with `--comments N`,
  `--activity N`, and `--links N` (batch reads only; use `work context` for
  one item with sections) and long text bodies are trimmed with `--compact`.
  (CLI-54)

- `hamstik init` bootstraps the working directory by creating a `.hamstik.toml`
  file populated from the current resolved context (organization, project) and
  prints tailored first-run guidance. It fails if the file already exists and
  supports `--json` for machine-readable output. (CLI-68)

- Append-only local audit log for state-changing CLI operations. Each
  successful mutation appends exactly one JSON line — `{ when, command,
  target, revisionBefore, revisionAfter, requestId }` (revisions are `null`
  where the Public API has none) — to
  `$XDG_STATE_HOME/hamstik/audit.log`, default
  `~/.local/state/hamstik/audit.log`, with the platform-appropriate equivalent
  on macOS and Windows. `HAMSTIK_AUDIT_LOG` pins the location. The log is
  best-effort: an I/O failure warns on stderr but never fails the mutation, and
  the server-side audit record stays authoritative. Opt out with
  `hamstik config set audit_log false` (`settings.audit_log = false`), which
  `hamstik doctor` reports; `hamstik doctor` also reports the effective path.
  Only identifiers are recorded — never credentials, headers, request or
  response bodies, titles, or descriptions. The CLI never rotates the file; it
  grows by one short line per mutation and may be truncated or deleted at any
  time. (CLI-70)

## [0.1.3] - 2026-09-17

### Added

- `hamstik org use <slug>` and `hamstik project use <key>` now auto-create a
  config profile on-the-fly when `HAMSTIK_TOKEN` is set but no stored
  credential or config profile exists. The CLI calls `GET /api/v1/me` to
  populate the new profile (host + identity), then validates the requested
  organization or project through the API before persisting it as the
  default. This matches the existing env-token auth flow used by other
  commands and eliminates the previous "no active profile" error for
  headless or ephemeral setups (`org`, `project`).
- `hamstik work await <KEY>` polls until a Work Item reaches a server-reported
  condition (repeatable `--status`, `--timeout` up to 1h with exponential
  backoff, human-readable and `--json` output). (CLI-25)

- Global configuration commands (CLI-6): `hamstik config path|list|get|set|unset`
  manages safe, non-secret defaults in `~/.config/hamstik/config.toml`
  (`editor`, `pager`, `output`, `git_branch_template`, `profile`,
  `organization`, `project`). `set` rejects credential-like values (use
  `hamstik credential` or the OS keyring), and `get`/`set`/`unset` expose
  stable `{ key, value }` or `{ path }` JSON envelopes. Unknown keys and
  unsupported schema versions fail deterministically with exit code 10.

- Opt-in live Public API v1 acceptance suite (CLI-6):
  `cargo test --test live_acceptance -- --test-threads=1 --ignored` drives the
  real `hamstik` binary against a dedicated test Organization/Project over
  documented `/api/v1` routes. Gated behind `HAMSTIK_TOKEN`, `HAMSTIK_HOST`,
  `HAMSTIK_ACCEPTANCE_ORG`, and `HAMSTIK_ACCEPTANCE_PROJECT` (plus
  `HAMSTIK_ACCEPTANCE_MUTATIONS=true` for writes) and `#[ignore]`d, so offline
  `cargo test` only compiles it. A read-only smoke scenario proves auth, TLS,
  context, and the read routes; a mutation scenario exercises the full
  lifecycle — idempotent create/replay, edit, a real ETag conflict, status
  transitions, labels, comments, links, watchers, byte-for-byte attachment
  round-trip, sprints, and bulk create/update/transition — cleaning up every
  Work Item it created (delete, archive fallback) without masking the primary
  failure. All created resources carry a run-unique `acc-<unix-ts>-<pid>`
  marker; labels and sprints (undocumented delete routes) are the only
  residue. Runbook: `crates/hamstik-cli/tests/live_acceptance/README.md`.
  Normal CI compiles but does not execute the ignored live suite; it is run
  explicitly when a dedicated test deployment and credentials are available.
- Doctor support bundle (CLI-6): `hamstik doctor --bundle <path>` produces a
  versioned ZIP bundle (`bundleVersion: 1`) containing a redacted
  `doctor-report.json`, `context-explain.json`, `cli-info.json`,
  `api-compatibility.json`, `config-metadata.json`, and
  `bundle-manifest.json`. The bundle never contains PAT values, Authorization
  headers, credential-store contents, token-bearing environment variables, or
  credential-bearing proxy URLs. It is compatible with `--local-only`; remote
  checks are safely skipped when the flag is active. Automated redaction unit
  tests verify all bundle sections are secret-free.
- Work Item context bundle (CLI-21): `hamstik work context <KEY>` returns a
  one-invocation, data-only read bundle for a Work Item — metadata,
  description, server-reported links, labels, recent comments, recent
  activity, and the authenticated user's watcher state — composed entirely
  from Public API v1 reads. `--json` emits a stable, deterministic envelope
  (`bundleVersion: 1`); `--format markdown` renders the same content
  readably; `--comments N`, `--activity N`, and `--compact` bound the
  output size, and every clipped or omitted section carries an explicit
  marker (nothing is silently truncated). The bundle is data, not
  instructions: no workflow meaning is computed client-side, and the
  command is strictly read-only.
- Public API v1 passthrough (CLI-20): `hamstik api request /api/v1/...`
  and the shorthand `hamstik api /api/v1/...` call any documented Public
  API v1 route through the official CLI, in the spirit of `gh api` —
  including server routes newer than the installed CLI (the OpenAPI
  snapshot is never an allowlist). GET is the default; `--method`
  POST/PATCH/PUT/DELETE, `--body-file -|<file>`, `--field key=value`,
  `--query key=value`, and allowlisted `--header` overrides
  (`Accept`, `Content-Type`, `If-Match` — `Authorization` and any
  credential-bearing header are rejected) reuse the typed commands'
  transport: profile auth, TLS/CA bundle, retries with `Retry-After`
  handling, automatic idempotency keys for POST/DELETE reused across
  internal retries, request-ID preservation, redacted diagnostics, and
  stable exit codes. `--dry-run` previews the exact request with the
  versioned preview envelope; `--json` emits
  `{ method, path, data, meta? }` with `meta` carrying `requestId`,
  `etag`, `idempotencyReplayed`, `location`, and the rate-limit snapshot.
- Canonical Agent Skill installation and validation (CLI-3): the new
  `hamstik agent skill install` writes the skill bundled into the binary
  ([`skills/hamstik/SKILL.md`](skills/hamstik/SKILL.md)) into an Agent
  Skills discovery location — the current project's portable
  `.agents/skills/hamstik/` directory by default, the user-level portable
  location with `--global` (override with `HAMSTIK_SKILL_HOME`). Re-running
  an install is idempotent when the installed file is identical; a locally
  modified installed skill is never silently overwritten — `--force` is
  required for replacement. `hamstik agent skill check` validates frontmatter
  CLI-version compatibility plus every referenced command path and option
  against this binary's live `hamstik commands --json` manifest, for the
  installed skill by default (project location, then global) or an explicit
  file path (CI use); failures exit nonzero.

### Fixed

- OpenAPI refresh tooling and documentation now use the canonical
  `https://hamstik.com` API origin instead of the redirecting `www` host.
- `hamstik doctor` now recognizes a selected profile with no stored credential
  as the expected post-logout state. It reports credential-dependent checks as
  skipped instead of failing with `AUTH_REQUIRED`; inaccessible stores and
  invalid or rejected credentials still fail.
- Network diagnostics are now classified from the concrete transport error
  source chain (DNS, connection, timeout, TLS) instead of pattern-matching
  the rendered error text, so a library rewording can no longer silently
  degrade `doctor` and error output to the generic `network` stage.
- `hamstik org use` and `hamstik project use` now make the auto-created profile
  the active profile when they create it under an ephemeral `HAMSTIK_TOKEN`.
  Previously the default organization/project was written into a profile that
  profile selection never chose, so `doctor` and every command reported
  "organization not selected" even though the config contained the default.
- Auto-created profiles now seed the user's default organization from
  `GET /api/v1/me` (matching `auth login`), and configuration write failures
  during `org use`/`project use` report the configuration error class (exit 10)
  instead of a generic internal error.


## [0.1.2] - 2026-09-15

### Added

- Release lifecycle documentation (CLI-30): the maintainer release
  procedure is documented end-to-end in `design/RELEASE.md`
  (version prep → tag → automated build/publish → post-release checks),
  broken/yanked-release handling and per-channel rollback in
  `design/INSTALL.md`, and credential-safe troubleshooting guidance
  aligned with `doctor --local-only` / `context explain`. Release notes
  remain derived verbatim from `CHANGELOG.md` (the release pipeline
  generates the announcement from the renamed section); every release
  section now carries the artifact-verification pointer so it renders in
  the release notes. Updates stay channel-based per SPEC §73: no silent
  self-update, no telemetry, no background version checks.
- Verify downloads per [design/SIGNING.md](SIGNING.md): run
  `sha256sum -c sha256.sum` against the release's `sha256.sum` and
  `gh attestation verify <artifact> --repo blackboardstudios/hamstik-cli`
  (Sigstore build provenance). See also upgrade/uninstall/rollback notes
  in [design/INSTALL.md](INSTALL.md).

## [0.1.1] - 2026-09-15

### Added

- Installable release channels (CLI-29): a curl-able shell installer
  (`hamstik-cli-installer.sh`) for Linux/macOS, a PowerShell installer for
  Windows, and a Homebrew formula published per stable release to the
  `blackboardstudios/homebrew-hamstik` tap (`brew tap
  blackboardstudios/hamstik https://github.com/blackboardstudios/homebrew-hamstik`
  then `brew install blackboardstudios/hamstik/hamstik`). Winget manifests
  are generated per stable release into `winget/` and submitted to
  `microsoft/winget-pkgs` by a maintainer (deliberate manual step). Linux
  ships tarball + installer; `.deb`/`.rpm` and hosted APT/RPM repositories
  are deliberately deferred (design/INSTALL.md decision record). The
  Homebrew formula's sha256 lines are injected from the release's
  aggregate checksums at publish time, and the tap push requires a
  dedicated fine-grained PAT (`HOMEBREW_TAP_TOKEN`) scoped to the tap
  repository only.
- Verify downloads per [design/SIGNING.md](SIGNING.md): run
  `sha256sum -c sha256.sum` against the release's `sha256.sum` and
  `gh attestation verify <artifact> --repo blackboardstudios/hamstik-cli`
  (Sigstore build provenance). See also upgrade/uninstall/rollback notes
  in [design/INSTALL.md](INSTALL.md).

## [0.1.1-rc.1] - 2026-09-15

### Added

- Release supply-chain verification (CLI-28): every release now publishes
  CycloneDX 1.5 SBOMs (`hamstik-cli.cdx.json`,
  `hamstik-api-client.cdx.json`) generated from the committed `Cargo.lock`,
  and Sigstore build-provenance attestations for every downloadable asset.
  The release pipeline verifies all checksums and attestations before
  anything is published and fails closed on a mismatch. Verify downloads
  with `sha256sum -c sha256.sum` and `gh attestation verify <artifact>
  --repo blackboardstudios/hamstik-cli`; see `design/SIGNING.md` for the
  threat model and the deferred platform-signing decision (Windows
  Authenticode and macOS notarization await certificate provisioning).

### Fixed

- Updated `rustls` to 0.23.45 for RUSTSEC-2026-0285 (TLS 1.3 handshake
  messages incorrectly accepted across encryption level boundaries).

## [0.1.0] - 2026-09-15

### Added

- Versioned release builds (CLI-27): pushing a version tag (`vX.Y.Z`) now
  triggers a cargo-dist release pipeline that builds, smoke-tests, and
  publishes archives for the supported platform matrix — Linux glibc x64 and
  arm64 (`.tar.gz`), macOS Intel and Apple Silicon (`.tar.gz`), and Windows
  x64 (`.zip`), each with a sha256 checksum. The pipeline fails closed when
  the tag, the workspace `Cargo.toml` version, and the matching
  `CHANGELOG.md` release section disagree, and runs artifact-level smoke
  tests (the released binary itself: `--help`, `version`, and an offline
  `doctor --local-only`) before anything is published. Pull requests
  validate the release configuration in plan mode only. See
  `design/RELEASE.md` for the target matrix, exclusions, and maintenance
  workflow.
- `hamstik version` now reports build identity: human output adds
  `commit` (short source commit) and `target` (Rust target triple) lines
  under the banner, and `--json` output adds additive `commit` (full
  commit sha, or `"unknown"` when built outside a git checkout) and
  `target` fields. The existing `version` field and the `hamstik
  --version`/`-V` terse line are unchanged.
- `context explain` (CLI-12): an offline precedence report for every resolved
  setting — host, Organization, Project, profile, credential source, and
  color/input/retry behavior — showing the winning source, every shadowed
  source, and context-file discovery. JSON output uses stable source/status
  enums; HAMSTIK_TOKEN and stored credentials are represented only as
  present/absent, never displayed. Malformed context files degrade the report
  rather than aborting it (`context`).
- SqueakQL query files and saved queries (CLI-8): `squeakql validate` and
  `work search` accept `--file <PATH>` (`-` = stdin) alongside the existing
  inline expression, with multi-source conflicts rejected at parse time and
  one trailing newline stripped (all other content verbatim). A local
  saved-query store (`queries.toml` next to the config; plain expressions
  only, never credentials) with `squeakql list|show|save|delete`, name
  validation, and `--saved <NAME>` on validate/search. Validation JSON
  preserves the full Public API response including error spans; read-only
  SqueakQL POSTs carry no Idempotency-Key (`squeakql`, `work`).
- Editor authoring and consistent stdin conventions (CLI-7):
  `--description-editor`/`--body-editor` launch `$VISUAL` (then `$EDITOR`,
  then a platform default) on a secure owner-only temporary file removed on
  every exit path; unchanged or empty content cancels the command without
  sending. Source conflicts (inline + file + editor) fail at argument-parse
  time. `--no-input` rejects editor authoring immediately. Text content is
  preserved verbatim (Unicode, Markdown `#` headings, final-newline
  semantics) (`work`, `project`).
- Work Item watcher commands backed by the newly additive Public API
  operations (`getWorkItemWatcher`, `updateWorkItemWatcher`): `work watcher
  show|watch|unwatch|mute|unmute` report and change the authenticated user's
  own watcher state (never other watchers'). Actions are idempotent
  (`Idempotency-Key`, replay note) and carry no revision guard; the
  operations are classified as Complete in `openapi/api-parity.json` with
  client and CLI wire tests (`work`).
- Coherent PAT onboarding and troubleshooting journey: `auth status --json` now
  reports credential type/name, expiry, the credential source (environment vs
  credential store), and a structured scope inventory; near-expiry credentials
  warn within 14 days on every authenticated run, and client-detected expiry
  fails `auth status` with the stable authentication exit code. `me` surfaces
  the same expiry summary and scope inventory. Structured 401/403 failures
  preserve code/status/request ID with CLI-added remediation
  (`auth`, `me`, `doctor`).
- README examples verification harness (`scripts/readme_examples.py`, guarded by
  the Rust `readme_examples` contract test in `cargo test`): every executable
  README shell block is classified (parse-only, mock, prose) and verified
  against the actual binary; untestable prose blocks are explicitly exempted,
  so examples cannot drift from real flags, enums, or output conventions.
  Run the harness with `python3 scripts/readme_examples.py --verbose` after
  `cargo build --release`.

- Bulk operations preflight: operation files are validated locally (JSON syntax,
  typed envelope, per-operation required fields, unknown fields, enum spellings,
  revision constraints, 1–50 count) before any request, with diagnostics naming
  the failing operation index and field path. Human bulk output states the
  selected concurrency mode, summarizes succeeded/failed counts, and prints
  actionable per-failure details with server request ids; JSON preserves all
  documented per-operation fields. Stdin and file inputs behave identically
  (`work`).
- `doctor --local-only` verifies configuration, context resolution, profile
  selection, credential-store accessibility, terminal behavior, and bundled
  compatibility metadata with zero network traffic; remote checks are reported
  as skipped, making the output safe for support bundles. Doctor now classifies
  network failures by failing stage (DNS, TCP connection, proxy, timeout, TLS)
  with stage-specific remediation and per-check latency, reports PAT expiration
  (expired / within 14 days / healthy) and scope readiness, and prints a final
  pass/warn/fail/skipped summary in human and JSON modes (`doctor`).
- Deep parsed-OpenAPI schema parity guards: parameters, header parameters
  (`If-Match`, `Idempotency-Key`), request/response content types (multipart
  uploads, binary avatar downloads), response status codes, component schema
  drift (required/optional/nullable, enums, formats, property removals), and
  bulk envelope bounds are checked structurally against the checked-in
  snapshot, with additive changes classified separately from breaking drift
  and failures naming the operation or JSON pointer (`openapi`).
- Global `--dry-run` flag for mutation commands (`work create/edit/transition/start/close/
  archive/unarchive/delete`, `work label add|remove`, `work attachment upload|delete`,
  `work comment add|edit|delete`, `work link add|delete`, `work bulk create|update|transition`,
  `project create/edit/archive/unarchive`, `sprint create/transition`, `label create`).
  The CLI resolves identifiers and validates local input exactly as a real invocation
  would, then emits a versioned preview (`previewVersion: 1`) with method, path template,
  resolved path, sanitized header intent (`If-Match`, `Idempotency-Key` — never
  `Authorization`), and the typed request body, without sending any mutation request or
  consuming an idempotency key. Read commands reject the flag with a usage error
  (`work`).
- Canonical portable Hamstik Agent Skill at `skills/hamstik/SKILL.md`, covering
  credential-safe CLI usage, explicit context discovery, deterministic output,
  concurrency-aware mutations, Work Item workflows, user/Org member resolution,
  bulk `update`/`transition` concurrency modes, and structured failure handling.
- Full support for the current 53-operation Hamstik Public API v1 contract,
  including `me`, first-class `work mine` / `work my`, SqueakQL search via
  `work search`, independent `squeakql validate`, and unauthenticated
  `api openapi` contract retrieval.
- Parsed OpenAPI parity enforcement through `openapi/api-parity.json`; adding,
  removing, or moving an operation now fails a test until client, CLI, and test
  support are deliberately classified.
- `scripts/update-openapi.sh --check|--update` for explicit live-contract drift
  checks and validated byte-for-byte snapshot refreshes without making offline
  builds network-dependent.
- Typed API client crate (`hamstik-api-client`) with bearer injection,
  retry policy with `Retry-After` awareness, idempotency-key generation,
  ETag capture, cursor pagination helpers, and multipart upload support.
- Frozen Hamstik Public API v1 OpenAPI snapshot (`openapi/hamstik-v1.json`)
  as the development contract.
- `auth login --with-token`, `auth status`, `auth list`, `auth switch`,
  `auth logout`, and `auth forget` with PAT storage in the OS credential
  store (Windows Credential Manager, macOS Keychain, Linux Secret Service);
  headless automation via the `HAMSTIK_TOKEN` environment variable (`auth`).
- Working-context management: `context init`, `context show`, `context set`,
  `context clear`, backed by a project-local `.hamstik.toml` plus global
  profile defaults (`context`).
- Organization and project commands: `org list|view|use`,
  `project list|view|create|use` (`org`, `project`).
- Work item commands: `work list|view|create|edit`, status transitions
  (`work transitions`, `work transition`, `work start`, `work close`),
  comments (`work comment list|add|delete`), labels
  (`work label add|remove`), and attachments
  (`work attachment list|upload|download|delete`) (`work`).
- Sprint commands: `sprint list|view|create|transitions|transition` with
  completion actions (`--move-to-backlog` / `--move-to-sprint`) and Sprint
  ETag handling (`sprint`).
- Project label management: `label list|create` (`label`).
- Work item list filters, including `label`, `labelName`, `parent`,
  `topLevel`, `sprint`, and `updatedAfter`.
- Machine-facing output: `--json` / `--quiet` global modes, stable exit
  codes (SPEC §52), raw API body echo in `--json`, and an ANSI-free JSON
  failure envelope on stderr.
- Dependency-aware diagnostics: `hamstik doctor` reports configuration and
  context sources, credential source/store health, proxy presence,
  network/TLS reachability, live OpenAPI availability and compatibility,
  authentication, selected Organization/Project validity, and terminal
  rendering. JSON checks have stable IDs and explicit
  `pass|warn|fail|skipped` statuses while retaining the original `ok` and
  `critical` fields (`doctor`).
- Shell completion scripts via `hamstik completion <shell>` (`completion`).
- Terminal identity banner on root help and version surfaces (`version`).
- Compatibility contract test suite (`crates/hamstik-cli/tests/contract.rs`)
  guarding the documented stable surfaces: command hierarchy, subcommand
  sets, aliases, global options, enum spellings, the JSON failure envelope,
  stable exit codes (0–10), stdout/stderr separation, quiet/no-input
  behavior, pagination envelopes, sparse-field forwarding, mutation
  revision/replay metadata, binary-download safeguards, and context
  precedence. README documents which surfaces are stable and how breaking
  changes are communicated.
- Project and label colors render as bracketed swatches (`[██]`) in
  `project list`, `project view`, `label list`, and label detail views. The
  block glyphs are painted in the resource's actual color (truecolor with an
  xterm-256 fallback); the bracket frame and hex text stay in plain
  foreground so extreme colors such as black-on-black remain legible.
  Color-disabled and `--json` output are unchanged.
- `work comment list --exclude-deleted` hides soft-deleted comments instead of
  rendering `(deleted)` placeholders (`work comment`).
- The API client captures the documented `RateLimit-Limit` /
  `RateLimit-Remaining` / `RateLimit-Reset` headers on responses and errors.
  A successful request that leaves the window depleted waits out the reset
  (capped at the same 30 s bound as `Retry-After`) instead of sending the
  next request straight into a guaranteed `429`.

### Changed

- Refreshed the frozen OpenAPI snapshot from the live authoritative contract
  (51 → 53 operations) and aligned required/nullable response fields, bulk
  request operation types, profile avatar selectors, Work Item parent input,
  and Work Item label assignment by id or name.
- My Work and user-profile Work commands expose every filter defined by their
  respective current operations, including repeated array parameters, sparse
  fields, due-date filters, archived state, and opaque cursors.
- Binary download metadata now preserves content type, content length,
  content disposition, cache control, and request id where the operation
  defines them; JSON output contains metadata only, never binary bytes.
- API errors retain `fieldErrors`, `details`, and request ids. Human errors
  always print the correlation id when available; JSON preserves the complete
  structured error information.
- `Me` identity output includes `publicId` and per-Organization `username`;
  `auth status` reports both in human and JSON output.
- Authentication field names match the deployed API specification
  (`authentication` context in `GET /me`).
- Table rendering pads columns by visible width, so cells containing ANSI
  escape sequences (color swatches) no longer break column alignment.
- Retry/backoff is now implemented once in the API client and shared by JSON,
  void, binary, and multipart sends instead of four near-identical loops.

### Changed (API sync)

The frozen OpenAPI snapshot was refreshed against the updated Hamstik Public
API v1 (29 → 51 operations; all changes additive):

- Project lifecycle: `project edit`, `project archive`, `project unarchive`,
  and `project list --archived`; Project projections now carry `revision` and
  `archivedAt`, and edits use the `project-N` ETag (`project`).
- Work Item lifecycle: `work archive`, `work unarchive`, and
  `work delete [--cascade]` with Work Item ETag protection (`work`).
- Work Item links: `work link list|add|delete` with the `blocks`,
  `blocked_by`, and `relates` relations; new 409 codes `LINK_DUPLICATE` and
  `LINK_CONTRADICTION` map to the conflict exit code (`work link`).
- Activity feeds: `work activity`, `project activity`, and
  `user activity`, all paginated (`--since` supported) (`work`, `project`,
  `user`).
- Comment editing: `work comment edit` (author-only; responses carry
  `editedAt`) (`work comment`).
- Bulk operations: `work bulk create|update|transition` accepting 1–50
  operations per request from a JSON file or stdin, with per-item embedded
  results and `require-revision` / `last-write-wins` concurrency modes
  (`work bulk`).
- Member directory: `org members` over `GET /organizations/{slug}/users`
  (`org members`).
- Organization Work and My Work collections: `org work` lists Work Items
  across the organization with Project context; `--mine` sends
  `assignee=me` (`org work`).
- User profiles: `user view`, `user work`, `user activity`, and
  `user avatar` over the `profile:read` endpoints; profiles expose only
  public data (`publicId`, shared Organization usernames, visibility-scoped
  stats) (`user`).
- Work Item filters extended with `overdue`, `dueBefore`, `dueAfter`,
  `sort` (`updated|dueDate|priority|rank`), `archived`, and sparse
  `fields` fieldsets on `work list` and `org work`.
- Assignees now accept immutable public IDs (`usr_...`) on create/edit,
  mapped to `assigneePublicId`; legacy UUIDs keep using `assigneeId`.
- `user work` decodes the dedicated profile Work projection
  (`ProfileWorkItemList`) and gained a `REPORTER` table column; the
  reporter is rendered from the complete projection and shows `-` when
  absent or when a sparse fieldset omits it.
- Refreshed the frozen OpenAPI snapshot (`openapi/hamstik-v1.json`) for
  the profile Work projection change.
- Work Item list summaries are sparse-tolerant: only `id`, `key`, and
  `revision` are guaranteed when `fields=` is set; table rendering falls
  back per column.
- User summaries arrive in two shapes (`{id, name}` legacy and
  `{publicId, name}` public); the client decodes both, so `work archive`,
  `work unarchive`, label attach/detach, and comment edit responses render
  correctly.
- Work Item delete requests send an explicit `{"cascade": ...}` body and
  require the `work-item:delete` scope (Organization owners only).
- Error exit mapping: `LINK_DUPLICATE`, `LINK_CONTRADICTION`,
  `WORK_ITEM_ARCHIVED`, `WORK_ITEM_HAS_CHILDREN`, and `PROJECT_ARCHIVED`
  map to the conflict exit code (6).

### Breaking (API sync)

- `work list` gained shared filter flags via a flattened group; existing
  flag names and semantics are unchanged.
- `project list` output gained a `STATE` column; `project view` gained
  `revision` and `archived` detail lines (`0.x`, pre-release).

### Security

- Human-readable output now filters server-influenced text through a
  control-sequence sanitizer that keeps only SGR color sequences and strips
  every other escape/control character. Work Item titles, descriptions,
  comments, and other resource content can no longer inject OSC/CSI sequences
  (clipboard writes, cursor movement, title changes) into the user's terminal.
  `--json` output is byte-faithful and relies on JSON escaping.
- Upload (`work attachment upload`) and bulk operations (`--operations-file -`)
  now read stdin through the same streaming size caps as file inputs, so an
  oversized or misdirected stream is rejected at the first excess byte instead
  of being buffered whole.

### Fixed

- `doctor` now exits nonzero when a credential-store read fails, reports an
  invalid `HAMSTIK_TOKEN` only once, distinguishes global configuration from
  local context failures, preserves API error/request metadata, and avoids
  interactive terminal samples in redirected output.
- `work edit --parent` and `work create --parent` forward the current API's
  bounded parent identifier directly so the server remains authoritative for
  identifier resolution and validation (`work`).
- `work label add|remove --label` accepts a label name (case-insensitive;
  labels are stored lowercase) in addition to a UUID. Attach-by-name uses the
  current request body directly; detach-by-name resolves the id required by
  the DELETE path (`work label`).
- `--assignee me` on `work create|edit` resolves to the caller's `usr_` public
  ID via `GET /me` before sending; the server only accepts ids on writes
  (`work`).
- `user view|work|activity|avatar` accept `me` as the target, resolving it the
  same way (`user`).
- `--host` no longer hides a profile's stored credential: the lookup tries the
  selected host first, then the profile host recorded at login, and the
  failure message names both hosts when both were tried (`auth`, `doctor`).
- Human API errors always include `requestId`; `--verbose` additionally shows
  the HTTP status, matching the structured information retained by `--json`
  (`output`).
- Interactive prompts are now disabled whenever `--json` or `--no-input` is
  active (prompting additionally requires an interactive stdout, not just
  stdin). Missing required values fail deterministically with exit 2 instead
  of prompting (PRD §24/§30, SPEC §41; `work create`, `sprint create`,
  `project create`, `label create`, `auth login`).
- `auth login` no longer consumes `HAMSTIK_TOKEN` and writes it to the OS
  credential store. An environment token is ephemeral (SPEC §27, PRD §6.3);
  persistence now requires an explicit `auth login --with-token`, and a set
  `HAMSTIK_TOKEN` produces a clear usage error instead.
- `auth switch --json`, `context init --json`, and `context clear --json` now
  emit JSON on stdout; `auth switch` and `context init|clear` also honor
  `--quiet`. Previously they printed human text in `--json` mode, violating
  the stdout contract.
- `work edit --title` no longer conflicts with `--clear-description`; both may
  be combined (`work edit`).
- Retrying a `DELETE` after a transient failure no longer reports a false
  `NOT_FOUND` when the retry finds the resource already gone. A 404 on a
  retry is treated as success (the desired end state holds); a first-attempt
  404 still fails as before (`work comment delete`, `work attachment delete`,
  `work link delete`, `work delete`).
- `work link list --all` and `work activity --all` now honor `--limit` on
  every page, matching `work list --all` (`work`).
- The `--archived` filter on Work Item collections (`work list`, `org work`,
  `work mine`, `user work`) and `project list --archived` is now documented
  accurately: it selects only archived (`true`) or only unarchived
  (`false`/omitted) resources rather than "including" archived items
  alongside active ones. Serialization was already correct and unchanged;
  users who need both states in one result must issue separate queries
  (`work`, `project`).

[Unreleased]: https://github.com/blackboardstudios/hamstik-cli/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.2.0
[0.1.3]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.1.3
[0.1.2]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.1.2
[0.1.1]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.1.1
[0.1.1-rc.1]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.1.1-rc.1
[0.1.0]: https://github.com/blackboardstudios/hamstik-cli/releases/tag/v0.1.0
