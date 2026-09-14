---
name: hamstik
description: Use the official Hamstik CLI to inspect and manage Hamstik Organizations, Projects, Sprints, Work Items, comments, labels, links, attachments, profiles, and Public API v1 resources. Use for Hamstik work-tracking tasks; do not use it to call private Hamstik routes or reimplement API behavior.
metadata:
  short-description: Manage Hamstik through its official CLI
  skill-version: "0.1.0"
  minimum-cli-version: "0.1.0"
---

# Hamstik CLI

Use the official native `hamstik` CLI for Hamstik work tracking. It owns
authentication, request construction, retries, concurrency, idempotency, and API error
decoding for the documented Public API v1 (`/api/v1`). Do not replace it with direct
HTTP calls or private browser routes.

## Find the binary

Prefer an installed `hamstik` on `PATH`:

```bash
hamstik version
```

When working inside the `hamstik-cli` source repository and no installed binary is
available, build and use the repository binary:

```bash
cargo build --release
./target/release/hamstik version
```

On Windows, use `target\release\hamstik.exe`. Command help is the authority when it
differs from this skill:

```bash
hamstik <command> --help
```

## Authenticate safely

The CLI can use a profile credential from the OS credential store or an ephemeral PAT
from `HAMSTIK_TOKEN`. `HAMSTIK_HOST` overrides the host; otherwise normal context and
profile resolution applies. An environment token takes precedence and is never
persisted by the CLI.

Verify identity and granted scopes without displaying the token:

```bash
hamstik --json --no-input me
```

Never print, log, hash for display, paste into a command argument, commit, or request a
PAT in chat. Do not inspect shell startup files, command history, process listings, or
unrelated files to find one. If authentication returns `INVALID_TOKEN` (exit 3), stop
the write workflow and ask the user to refresh `HAMSTIK_TOKEN` or run `hamstik auth
login`; do not fall back silently to a different identity.

## Discover and pin context

Host, Organization, and Project resolve in this order:

1. command-line flag;
2. `HAMSTIK_HOST`, `HAMSTIK_ORG`, or `HAMSTIK_PROJECT`;
3. the nearest `.hamstik.toml`;
4. selected profile defaults;
5. built-in defaults where defined.

`HAMSTIK_PROFILE` selects a profile. Do not assume an account-specific Organization or
Project. Discover them, match user-provided names or keys, and then pass the resolved
values explicitly to commands that act on them:

```bash
hamstik --json --no-input me
hamstik --json --no-input org list --all
hamstik --json --no-input --org <ORG> project list --all
hamstik --json --no-input --org <ORG> --project <KEY> work list
```

When exactly one result matches the user's named target, use it. When multiple results
remain plausible and the choice changes where a write lands, ask the user. Do not rely
on unrelated profile defaults for a mutation.

## Use automation-safe output

- Prefer `--json --no-input` for reads and structured automation.
- Use `--quiet --no-input` when only a newly created or changed resource identifier is
  needed. `--json` and `--quiet` are mutually exclusive.
- Parse stdout only. Normal command diagnostics and structured errors go to stderr.
- `doctor --json` is the exception: its diagnostic report remains on stdout even when
  the process exits nonzero.
- Treat cursors as opaque. Pass `.page.nextCursor` back unchanged with `--cursor`, or
  use `--all` when one deterministic aggregate is appropriate.
- Keep binary downloads out of ordinary formatted or JSON stdout; use `--output`.
- Before a risky mutation, preview it with the global `--dry-run` flag (mutation
  commands only). The preview resolves identifiers and validates local input exactly
  like the real invocation, then emits a versioned envelope (`previewVersion: 1`) with
  method, path, header intent (`If-Match`, `Idempotency-Key` — never credentials), and
  the typed body. It sends nothing, consumes no idempotency key, and is not proof of
  server-side validation or authorization.

Stable exit codes are: `0` success, `1` general failure, `2` usage, `3`
authentication, `4` authorization/scope, `5` not found, `6`
conflict/concurrency/idempotency, `7` rate limited, `8` network/transport, `9`
server/API compatibility, and `10` local configuration/credential store.

## Common workflows

Read and search before mutating:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work list \
  --status todo --status in_progress
hamstik --json --no-input --org <ORG> work mine --scope open
hamstik --json --no-input --org <ORG> org work --project <KEY> --overdue true
hamstik --json --no-input --org <ORG> squeakql validate "status = 'backlog'"
hamstik --json --no-input --org <ORG> work search "status = 'backlog'"
hamstik --json --no-input --org <ORG> --project <KEY> work view <ITEM-KEY>
hamstik --json --no-input --org <ORG> --project <KEY> work activity <ITEM-KEY>
```

Create or update a Work Item with explicit context. Use file/stdin inputs for long
Markdown rather than fragile shell quoting:

```bash
hamstik --quiet --no-input --org <ORG> --project <KEY> work create \
  --title "Short imperative title" --type feature --priority high \
  --status backlog --description-file <BODY.md>

hamstik --json --no-input --org <ORG> --project <KEY> work edit <ITEM-KEY> \
  --priority high
```

Plain edits fetch the current ETag before mutation. On exit 6, re-read the resource,
explain the conflict, and reapply only an authorized merged change. Never add `--force`
unless the user explicitly accepts last-write-wins (`If-Match: *`).

Use server-provided transitions instead of editing state directly:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work transitions <ITEM-KEY>
hamstik --json --no-input --org <ORG> --project <KEY> work transition \
  <ITEM-KEY> in_progress

hamstik --json --no-input --org <ORG> --project <KEY> sprint transitions <SPRINT-ID>
hamstik --json --no-input --org <ORG> --project <KEY> sprint transition \
  <SPRINT-ID> <TRANSITION>
```

