# Changelog

All notable changes to the Hamstik CLI are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/) as detailed in
[`design/VERSIONING.md`](design/VERSIONING.md). While the CLI is `0.x`,
MINOR releases may contain breaking changes — read the *Breaking* notes
before upgrading.

No release has been published yet: everything below is part of the
in-progress first release and will be dated and versioned when it ships.

## [Unreleased]

### Added

- Coherent PAT onboarding and troubleshooting journey: `auth status --json` now
  reports credential type/name, expiry, the credential source (environment vs
  credential store), and a structured scope inventory; near-expiry credentials
  warn within 14 days on every authenticated run, and client-detected expiry
  fails `auth status` with the stable authentication exit code. `me` surfaces
  the same expiry summary and scope inventory. Structured 401/403 failures
  preserve code/status/request ID with CLI-added remediation
  (`auth`, `me`, `doctor`).
- README examples verification harness (`scripts/readme_examples.py` plus the
  `readme-examples` pytest suite and `make readme-check` target): every
  executable README shell block is classified (parse-only, mock, prose) and
  verified against the actual binary; untestable prose blocks are explicitly
  exempted, so examples cannot drift from real flags, enums, or output
  conventions.

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
  concurrency-aware mutations, Work Item workflows, and structured failure handling.
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

[Unreleased]: https://github.com/blackboardstudios/hamstik-cli/commits/main