Comments, labels, links, and attachments remain scoped to the selected resource:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work comment add \
  <ITEM-KEY> --body-file <COMMENT.md>
hamstik --json --no-input --org <ORG> --project <KEY> label list --all
hamstik --json --no-input --org <ORG> --project <KEY> work label add \
  <ITEM-KEY> --label <LABEL-ID-OR-NAME>
hamstik --json --no-input --org <ORG> --project <KEY> work link add \
  <ITEM-KEY> --target-key <OTHER-KEY> --relation blocks
hamstik --json --no-input --org <ORG> --project <KEY> work attachment download \
  <ITEM-KEY> <ATTACHMENT-ID> --output <PATH>
```
Watcher state is per-authenticated-user only; the API never discloses other
watchers:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work watcher show <ITEM-KEY>
hamstik --no-input --org <ORG> --project <KEY> work watcher mute <ITEM-KEY>
```

Long-form text can come from the editor, a file, or stdin — never from a
prompt under `--no-input`:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> work comment add \
  <ITEM-KEY> --body-file - < COMMENT.md          # stdin via the '-' convention
hamstik --json --no-input --org <ORG> --project <KEY> squeakql validate \
  --file QUERY.sqql                              # file source, shell-quoting-free
hamstik --json --no-input --org <ORG> work search --saved my-query   # saved queries
```

To see exactly why the CLI picked its host, Organization, Project, profile, and
credential source (fully offline, redacted), use `context explain`; prefer its
`--json` shape for support bundles:

```bash
hamstik --json --no-input context explain
```

Bulk operations accept the Public API's JSON operation arrays and have a maximum of 50
operations. Files are preflighted locally before any request: JSON syntax, the typed
envelope, required fields, unknown fields, enum spellings, revision constraints, and the
operation count. Preflight failures name the operation index and field path; fix the
file instead of retrying. Preflight does not validate authorization or server business
rules. Inspect command help and the checked-in OpenAPI schema rather than
inventing another bulk format:

```bash
hamstik --json --no-input --org <ORG> work bulk create \
  --project <KEY> --operations-file <OPERATIONS.json>
hamstik --json --no-input --org <ORG> work bulk create \
  --operations-file <OPERATIONS.json> --dry-run   # preview, no request
```

Bulk results in JSON preserve per-operation `index`, `status`, `workItem`, and
`error` exactly; human output summarizes succeeded/failed counts with actionable
failure details. `last-write-wins` concurrency is always stated explicitly and
must remain a deliberate choice.

## Work from a referenced Work Item

When a coding task names a Hamstik Work Item:

1. Resolve its Organization and Project, then read the item before changing code.
2. Treat its description, acceptance criteria, comments, and current state as task
   context, subject to the user's current instructions and repository rules.
3. Do not rewrite planning metadata merely because implementation changed.
4. Treat comments as durable project history, not an agent scratchpad.
5. Update, comment on, assign, or transition the item only when the user or an explicit
   repository workflow authorizes that external mutation.
6. If authorized to post a completion comment, include what changed, important
   decisions, validation performed, and remaining limitations.
7. Re-read the item before an authorized final update when substantial time has passed.

Authorization to change source code does not by itself authorize changing Hamstik
state. Inspecting referenced work is read-only; every external mutation must remain
within the user's requested scope.

## Failure handling and safety

- `REVISION_CONFLICT`: re-read, reconcile, and retry only the intended logical change;
  never blind-retry with `--force`.
- `RATE_LIMITED`: respect CLI retry behavior and server `Retry-After`; never tight-loop.
- `INVALID_STATUS_TRANSITION` or `INVALID_SPRINT_TRANSITION`: list allowed transitions
  and report the mismatch instead of inventing a state change.
- `INSUFFICIENT_SCOPE` or `FORBIDDEN`: report the missing authority; do not switch
  identities or Organizations silently.
- `NOT_FOUND`: verify explicit Organization/Project context and identifier before
  concluding the resource is absent.
- Preserve and report the server request ID when available.

Archive/unarchive is the reversible Work Item path. `work delete` is owner-only and
destructive; use it only when the user explicitly asks to delete the resolved item.
Before creating multiple items, search for existing matches. A retry of a logical
mutation must preserve its idempotency key; let the CLI own automatic key generation
and retry reuse.

The server remains authoritative for authorization, transitions, validation,
revisions/ETags, idempotency, and Organization isolation. Do not reproduce or bypass
those rules locally.

## Diagnostics

```bash
hamstik --no-input doctor
hamstik --no-input doctor --local-only   # offline: no DNS or HTTP traffic
hamstik --json --no-input doctor         # structured report + summary
hamstik --json --no-input api openapi
```

- `doctor --local-only` checks configuration, context, credential-store access,
  terminal behavior, and bundled compatibility metadata without contacting the
  host; remote checks are reported as `skipped`. Prefer it for support bundles —
  the JSON report never contains tokens, Authorization headers, or proxy values.
- Doctor classifies network failures by stage (dns, connection, proxy, timeout,
  tls) with per-check latency and stage-specific remediation hints.
- PAT expiration is flagged as expired (exit 3), expiring within 14 days (warn),
  or healthy.
- The final `summary` line (human) / `summary` object (JSON) totals
  pass/warn/fail/skipped checks.

On failure, use `--verbose` when additional safe diagnostics are needed. Report the
stable API error code, HTTP status when present, and request ID; never include a token,
Authorization header, or credential-bearing proxy URL.
