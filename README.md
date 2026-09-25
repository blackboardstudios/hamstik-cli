<p align="center">
  <img
    src="assets/github/readme-header.png"
    alt="Hamstik CLI — build, automate, and integrate from your terminal"
    width="100%"
  />
</p>

Hamstik CLI is the official native command-line interface for
[Hamstik](https://hamstik.com), built for developers, automation, CI/CD, and
coding agents.

It talks to Hamstik exclusively through the documented **Hamstik Public API v1**
under `/api/v1`, making the same platform capabilities available to terminals,
scripts, and agent workflows.

> **Status:** Pre-alpha. The current Public API v1 command surface is implemented,
> but packaging and the first supported release are still under development.

[![CI](https://github.com/blackboardstudios/hamstik-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/blackboardstudios/hamstik-cli/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## Quick start

### Install a release (recommended)

Stable releases publish installers and archives for macOS (Homebrew +
installer script), Windows (PowerShell installer + archives), and Linux
(shell installer + archives) — see [design/INSTALL.md](design/INSTALL.md)
for every channel and its upgrade/uninstall steps:

```bash
# macOS — Homebrew
brew tap blackboardstudios/hamstik https://github.com/blackboardstudios/homebrew-hamstik
brew install blackboardstudios/hamstik/hamstik

# Linux — shell installer
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/blackboardstudios/hamstik-cli/releases/latest/download/hamstik-cli-installer.sh \
  | sh

# Windows — PowerShell installer
irm https://github.com/blackboardstudios/hamstik-cli/releases/latest/download/hamstik-cli-installer.ps1 | iex
```

Verify any downloaded release per [design/SIGNING.md](design/SIGNING.md)
(checksums + `gh attestation verify`). Then sign in and start working; the
Personal Access Token is stored in the OS credential store:

```bash
hamstik auth login --with-token      # reads the PAT from stdin
hamstik context init --org acme --project HAM
hamstik work list --mine
hamstik work start HAM-1
```

### Build from source

The CLI is not yet on crates.io. Build it from source and install the
resulting binary into Cargo's binary directory:

```bash
git clone https://github.com/blackboardstudios/hamstik-cli.git
cd hamstik-cli
cargo build --workspace
cargo install --path crates/hamstik-cli --locked
hamstik --help
hamstik --version
```

If Cargo's binary directory is not on `PATH`, invoke the workspace build as
`./target/debug/hamstik` (or `target\debug\hamstik.exe` on Windows).

## Terminal banner

Running `hamstik` with no command — or with `--help` / `--version` — prints the
identity banner:

```text
              _                         _   _ _
             | |__   __ _ _ __ ___  ___| |_(_) | __
    (\___/)  | '_ \ / _` | '_ ` _ \/ __| __| | |/ /
    (='.'=)  | | | | (_| | | | | | \__ \ |_| |   <
    (")_(")  |_| |_|\__,_|_| |_| |_|___/\__|_|_|\_

🐹 hamstik cli v0.1.0      © Blackboard Studios LLC
```

The banner appears on the human help surfaces (`hamstik` with no command,
`-h`/`--help`, and the `version` subcommand). The machine-parsed `-V`/`--version`
surface stays terse: it prints a single `hamstik <version>` line. The banner is
intentionally omitted from `--json` machine output, subcommand help, and
completion scripts.

Human `hamstik version` extends the banner with build identity — the source
commit and the Rust target triple the binary was built for — so a release
artifact can be identified at a glance:

```text
🐹 hamstik cli v0.1.0      © Blackboard Studios LLC

commit    edcd8f1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e
target    aarch64-apple-darwin
```

`hamstik version --json` carries the same identity additively (`version`,
`commit`, `target` fields; `commit` is the full sha or `"unknown"` when the
binary was built outside a git checkout). See [design/RELEASE.md](design/RELEASE.md).

## Why Hamstik CLI?

![Why Hamstik CLI?](assets/github/why-hamstik-cli.png)

- **API-first** — the CLI is built against the documented Hamstik Public API,
  not private server internals. What the API allows, the CLI does; what it
  doesn't, the CLI won't pretend to.
- **Automation-ready** — designed from the start for terminals, scripts,
  CI/CD pipelines, and coding agents, with a stable machine-facing contract in
  mind.
- **Native Rust binary** — no Node.js, Python, or other language runtime is
  required to build or run it.
- **Frozen API contract** — the repository carries a checked-in OpenAPI
  snapshot of Hamstik Public API v1 so CLI development has a reproducible
  reference as the platform evolves.

## How it works

![Terminal to Hamstik API workflow](assets/github/workflow-diagram.png)

```text
Terminal / automation / coding agent
              ↓
          Hamstik CLI
              ↓
      Hamstik Public API v1
              ↓
        Hamstik workspace
```

Hamstik CLI does not reach into Hamstik's database or application internals.
Everything goes through the documented Public API under `/api/v1`. The
checked-in OpenAPI snapshot gives CLI development a reproducible contract while
the live Hamstik service remains the authoritative implementation.

## Current capabilities

The Dogfooding Alpha command surface is implemented. Today the CLI provides:

- a native Rust workspace that builds the `hamstik` executable;
- a typed client for the Hamstik Public API v1 (`/api/v1`) with pagination,
  retries, and idempotency-key support;
- authentication and profiles — `hamstik auth login|status|list|switch|logout|forget`,
  with secrets held only in the OS credential store;
- first-run bootstrap — `hamstik init` creates a `.hamstik.toml` in the
  current directory with first-run guidance;
- working context — `hamstik context show|set|clear|init` backed by a
  project-local `.hamstik.toml` plus global profile defaults;
- global configuration — `hamstik config path|list|get|set|unset` for safe
  non-secret defaults (`profile`, `organization`, `project`, `editor`, `pager`,
  `output`, `git_branch_template`, `audit_log`); values are validated locally
  before they are written, and credentials are never accepted;
- organizations and projects — `hamstik org list|view|use` and
  `hamstik project list|view|create|edit|archive|unarchive|activity|report|stats|use`;
- Organization Attributes — `hamstik attribute list|view|create|rename|transition`,
  `attribute option add|rename|reorder|retire`, and `attribute project
  list|enable|disable`; definitions and options use stable keys, while the
  server remains authoritative for Organization governance and Project
  authorization;
- Advanced Reporting — `hamstik report list|view|create|edit|delete|run|selection-items`
  and `hamstik dashboard list|view|run`, including JSON definition/filter
  files, cursor traversal, automatic revision reads, and report ETag protection;
- member directory — `hamstik org members` and Organization-wide work via
  `hamstik org work` (`--mine` for the authenticated user);
- user profiles — `hamstik user view|work|activity|avatar` for
  visibility-scoped public data;
- authenticated identity via `hamstik me`, including credential scopes,
  expiration, default Organization, and memberships;
- sprints — `hamstik sprint list|view|create|transitions|transition|report|stats`
  with completion actions for sprints that still have unfinished work items;
- read-only boards — `hamstik board view [--sprint <ID>] [--project <KEY>]`
  composes the existing Work Item list read into status columns (`--json`
  emits a stable `boardVersion: 1` document);
- release versions — `hamstik release
  list|view|create|edit|transitions|transition|archive|restore|scope`, Work
  Item membership (`release item list|add|remove|replace`, `release
  bulk-membership`), announcements (`release announcement
  current|revision|publish`, `release announcement draft
  show|generate|edit|discard`), and frozen audit packages (`release audit
  generate|get|list`);
- Organization Milestones — `hamstik milestone
  list|view|create|edit|transitions|transition|releases|events` with release
  membership (`milestone release add|remove`);
- labels — `hamstik label list|create` and `hamstik work label add|remove`
  (attach/detach with Work Item revision protection);
- work items — `hamstik work list|view|create|edit`, status transitions
  (`transitions`, `transition`, `start`, `close`), comments
  (`work comment list|add|edit|delete`), links
  (`work link list|add|delete`), activity (`work activity`), archive/
  unarchive/delete lifecycle, watcher state (`work watcher
  show|watch|unwatch|mute|unmute`), bulk operations
  (`work bulk create|update|transition`), condition waiting
  (`work await`), and live following (`work watch <KEY>` with
  `--since`/`--interval`/`--notify`; `--json` streams one event per line).
  `work create` also accepts `--from <KEY>` (copy the
  documented title/type/priority/description/labels allow-list from an
  existing Work Item; identity, ownership, and workflow state are never
  copied) and `--template <FILE>` (a local Markdown file with YAML
  frontmatter) as mutually exclusive starting points, both fully resolved by
  `--dry-run`;
- first-class My Work via `hamstik work mine` (`work my` alias), a composed
  daily triage view via `hamstik work triage` (open Work assigned to you,
  overdue Work, and recent Project activity in one command — each section
  degrades independently, and `--json` marks a failed section with its error
  instead of silently omitting it), and Organization-wide SqueakQL search via
  `hamstik work search` / `hamstik squeakql validate`;
- a read-only multi-project overview via `hamstik work dashboard` that composes
  `work mine` with one Work Item list per configured Project (`--project`
  flags, or `dashboard_projects` in `.hamstik.toml`). Fetches run with bounded
  concurrency, every item is attributed to its Project, and one failing
  Project is reported in-band without hiding the others' results (`--json`
  carries a stable `dashboardVersion: 1` document with per-section
  `status`/`error` and `failedSections`);
- a one-invocation Work Item read bundle via `hamstik work context
  <KEY>` (`--json` for agents, `--format markdown` for readable
  hand-off, `--comments N` / `--activity N` / `--compact` size
  controls with explicit truncation markers) — data only, composed
  entirely from Public API v1 reads;
- a bounded parent/child hierarchy via `hamstik work tree <KEY>
  [--depth N] [--max-nodes N] [--links]` with explicit depth, node,
  and cycle truncation markers, plus optional server-reported
  `blocks`/`blocked_by`/`relates` annotations — no client-side
  readiness or blocking is computed;
- a read-only kanban board via `hamstik board view [--sprint <ID>]
  [--project <KEY>]` composing the existing Work Item list read into
  width-aware status columns (`--json` for a stable `boardVersion: 1`
  document). Columns come from the statuses the server reports, items with
  no status render in an explicit `(no status)` bucket, and there is no
  `board move` — state changes continue through `work transition`;
- attachments — `work attachment list|upload|download|view|delete`;
- the unauthenticated live contract via `hamstik api openapi`;
- a point-in-time Public API rate-limit snapshot via `hamstik api
  rate-limit` — one cheap authenticated read that reports the server's
  `RateLimit-*` headroom without the proactive depleted-window wait that
  typed reads apply;
- a `gh api`-style Public API v1 passthrough (`hamstik api /api/v1/...` /
  `hamstik api request /api/v1/... --method POST`) so any documented v1
  route is callable — including server routes newer than the installed CLI
  — with the same auth, TLS, retries, idempotency, and redaction as typed
  commands;
- automation-friendly output via `--json` / `--quiet` and stable exit codes;
- shell completions (`hamstik completion <shell>`) and dependency-aware
  diagnostics (`hamstik doctor`) covering local configuration, credential
  sources, network/TLS, Public API compatibility, authentication, selected
  Organization/Project validity, and terminal rendering;
- a local failed-request journal and `hamstik replay` to review recent server
  errors and network failures by request id, entirely offline;
- agent automation — `hamstik agent skill install` writes the canonical
  bundled Agent Skill ([`skills/hamstik/SKILL.md`](skills/hamstik/SKILL.md))
  into an Agent Skills discovery location (the current project's portable
  `.agents/skills/hamstik/` directory by default, `--global` for the
  user-level location); a locally modified installed skill is never silently
  overwritten (`--force` is required), and `hamstik agent skill check`
  validates any skill file — or the installed skill — against this binary's
  live command manifest and CLI-version metadata. `hamstik agent validate`
  (also `agent doctor`) verifies the local agent harness: installed skill
  metadata against the running CLI version, credential-shaped content in the
  config directory (reported by location and kind, never printed), and
  bundled OpenAPI snapshot freshness when online (skipped with `--offline` or
  when the live contract is unreachable);
- a machine-readable command manifest (`hamstik commands --json`), a
  copy-pasteable API cookbook (`hamstik commands --cookbook`) derived from
  the bundled Agent Skill, and generated command reference + man pages under
  [`docs/`](docs/reference) — regenerated deterministically with
  `scripts/generate_docs.py` and drift-checked in CI;
- cross-platform CI on Linux, Windows, and macOS.

OAuth and the MCP server remain future work. The canonical portable Agent
Skill ships with the CLI (install it with `hamstik agent skill install`; see
[`skills/hamstik/SKILL.md`](skills/hamstik/SKILL.md)). See the
[design documents](#design-documents) for where the CLI is headed.

## Command examples

Global `--org`, `--project`, and `--json` options may be placed before or after
subcommands. Cursors are opaque: pass the returned `nextCursor` unchanged.

### Shell completions

`hamstik completion <shell>` prints static completion for every supported shell.
Bash and fish also query live Organization slugs, Project keys, and labels
through the hidden `hamstik _hamstik_dyn_complete` helper; Bash additionally
uses it for Work Item key positions. These callbacks make read-only Public API
v1 `GET` requests, fetch one bounded page with no retries, and silently return
no candidates when offline or unauthenticated. Context precedence remains
flag > environment > `.hamstik.toml` > profile. Zsh, PowerShell, and Elvish
are static-only:

```bash
# Bash (current session)
source <(hamstik completion bash)
# Bash (persistent)
hamstik completion bash > ~/.local/share/bash-completion/completions/hamstik

# Zsh
hamstik completion zsh > "${fpath[1]}/_hamstik"

# Fish
hamstik completion fish > ~/.config/fish/completions/hamstik.fish

# PowerShell
hamstik completion powershell | Out-String | Invoke-Expression
```

The generated command reference lives in
[`docs/reference/`](docs/reference/) (one page per command) with
[man pages](docs/man/hamstik.1) shipped in every release archive.

For the common read → search → edit → transition → comment loop, print the
copy-pasteable cookbook derived from the bundled Agent Skill:

```bash
hamstik commands --cookbook          # human-readable examples
hamstik commands --cookbook --json   # structured steps for automation
```

Replace the `<ORG>`, `<KEY>`, `<ITEM-KEY>`, and `<COMMENT.md>` placeholders.
Every example uses `--json --no-input` and passes `--org`/`--project`
explicitly instead of relying on stored defaults.

```bash
# Authentication, identity, and context
hamstik init
hamstik auth login --with-token
hamstik me --json
hamstik context init --org acme --project HAM

# Organizations, members, Projects, and Sprints
hamstik org list --all
hamstik org members acme --search steven
hamstik project create --name "Website" --key WEB --color '#3b82f6'
hamstik project edit WEB --description "Public site"
hamstik sprint create --name "September" --goal "Ship v1" --target-points 40
hamstik sprint transitions 11111111-1111-4111-8111-111111111111
hamstik sprint transition 11111111-1111-4111-8111-111111111111 active

# Release versions, announcements, and audit packages
hamstik release create --name "Hamstik 0.4.0" --display-version 0.4.0
hamstik release list --state planned --include-archived
hamstik release view 22222222-2222-2222-2222-222222222222
hamstik release edit 22222222-2222-2222-2222-222222222222 --clear-target-date
hamstik release transitions 22222222-2222-2222-2222-222222222222
hamstik release transition 22222222-2222-2222-2222-222222222222 released \
  --confirm-incomplete-scope --reason "GA day"
hamstik release scope 22222222-2222-2222-2222-222222222222 --search checkout --all
hamstik release item add HAM-42 22222222-2222-2222-2222-222222222222
hamstik release announcement draft show 22222222-2222-2222-2222-222222222222
hamstik release announcement publish 22222222-2222-2222-2222-222222222222 --confirm-empty
hamstik release audit generate --kind register --from 2026-09-01T00:00:00Z \
  --through 2026-09-30T00:00:00Z
hamstik release audit get 77777777-7777-7777-7777-777777777777

# Organization Milestones (Organization context)
hamstik milestone create --name "Q4 hardening" --target-date 2026-12-31
hamstik milestone list --state in_progress
hamstik milestone view 44444444-4444-4444-4444-444444444444
hamstik milestone release add 44444444-4444-4444-4444-444444444444 \
  22222222-2222-2222-2222-222222222222
hamstik milestone events 44444444-4444-4444-4444-444444444444

# Advanced Reports and Dashboards (Organization context)
hamstik --org acme report list --visibility organization --all
hamstik --org acme report create --file report.json
hamstik --org acme report run 11111111-1111-4111-8111-111111111111 --json
hamstik --org acme dashboard list --all
hamstik --org acme dashboard run 44444444-4444-4444-8444-444444444444 \
  --filters-file dashboard-filters.json --json

# Structured Work Item filters, Organization Work, and My Work
hamstik work list --status todo --status in_progress --label-name api \
  --sprint none --top-level --sort dueDate --fields title,status,dueDate
hamstik org work --project HAM --priority urgent --overdue true
hamstik work mine --scope open --project HAM --project WEB --all
hamstik work triage --org acme --project HAM
hamstik work triage --org acme --project HAM --since 7d --activity 20 --json

# Read-only multi-project overview (My Work plus one section per Project)
hamstik work dashboard --org acme --project HAM --project WEB --json

# Read-only kanban board (Sprint or Project scope)
hamstik board view --org acme --project HAM
hamstik board view --org acme --project HAM --sprint 11111111-1111-4111-8111-111111111111 --all --json

# Parent/child hierarchy with bounded depth and server-reported link annotations
hamstik work tree --org acme --project HAM HAM-42 --depth 4 --links --json

# SqueakQL validation and read-only JSON-body search
hamstik squeakql validate 'status = todo and priority >= high'
hamstik work search 'status = todo and priority >= high' --limit 100 --json
# Forward server-provided query-plan hints when the API exposes them
hamstik work search 'status = todo' --explain --json

# Work Item lifecycle and optimistic concurrency
hamstik work create --title "Document API" --type task --assignee me
# Start from an existing Work Item or a local Markdown/YAML-frontmatter template
hamstik work create --from HAM-42 --title "Recurring bug report"
hamstik work create --template bug-template.md
# Hand off a Work Item as Markdown and re-import it elsewhere
hamstik work export HAM-42 --format markdown --comments > HAM-42.md
hamstik work import --file HAM-42.md --json
hamstik work edit HAM-42 --priority high --parent HAM-7
# Organization-governed Attributes; use stable keys in API, CLI and queries.
hamstik attribute list --org acme --include-retired --json
hamstik attribute create --org acme --key product_area --name "Product Area" --type multi_select
hamstik attribute option add --org acme product_area --key mobile --label "Mobile"
hamstik attribute project list --org acme --project HAM --json
hamstik attribute project enable --org acme --project HAM product_area --reason "Used by this Project"
hamstik work create --org acme --project HAM --title "Classify request" \
  --attribute-option product_area=search --attribute-option product_area=mobile
hamstik work edit --org acme --project HAM HAM-42 --attribute-boolean verified=false
hamstik work edit --org acme --project HAM HAM-42 --clear-attribute customer
hamstik work search --org acme "attribute_verified = FALSE OR attribute_customer IS NULL"
hamstik work search --org acme \
  "attribute_has_any('product_area', 'search', 'mobile') OR attribute_has_all('product_area', 'search', 'mobile')"
hamstik work list --org acme --project HAM --fields key,title,attributes --json
hamstik work transitions HAM-42
hamstik work transition HAM-42 in_progress
hamstik work archive HAM-42
# --force is explicit last-write-wins (If-Match: *); it is never the default.
hamstik work unarchive HAM-42 --force

# Bulk request files map directly to the Public API operation-array schemas
hamstik work bulk create --operations-file create-operations.json --json
hamstik work bulk update --operations-file update-operations.json \
  --concurrency require-revision --json
hamstik work bulk transition --operations-file transition-operations.json \
  --concurrency last-write-wins --json

# Convert a CSV into the same operations envelope and pipe it into bulk
hamstik work bulk from-csv items.csv --op create --project HAM \
  | hamstik work bulk create --operations-file -

# Run a large set in resumable batches of at most 50, journaling each result
hamstik work bulk run --op create --operations-file operations.json \
  --journal create.journal.jsonl --json
# JSON-lines and stdin keep pipelines from materializing one huge file
jq -c '.[]' operations.json \
  | hamstik work bulk run --op create --operations-file - \
      --journal create.journal.jsonl
# Resume from the journal alone; completed batches are skipped
hamstik work bulk run --op create --journal create.journal.jsonl --json

# Dry-run: preview the exact mutation without sending it
hamstik work create --title "Document API" --dry-run --json
hamstik work edit HAM-42 --priority high --dry-run --json
hamstik work bulk create --operations-file create-operations.json --dry-run --json

# Explain configuration precedence for every resolved setting (offline)
hamstik context explain --json

# SqueakQL from a file or stdin (no fragile shell quoting)
hamstik squeakql validate --file query.sqql
printf 'status = todo' | hamstik squeakql validate --file - --json
hamstik squeakql save urgent 'priority >= urgent'
hamstik work search --saved urgent --json
# Inspect or clear the local saved-query cache (fully offline)
hamstik squeakql cache size --json
hamstik squeakql cache clear

# Long-form authoring in $VISUAL/$EDITOR
hamstik work create --title "New design" --description-editor
hamstik work comment add HAM-42 --body-editor
hamstik --no-input work comment add HAM-42 --body-file - < comment.md

# Watcher state (the authenticated user's own; never other watchers)
hamstik work watcher show HAM-42 --json
hamstik work watcher watch HAM-42
hamstik work watcher mute HAM-42
hamstik work watcher unmute HAM-42

# Follow activity and comments live; Ctrl-C stops cleanly. Unlike `work await`
# (which exits once one condition holds), `work watch` streams every new entry.
hamstik work watch HAM-42 --since 1h
hamstik work watch HAM-42 --notify 'notify-send Hamstik "$HAMSTIK_WATCH_JSON"'
hamstik --json work watch HAM-42

# Labels, links, threaded comments, and attachments
hamstik label create --name api --color '#6366f1'
hamstik work label add HAM-42 --label api
hamstik work link add HAM-42 --target-key WEB-9 --relation blocks
hamstik work comment add HAM-42 --body "Ready for review"
hamstik work comment add HAM-42 --body "Agreed" --parent COMMENT_UUID
hamstik work attachment upload HAM-42 ./design.png --content-type image/png
hamstik work attachment download HAM-42 ATTACHMENT_UUID --output ./design.png
hamstik work attachment view HAM-42 ATTACHMENT_UUID

# Public user profile resources use immutable usr_ identifiers
hamstik user view usr_cPbfeqnghA-RLpDVOMQhHg
hamstik user work usr_cPbfeqnghA-RLpDVOMQhHg --involvement created
hamstik user activity usr_cPbfeqnghA-RLpDVOMQhHg --since 2026-09-01T00:00:00Z
hamstik user avatar usr_cPbfeqnghA-RLpDVOMQhHg --image-format webp --output avatar.webp
```

Downloads always write binary data to a file. In `--json` mode stdout contains
only JSON metadata (path, size, content type, and available response headers),
never the binary payload. Diagnostics and structured failures go to stderr.

### Working the parent/child tree

`hamstik work tree <KEY>` renders an item's descendants in one invocation,
composed only from the existing Public API v1 reads (the root's full Work Item
and each node's `parent=` child listing). `--depth N` (default `3`, maximum
`10`) caps descendant levels below the root; `--max-nodes N` (default `200`,
maximum `2000`) caps the total nodes rendered; `--links` adds each node's
server-reported `blocks` / `blocked_by` / `relates` links as annotations. The
CLI never computes whether an item is "ready" or "blocked".

```bash
hamstik --org acme --project HAM work tree HAM-42
hamstik --org acme --project HAM work tree HAM-42 --depth 5 --links --json
```

A traversal that ends early says so explicitly: the human view appends a
`[depth limit reached; children not expanded]`, `[node limit reached; …]`, or
`[cycle detected; …]` line, and the JSON `truncated` array carries the same
reasons. The `--json` document is stable and versioned
(`treeVersion: 1`):

```json
{
  "treeVersion": 1,
  "organization": "acme",
  "project": "HAM",
  "depth": 3,
  "maxNodes": 200,
  "truncated": false,
  "root": {
    "key": "HAM-42",
    "title": "Ship the release",
    "type": "epic",
    "status": "in_progress",
    "priority": "high",
    "parent": null,
    "children": [],
    "truncated": []
  }
}
```

Every `status`, `type`, and `priority` is the server's value verbatim. Each node
has the same shape nested under `children`; `truncated` entries are
`{"reason": "depth"|"nodes"|"cycle", "message": "…"}`. With `--links`, each
node also carries a `links` array of the raw server link objects (the key is
absent otherwise). `--json` reorders a node's children by Work Item key so
repeated runs are byte-identical; the hierarchy itself (which items are nested
under which) is server data.

### Advanced Reporting

Advanced Reports and Dashboards are Organization-scoped, capability-gated
Public API resources. A Premium plan, the enabled Advanced Reporting App,
Organization membership, the corresponding Advanced capability, and suitable
`report:read`/`report:write` PAT scopes are enforced by the server. The CLI
preserves those error codes and never guesses entitlement locally.

`report create` and `report edit` accept the complete `AdvancedReportInput`
JSON document documented by the checked-in OpenAPI contract; `-` reads stdin.
Edits and deletes fetch the current ETag unless `--force` explicitly requests
`If-Match: *`. Runs fetch the current resource revision and supply
`expectedRevision` automatically:

```bash
hamstik --org acme report create --file report.json --json
hamstik --org acme report edit <REPORT_ID> --file report.json --json
hamstik --org acme report run <REPORT_ID> --json
hamstik --org acme report selection-items <RUN_ID> <CELL_ID> --all --json
hamstik --org acme dashboard run <DASHBOARD_ID> \
  --filters-file filters.json --json
```

The Public API currently exposes Dashboard reads and evaluations, not Dashboard
create/edit/delete. Report and Dashboard results are emitted exactly as the
server returns them; the CLI does not recompute datasets, selections, widgets,
or aggregations.

## Dry-run previews

Mutation commands accept a global `--dry-run` flag: the CLI resolves every
input exactly as a real invocation would (Organization, Project, target
identifiers, inline/file/stdin sources, request body, concurrency mode),
validates local input shape and size, and then prints a labeled preview
instead of sending anything.

- `--json` emits a versioned envelope (`previewVersion: 1`) with the
  operation name, method, path template and fully resolved path, the header
  intent (`If-Match` and `Idempotency-Key` — never `Authorization` or any
  credential material), the typed request body, and the resolved targets.
  Upload previews carry file metadata (name, type, size), never the bytes.
- Human mode prints a `dry-run: METHOD /path (client-side preview only; …)`
  summary plus headers, body, and notes. Quiet mode prints only the summary
  line.
- Previews are client-side only: they are not proof that server validation,
  authorization, concurrency, or idempotency will succeed. No mutation
  request is sent and no idempotency key is consumed.
- For revision-protected operations (`work edit`, `work delete`,
  `work archive`/`unarchive`, `project edit`, `project archive`/`unarchive`,
  `sprint transition`, `work label add|remove`, `report edit|delete`), dry-run performs the safe
  ETag read and shows the resolved `If-Match` value. With `--force`, the
  preview shows `If-Match: *` (explicit last-write-wins) and makes zero
  network requests.
- Read commands (`work list`, `work view`, `me`, `doctor`, …) reject
  `--dry-run` with a usage error.

## Destructive-operation consent

Five commands are destructive and require explicit per-process consent
before any request is sent:

- `work delete` (soft-delete a Work Item);
- `project archive`;
- `sprint transition <SPRINT-ID> done` (completing a Sprint);
- `release archive` (and `release transition <RELEASE-ID> archived`);
- `milestone transition <MILESTONE-ID> archived`.

Consent is expressed with the global `--confirm-destructive` flag. The
`--yes` flag is the documented scripting override and satisfies the same gate
without prompting. Interactive terminal sessions without either flag may
confirm at a `y/N` prompt; `--no-input` and `--json` never prompt, so
automation must pass one of the flags. Missing consent is a usage error
(exit 2) whose message names `--confirm-destructive`, and the server is never
contacted. `--dry-run` sends no mutation and needs no consent. The server
remains authoritative for authorization; this gate only records that consent
was expressed locally.

```bash
hamstik --no-input --org ACME --project HAM work delete HAM-42 --confirm-destructive
hamstik --no-input --org ACME project archive OLD --yes
```

## Context precedence diagnostics

`hamstik context explain` reports how every resolved setting won precedence —
host, Organization, Project, profile, credential source, plus color/input/retry
behavior — fully offline (no network connection, no prompts):

- each value shows its complete chain: the winning source and every lower-
  precedence source that was shadowed (`--flag` > environment variable >
  nearest `.hamstik.toml` > profile/global config > built-in default);
- JSON output uses stable `source` and `status` enums (`winner`, `shadowed`,
  `unset`) for CI troubleshooting;
- HAMSTIK_TOKEN is represented only as present/absent — its value, stored
  credential values, and proxy variables are never displayed;
- context-file discovery (searched-from directory, filename, found path) is
  included so an unexpected Organization or Project can be traced to the file
  that set it;
- discovery is purely additive: after the directory walk-up finds nothing, a
  linked git worktree falls back to the primary checkout (`git rev-parse
  --show-toplevel`/`--git-common-dir`), mapping the current subtree onto the
  primary checkout, so a worktree can reuse the primary checkout's
  `.hamstik.toml` without copying it. A file inside the worktree still wins.

Diagnosing an unexpected host, Organization, Project, or profile:

```bash
hamstik context explain                 # human summary with remediation
hamstik context explain --json          # stable shape for CI
```

## Configuration drift advisory

When a command already fetches the authenticated identity from
`GET /api/v1/me` — `hamstik me`, `hamstik auth login`/`status`, the
ephemeral-token profile bootstrap used by `hamstik org use`/`project use`, or
`hamstik work create`/`edit --assignee me` — the CLI compares the resolved
Organization against the memberships in that response. If the resolved
Organization is not a member, a non-blocking warning is written to stderr
naming both the configured value and the actual memberships:

```text
warning: configuration drift: resolved organization "other-org" (.hamstik.toml) is not among the authenticated user's memberships [acme]; the command continued and the context was not changed
```

The warning never changes the exit code and never switches context
automatically. Commands that do not already hold membership data make no extra
request to obtain it, so the check degrades silently rather than slowing every
command. It is also skipped when `GET /me` returns no Organizations (offline,
or a token without the membership scope). Because the Public API's `/me`
exposes Organization memberships only, Project drift is reported only through
its Organization.

## Ambiguous references (interactive)

Organization slugs, Project keys, and Work Item keys resolve through the
Public API exactly as before. On a real TTY, when an exact lookup returns
`NOT_FOUND` for `context set`, `org use`, `project use`, `work context`, or a
single-key `work view`, the CLI lists the plausible candidates and asks for an
explicit choice (Esc/Ctrl+C cancels without changes). A unique fuzzy match
resolves without a prompt.

The picker is interactive-only. `--no-input`, `--json`/`--jsonl`/`--tsv`,
`--quiet`, and non-TTY stdin/stdout keep the original fail-fast `NOT_FOUND`
error and never list candidates, so scripts, agents, and CI are unaffected.

## Editor authoring and stdin conventions

Long-form text (Work Item descriptions, comment bodies, project descriptions)
can be supplied inline, from a file, from stdin, or through your editor:

- `--description-editor` / `--body-editor` launch `$VISUAL` (then `$EDITOR`,
  then `hamstik config set editor <command>`, then a platform default) on a
  secure temporary file (owner-only
  permissions, removed on every exit path). Saving accepts the content;
  leaving it empty/unchanged cancels the command without sending anything.
- `--*-file <PATH>` accepts a path, or `-` for stdin — the same convention on
  every text input, including `work search --file -` and saved-query input.
- Supplying more than one text source is a usage error at argument-parse
  time (exit 2), before any network activity.
- Under `--no-input`, editor authoring is rejected immediately; scripts
  should pipe through `--file -` instead.
- Content is preserved verbatim: Unicode, Markdown (including `#` headings),
  and final-newline semantics are not normalized.
- A failed or unchanged editor session is a clean cancellation: nothing is
  sent.

## Work Item starting points (`work create --from` / `--template`)

`hamstik work create` can seed a new Work Item from an existing item or a
local file. Both are client-side conveniences: the CLI still builds an ordinary
`POST .../work-items` request, the server remains authoritative, and no
template is persisted server-side.

- `--from <KEY>` copies the documented allow-list — `title`, `type`,
  `priority`, `description`, and `labels` — from an existing Work Item.
  Identity, ownership, workflow state, revision, sprint, parent, story points,
  and due date are never copied.
- `--template <FILE>` reads a Markdown file with YAML frontmatter (`-` for
  stdin). Recognized keys are `title`, `type`, `priority`, `labels`, and
  `description`; the Markdown body is the description when the frontmatter
  does not set one. Unknown keys (including `assignee`, `reporter`, and
  `status`) are rejected locally rather than silently ignored.
- Any explicit flag (`--title`, `--priority`, …) overrides the copied value.
- `--from` and `--template` are mutually exclusive and fail locally (exit 2)
  before any request.
- `--dry-run` resolves the source and previews the exact create payload.
  Labels are not part of the create body (the Public API v1 contract forbids
  unknown properties), so the preview names them and a real invocation attaches
  each copied label through the documented label endpoint after create.

```markdown
---
title: Bug report
type: bug
priority: high
labels:
  - regression
---

## Steps to reproduce

## Expected

## Actual
```

## Context and automation contract

Host, Organization, and Project values resolve in this order:

```text
command-line flag
→ HAMSTIK_HOST / HAMSTIK_ORG / HAMSTIK_PROJECT
→ nearest .hamstik.toml
→ selected profile defaults
→ built-in defaults
```

`HAMSTIK_PROFILE` selects a profile, `HAMSTIK_TOKEN` supplies an ephemeral
PAT without reading or writing the credential store, and
`HAMSTIK_CA_BUNDLE` supplies an additional PEM trust bundle. Explicit
`--host`, `--profile`, `--org`, `--project`, and `--ca-bundle` flags
override their environment equivalents. `HAMSTIK_IMAGE_PROTOCOL`
(`kitty`, `iterm2`, `sixel`, or `none`) overrides the inline-image
protocol used by `work attachment view`.

### Color and terminal profile

Color output resolves with the precedence `flag > environment > auto`:

```text
--color=always | --color=never   (explicit flag; highest precedence)
--no-color                       (alias for --color=never)
HAMSTIK_NO_COLOR                 (non-empty disables color)
CLICOLOR_FORCE                   (non-empty and not "0" forces color)
NO_COLOR                         (non-empty disables color)
CLICOLOR=0                       (disables color)
terminal detection               (auto: non-TTY stdout, unset/dumb TERM)
```

`--color=auto` (the default, and equivalent to setting no flag) honors the
environment variables and then falls back to terminal detection. `--color`
and `--no-color` are mutually exclusive. `--json`, `--jsonl`, and `--tsv` are
always ANSI-free regardless of `--color`, so `NO_COLOR=1` and `--color=never`
produce identical ANSI-free output.

`HAMSTIK_TERM=auto|unicode|ascii` selects the decoration profile. `auto` (the
default, also unset/empty) preserves existing detection; `unicode` (aliases
`utf8`, `utf-8`) forces Unicode decoration and emoji on; `ascii` (alias
`plain`) replaces CLI decoration with ASCII stand-ins (`[##]` swatches,
`-`/`|`/`->` separators), suppresses emoji, and pins width-sensitive layout
(e.g. `board view`) to a fixed width. This keeps captured CI logs byte-stable
independent of the host terminal. The profile never rewrites server-provided
content, which is rendered verbatim.

For automation:

- `--json` writes a single valid JSON success document to stdout; structured
  command failures go to stderr. `doctor` is the deliberate exception: its
  diagnostic report stays on stdout even when its exit code is nonzero;
- collection commands support `--jsonl` for one compact, server-shaped resource
  per line and `--tsv` for escaped tab-separated table rows. TSV includes a
  header by default; `--no-header` suppresses it. On a single-resource command
  both modes emit the resource as one compact JSON line, and `--jsonl` carries
  no page envelope — read pagination from `--json`. `--all` completes its
  traversal in API order before writing stdout, so a run that fails halfway
  emits no records and is resumed from a `page.nextCursor` read in `--json`
  mode;
- `--format FORMAT` selects the output mode by name: `ndjson`/`jsonl`
  (JSON Lines), `tsv`, `csv`, `table`/`human`, `json`, or `markdown`. It is
  the umbrella spelling of the dedicated `--json`, `--jsonl`, `--tsv`, and
  `--quiet` flags, which it conflicts with. `markdown` emits a
  GitHub-flavored table for list-shaped output, so a `work list` result can be
  pasted straight into a pull request or issue. `csv` follows RFC 4180
  quoting. `--json` output is unaffected by `--format`;
- `--columns NAME...` (space-separated) or `--fields a,b,c` (comma-separated)
  selects and orders the human/TSV/CSV/Markdown table columns by their printed
  header names; an unknown name fails with the list of valid names. Every table
  column carries a header name, so none is unreachable. On the Work Item list
  commands (`work list`, `work mine`, `org work`, `user work`) `--fields` is
  also the server-side sparse fieldset and is forwarded verbatim; a name that
  is not a documented Work Item field fails as a usage error. Because
  `--columns` takes multiple values, write it after the subcommand
  (`hamstik work list --columns KEY TITLE`); before the subcommand it would
  consume the command name as a column name;
- `--jq EXPR` filters the structured document of any command — the full
  collection envelope for list commands, the resource document otherwise — in
  `--json`, `--jsonl`, or `--tsv` mode. `--json` coalesces the filter results
  into one JSON document (`null` for none, the value for one, an array for
  many), `--jsonl` writes one result per line, and `--tsv` writes one result per
  row where an array becomes multiple cells. `--jq` and `--columns` cannot be
  combined, and an invalid expression is rejected before any network call;
- `--quiet` emits only the essential identifier or result;
- `--no-input` disables prompts and `--no-retry` disables safe automatic
  retries;
- `--confirm-destructive` consents to the destructive operation an invocation
  performs (`work delete`, `project archive`, completing a Sprint, archiving a
  release version, archiving a milestone); `--yes`
  is the scripting override. Destructive commands require one of them even
  outside `--no-input` (see [Destructive-operation consent](#destructive-operation-consent));
- `--cursor` requests the page that follows an opaque cursor — `--since-cursor`
  is the pipeline-checkpoint spelling of the same option — while `--all` starts
  at the first page, follows every returned cursor, and emits one deterministic
  aggregate; they compose, so a stream a downstream command crashed halfway
  through resumes with
  `--since-cursor "$(…previous output… | jq -r .page.nextCursor)" --all`
  without re-reading what was already emitted;
- `--limit` caps the total items a command emits, which is not the server page
  size: `--limit 40` is a single 40-item page, `--all --limit 400` walks pages of
  at most 200 items and stops at 400. A page is never larger than the endpoint
  allows (200 items, 100 on the organization, project, sprint, label, and link
  endpoints). A cap that cuts through a server page ends with
  `"nextCursor": null` and `hasMore: true`, because the Public API has no cursor
  for a position inside a page;
- `--sort KEY[:DIR]` orders by `updated`, `dueDate`, `priority`, or `rank` with
  an optional `:asc`/`:desc` direction. Ties always break on the Work Item key,
  so the same query repeats byte for byte; `rank` and `rank:asc` keep the
  server's rank order and `rank:desc` reverses it. The REST `sort` parameter has
  no direction, so the CLI applies it to the result it fetched: order a whole
  collection with `--all`, because without `--all` only the single page that was
  returned is reordered;
- `--json`, `--jsonl`, `--tsv`, and `--quiet` are mutually exclusive. The
  `--format` umbrella selects the same modes by name (`--format ndjson`,
  `--format tsv`, …) plus `csv`, `markdown`, and the explicit `table`/`json`
  spellings, and conflicts with the dedicated mode flags.

Line-oriented pipelines choose the mode that matches the consumer:

```bash
# Spreadsheets: one header row, then one row per Work Item. Import the file as
# tab-delimited text; a cell containing a tab, newline, or backslash carries the
# literal escapes \t, \n, \r, and \\, so a record is always one physical line.
hamstik work list --all --tsv --columns KEY TITLE STATUS ASSIGNEE --no-input > work.tsv

# A processor that supplies its own column names wants no header row.
hamstik work list --all --tsv --no-header --no-input | cut -f1

# Every --jsonl line is one complete, server-shaped resource and nothing else
# reaches stdout, so any line-oriented reader works.
hamstik work list --all --jsonl --no-input | grep -c '"status"'

# Embedded jq filters the command's own structured output: no external jq,
# no shell-out, and one result per line when paired with --jsonl.
hamstik work list --json --jq ".items[].key" --no-input
hamstik work list --jsonl --jq ".items[].title" --no-input

# The same reads through the external jq binary are equally valid, and the
# resume checkpoint comes from the --json envelope -- which is why --jsonl
# never prints pagination of its own.
hamstik work list --all --json --no-input | jq -r ".items[].key"
hamstik work list --all --json --no-input | jq -r ".page.nextCursor"

# GitHub-flavored Markdown for a pull request or issue comment. Pipe a
# spreadsheet out as RFC 4180 CSV instead.
hamstik work list --format markdown --columns KEY TITLE STATUS ASSIGNEE --no-input
hamstik work list --format csv --columns KEY TITLE STATUS ASSIGNEE --no-input > work.csv
```

Stable process exit codes are:

| Code | Meaning |
| ---: | --- |
| 0 | Success |
| 1 | General/internal failure |
| 2 | Invalid input or usage |
| 3 | Authentication failure |
| 4 | Authorization or insufficient scope |
| 5 | Resource not found |
| 6 | Conflict, revision conflict, or idempotency conflict |
| 7 | Rate limited |
| 8 | Network/transport failure |
| 9 | Server or API compatibility failure |
| 10 | Local configuration or credential-store failure |

### What is stable, and how breaking changes are communicated

The following surfaces are covered by compatibility contract tests
(`crates/hamstik-cli/tests/contract.rs`) and are stable across releases:

- the root command set, each command group's subcommands, documented command
  aliases (including `work mine` / `work my`), and the global option set;
- documented option names, conflicts, and enum spellings (`--status`,
  `--type`, `--priority`, `--scope`, `--sort`, `--involvement`, `--relation`,
  `--concurrency`, sprint states);
- the versioned JSON failure envelope (`{ "error": { kind, code, message,
  requestId?, status?, fieldErrors?, details? } }`) and the exit-code table
  above;
- stdout/stderr separation (success content on stdout, failures on stderr
  with empty stdout), `--quiet` identifier output, `--no-input` deterministic
  failure, and the structured-output/`--quiet` mutual exclusion;
- the collection envelope (`items` + `page.limit`/`hasMore`/`nextCursor`),
  the `--all` aggregate shape, JSON Lines resource stream, TSV escaping and
  column order, jq result shaping, and sparse `--fields` forwarding;
- the `--dry-run` preview envelope (`previewVersion: 1` with `operation`,
  `request.method`/`pathTemplate`/`path`/`headers`, `resolved`, `body`,
  `notes`) for mutation commands;
- mutation response metadata (revision echo, idempotent-replay tolerance).

Human table rendering may evolve; machine-readable surfaces above require a
reviewed, intentional change. Deliberate breaking changes are listed in the
changelog's `### Breaking` section and versioned per
[`design/VERSIONING.md`](design/VERSIONING.md). Non-breaking additions (new
commands, flags, or JSON fields) are announced in the changelog's Added/
Changed sections.

## Credentials, profiles, and logout

`hamstik auth login` validates the PAT against `GET /api/v1/me` before storing
anything. Interactive login prompts for the token; `--with-token` reads it from
stdin. `HAMSTIK_TOKEN` is deliberately not a login source: an environment token
is ephemeral by contract and is never written to the credential store
(SPEC §27). Two things are then kept in two different places:

| What | Where |
| --- | --- |
| The PAT itself | OS credential store — Windows Credential Manager, macOS Keychain, or the Linux Secret Service (GNOME Keyring / KWallet) |
| Profile metadata (host, user id, email, defaults) | `config.toml` next to the other configuration (`hamstik auth list` reads this) |

The CLI never falls back to plaintext storage. If no secure credential store is
reachable — a headless Linux box with no Secret Service, for example — every
login/logout fails with an explanation instead of writing the token to disk; use
`HAMSTIK_TOKEN` for headless automation (see SPEC §26). `hamstik doctor`
reports credential-store availability when stored credentials are in use. When
`HAMSTIK_TOKEN` is active, that check is explicitly reported as skipped
instead of touching the store.

`hamstik auth logout` is a **local logout**: it removes the stored PAT for the
selected profile and does not revoke the token server-side (the Public API has
no PAT-revocation endpoint yet). Profile metadata is intentionally kept, so
`hamstik auth list` still shows the profile after logout and `auth status`
reports `no stored credential` until you log in again. Logging out twice is
idempotent and reports `already logged out`. `hamstik doctor` treats that
retained, logged-out profile as an expected skipped credential source rather
than a failure; authenticated checks are skipped until the profile logs in
again.

To remove a profile entirely — credential *and* its `config.toml` entry — use
`hamstik auth forget [PROFILE]` (the selected profile when no name is given).
This is still local only: the PAT remains valid server-side, so revoke it in the
Hamstik web UI if that matters. If the profile being forgotten was active, the
slot is refilled only when exactly one profile remains; otherwise no profile is
active until `auth login` or `auth switch` picks one, because guessing would
silently change which host your commands talk to. If the credential store is
unreachable, the profile entry is still removed and the exact account key is
printed so the orphaned secret can be deleted by hand.

### Rotation, renewal, and CI usage

Rotation: create a new PAT in the Hamstik web UI, then `hamstik auth login
--with-token` (reads one line from stdin). The new credential replaces the
stored one for that identity; the old PAT stays valid server-side until you
revoke it in the web UI. Credentials that are expired or within 14 days of
expiry warn on every authenticated run (`auth status`, `me`, `doctor`) with an
actionable renewal message; `auth status` reports an expired credential with
the stable authentication exit code (3) while keeping the full JSON report on
stdout.

CI usage: prefer an ephemeral environment token (`HAMSTIK_TOKEN`) injected by
your CI secret store. It takes precedence over stored credentials for every
command, is never read from or written to the credential store, and is never
accepted as a `auth login` source. For a persistent CI credential, pipe the
PAT in with `--with-token` (never argv):

```bash
printf '%s\n' "$HAMSTIK_PAT" | hamstik --no-input auth login --with-token
hamstik --no-input --json auth status
```

`hamstik --json --no-input auth status` reports the credential type and name,
expiry, the credential source (`environment` vs `credential store`), the
granted-scope inventory, and structured errors (code, HTTP status, request ID)
for invalid, expired, revoked, malformed, or insufficiently scoped tokens —
never any secret material. The server decides per-command authorization; an
`INSUFFICIENT_SCOPE`/`FORBIDDEN` failure (exit 4) names the missing authority
exactly, and `doctor`'s `scope.readiness` check surfaces the granted scope
list so missing capabilities can be identified before a mutation runs.

### When the configuration file is broken

A missing `config.toml` is normal and treated as an empty configuration. A file
that exists but cannot be understood (malformed TOML, an unknown field, an
unsupported schema version, unreadable permissions) is always an error, never
silently ignored — that would discard profiles and context defaults. The failure
names the offending file, points at a repair or removal hint, and exits with the
configuration exit code (10). `hamstik doctor` reports the same problem as a
`FAIL` line instead of refusing to run. Independent checks continue; checks
that depend on unavailable configuration are reported as skipped with a reason.
The same applies to a broken project-local `.hamstik.toml`, which is identified
separately from the global configuration file.

## Doctor diagnostics

`hamstik doctor` performs dependency-aware checks for:

1. global configuration readability;
2. local context discovery, parsing, and source resolution;
3. the local mutation audit log and the failed-request journal (effective
   paths, size, or opt-out);
4. profile and credential-source resolution;
5. credential-store configuration and accessibility when used;
6. host validation and proxy-environment detection;
7. network reachability and TLS validation;
8. the unauthenticated `/api/v1/openapi.json` route;
9. Public API v1 compatibility;
10. PAT authentication through `/api/v1/me`;
11. PAT expiration (expired / imminently expiring within 14 days / healthy);
12. scope readiness for the credential's granted scopes;
13. selected Organization and Project accessibility;
14. terminal color and emoji rendering.

Human output uses `ok`, `WARN`, `FAIL`, and `skip` markers and includes
concrete remediation hints, a final `summary: N passed, N warned, N failed,
N skipped` line, and a pass/warn/fail/skipped summary object in JSON mode.
Network failures are classified by the earliest failing stage — DNS, TCP
connection, proxy, timeout, TLS — with stage-specific remediation and
per-check latency (`durationMs`). Independent checks continue after a failure;
dependent checks are marked `skip`. The first blocking root cause determines
the stable process exit code. Additive Public API operations produce a warning,
while a missing or moved operation required by this CLI is an API compatibility
failure.

A selected profile with no stored credential is an intentional logged-out
state: credential, authentication, Organization, and Project checks that need a
PAT are reported as `skip`, not `FAIL`. An inaccessible credential store, an
invalid explicit `HAMSTIK_TOKEN`, or a rejected stored credential remains a
failure.

`hamstik doctor --json` adds stable check IDs, explicit
`pass|warn|fail|skipped` statuses, `networkStage` on transport failures, and
structured error details. It preserves API error codes, HTTP status, and
server request IDs when available. Terminal visual samples are printed only
for interactive human output and never enter JSON or piped output. Proxy
values, `Authorization` headers, and the PAT itself are always redacted.

### Local-only mode

`hamstik doctor --local-only` verifies configuration, context resolution,
profile selection, credential-store accessibility, terminal behavior, and the
bundled OpenAPI compatibility metadata **without any network traffic**: remote
checks are reported as `skipped`. This makes the command safe for support
bundles and air-gapped environments.

Support-bundle guidance: prefer `doctor --json --local-only`; the JSON report
contains no secrets, tokens, or proxy values by construction.

## Local mutation audit log

Every mutation the CLI performs leaves a local trail. Each successful
state-changing command appends exactly one JSON line to an append-only audit
log, so `what ran, when, on what, and which server request it corresponded to`
can be reviewed without network access:

```json
{"when":"2026-01-02T09:12:44Z","command":"work.transition","target":"HAM-42","revisionBefore":7,"revisionAfter":8,"requestId":"0f2c9b1e"}
```

| Field | Meaning |
| --- | --- |
| `when` | RFC 3339 timestamp taken when the record was written |
| `command` | Command path, e.g. `work.create`, `project.archive`, `api.request`, `config.set` |
| `target` | The identifier acted on (Work Item key, Project key, Sprint id, comment id, config key, context file) |
| `revisionBefore` | Revision the CLI saw before the mutation, `null` when it had none (creates, `--force`, ungoverned mutations) |
| `revisionAfter` | Revision reported by the server afterwards, `null` when the operation returns none (deletes, bulk, passthrough) |
| `requestId` | Server request id, for correlating with the authoritative server-side audit record |

Location (the state directory, not the config directory):

- Linux: `$XDG_STATE_HOME/hamstik/audit.log`, default `~/.local/state/hamstik/audit.log`
- macOS: `~/Library/Application Support/hamstik/audit.log`
- Windows: `%LOCALAPPDATA%\hamstik\audit.log`

`HAMSTIK_AUDIT_LOG` pins the path (tests, containers, automation), and
`hamstik doctor` always prints the effective location. On Unix the file is
created owner-only (`0600`).

### What is never recorded

Credentials of any kind, `Authorization` headers, environment token values,
request or response bodies, and work content such as titles and descriptions.
Records carry identifiers only. Raw passthrough mutations
(`hamstik api request POST …`) are recorded as `api.request` with the method and
path — never the body, headers, or query string. The record is deliberately narrow because the
server-side audit record stays the authoritative account of who did what; this
log answers "what did this CLI ask for, from this machine".

### Rotation, size, and opt-out

The CLI never rotates, compresses, or trims the log: it is a plain text file of
one short line per mutation (roughly 200 bytes each) and grows only as fast as
you mutate. Deleting or truncating it at any time is safe and affects nothing
else. Treat it like shell history.

To stop writing it, set the documented opt-out:

```bash
hamstik config set audit_log false
```

The default is enabled. While disabled, `hamstik doctor` reports the audit log
as skipped and names the path it is not writing.

### Failure behavior

Audit logging is best-effort and never blocks work: if the record cannot be
written, the CLI prints `audit log write failed (...)` on stderr and the
mutation itself still completes with its normal exit code. A missing audit line
therefore means the request never succeeded (or logging is off) — the tradeoff
is intentional, and it is why the server-side audit record remains the record of
reference.

## Failed-request journal

When a Public API request fails, the CLI appends one redacted JSON line to a
local, append-only journal so a failure can be correlated with the
authoritative server-side diagnostics by request id:

```json
{"when":"2026-01-02T09:12:44Z","method":"GET","path":"/api/v1/organizations/acme/projects/HAM/work-items/HAM-1","headerIntent":["accept","authorization"],"requestId":"0f2c9b1e","status":500,"durationMs":184,"transport":false}
```

| Field | Meaning |
| --- | --- |
| `when` | RFC 3339 timestamp taken when the record was written |
| `method` | HTTP method (`GET`, `PATCH`, …) |
| `path` | Percent-encoded path under `/api/v1`; never the query string |
| `headerIntent` | Names of the headers the request intended to send (never values) |
| `requestId` | Server request id, for correlating with server-side diagnostics |
| `status` | HTTP status, `null` for a transport failure |
| `durationMs` | Elapsed time for the attempt(s), including retries |
| `transport` | `true` when the failure was transport/protocol-level, not HTTP |

Both server errors (4xx/5xx) and network failures are recorded. Review the
most recent entries with:

```bash
hamstik replay --last 20
hamstik --json replay --last 5
```

`replay` is strictly local: it reads the journal and prints it, never
re-sending a request and never contacting the network. It needs no host,
token, or context.

Location (the state directory, not the config directory):

- Linux: `$XDG_STATE_HOME/hamstik/request-journal.log`, default `~/.local/state/hamstik/request-journal.log`
- macOS: `~/Library/Application Support/hamstik/request-journal.log`
- Windows: `%LOCALAPPDATA%\hamstik\request-journal.log`

`HAMSTIK_REQUEST_JOURNAL` pins the path (tests, containers, automation), and
`hamstik doctor` reports the effective location. On Unix the file is created
owner-only (`0600`).

### What is never recorded

Tokens, `Authorization` values, credential-bearing proxy URLs, request or
response bodies, query strings, and work content such as titles and
descriptions. Records carry request shape and identifiers only. The server
remains authoritative; the journal only answers "what did this CLI ask for,
from this machine, and what did it get back".

### Retention and rollover

The journal is bounded by a documented policy: entries older than 7 days are
dropped, at most the 500 most recent entries are retained, and compaction runs
when the file grows beyond 256 KiB or its oldest entry passes 7 days. A healthy
journal is therefore append-only in the common case. Deleting or truncating it
at any time is safe and affects nothing else.

Writing is best-effort: a journal I/O failure never changes a command's
outcome, and a missing entry simply means the record could not be written.

## Bulk operations and preflight

Bulk request files are validated locally before any request is sent: JSON
syntax, the typed envelope, per-operation required fields, unknown fields,
enum spellings, revision constraints, and the 1–50 operation bound. Failures
identify the operation index and field path:

```text
ops.json failed bulk preflight with 2 problems:
  - operations[0].bogus: is not part of the Public API bulk schema (unknown fields are rejected)
  - operations[1].projectKey: must not be empty
```

Human bulk output states the selected concurrency mode (so
`last-write-wins` is always a visible, deliberate choice), summarizes
`bulk results: <succeeded>/<total> succeeded, <failed> failed`, and prints
actionable per-failure details (operation index, HTTP status, error code,
server message, request id, and a fix hint for revision conflicts, invalid
transitions, and missing targets). `--json` preserves every documented
per-operation field exactly as the server returned it. Preflight performs
local structure validation only; server-side validation, authorization, and
transition legality remain authoritative on the server.

`work bulk from-csv <FILE> --op create|update --project <KEY>` converts a local
CSV file into that exact operations array (write it with `--output <FILE>`, or
leave it on stdout to pipe into `--operations-file -`). The dialect is RFC 4180:
a required header row, comma-separated fields, `"` quoting, and `""` for an
embedded quote; an empty cell omits the field. `--op create` accepts `title`
(the only field the frozen bulk create envelope carries), and `--op update`
accepts `workItemKey` (required), `revision`, `title`, `description`, `type`,
`priority`, `assignee`, `sprint`, `parent`, `storyPoints`, and `dueDate`. The
converter never guesses: unknown columns, mis-spelled enums, and malformed rows
fail locally with the CSV row number and column name before any request, and the
result is re-run through the same bulk preflight and typed envelopes the JSON
path uses. Status and labels are not part of either bulk envelope (status is a
transition, labels use the label endpoints), so they are rejected rather than
silently dropped.

### Resumable bulk execution

`work bulk run --op create|update|transition --journal <FILE>` runs a large
operation set — larger than the single-request 50-operation limit — as a
sequence of batches of at most 50. It reads the operations source as a JSON
array or as JSON-lines (`--operations-file -` streams stdin one operation at a
time, so `jq`/`xargs`/agent pipelines never materialize one huge input file),
preflights every operation against the same schema-derived rules the
single-request commands use, and feeds the existing typed bulk envelopes. No
new bulk request format is introduced.

**Separate batches are separate API requests and are not one atomic
transaction.** A later batch can fail (or the process can be interrupted) after
earlier batches have already been applied. The `--json` envelope carries
`"atomic": false` and a `note`, and human output prints the same caveat.

Each batch's exact request body and idempotency key are written to the local
journal *before* the request is sent. Interrupting mid-run, killing the
process, or hitting a network failure leaves the journal intact; re-running the
same command with the journal (omit `--operations-file`) skips batches that
already completed and replays unfinished/uncertain batches with their original
body and idempotency key. Because the key is reused, the server's idempotency
replay window prevents duplicate creates. Failed batches are **never** retried
automatically; review the journal and re-run with `--retry-failed` to retry
them. `--restart` replaces an existing journal and starts planning over.

A journal records `completed`, `failed`, and `uncertain` outcomes. A batch is
`uncertain` when a transport failure left the outcome unknown; it is safe to
replay because the key and body are unchanged. If the server no longer
replays a key (for example because the replay window expired), the server's
conflict response is recorded and surfaced for explicit review — the CLI never
invents a new key or silently drops work. The journal belongs to one
Organization and operation kind; resuming it with a different `--org`, `--op`,
or `--concurrency` fails until `--restart` is passed.

#### Journal schema

The journal is a local, append-only JSON-lines file (one JSON object per
line). It contains no credentials, tokens, or request headers. Later records
supersede earlier ones for the same batch, and a torn final line from an
interrupted write is discarded on load.

| `type` | Fields | Written when |
| --- | --- | --- |
| `header` | `journalVersion`, `operation`, `organization`, `concurrency`, `source`, `createdAt` | once, before any plan |
| `batch` | `sequence`, `idempotencyKey`, `operations` | once per planned batch, before execution |
| `complete` | `at` | once, when planning finishes |
| `result` | `sequence`, `state`, `attemptedAt`, `recordedAt`, `results`, `error` | before and after each batch attempt |

`state` is `completed`, `failed`, or `uncertain`; `results` is the raw server
per-operation array for a completed batch, and `error` carries the stable
`code`, `message`, optional `status`, and optional `requestId` for a
failed/uncertain batch. The in-progress `uncertain` marker written before each
attempt is what makes a crash between send and response unambiguous.

## Export, import, and scheduled snapshots

`work export <KEY> --format markdown` writes one canonical document — YAML
frontmatter (`key`, `title`, `type`, `status`, `priority`, `labels`, and
optionally `links`/`comments`) plus the description as the Markdown body — so a
Work Item can be pasted into a GitHub/GitLab issue or handed to another
tracker. `--comments` includes the item's comments, and `--output <PATH>`
writes the document to a file. Pass the global `--json` to get the same fields
as a `documentVersion: 1` envelope instead of the Markdown text.

`work export --query <SQUEAKQL>` (or `--query-file`/`--query-saved`) instead
exports the matching Organization Work Items as a collection through the same
output contract as `work list`/`work search`: `--format csv|jsonl|tsv|json|
markdown|table`, `--columns`, and `--jq` all apply, and `--output <PATH>` writes
the exact bytes a stdout run would print.

`work import --file <PATH|->` reads that format and maps its fields onto the
existing create/edit requests. When the embedded `key` resolves in the selected
Project, the item is updated in place; otherwise a new item is created with an
idempotency key derived from the embedded key (or an explicit
`--idempotency-key`), so **re-importing the same document does not duplicate
items**. Labels, links, and comments are applied through the documented
endpoints and reconciled against the item's current state, so a re-import adds
nothing. `--dry-run` previews the mapped operations without sending a mutation,
and revision conflicts surface exactly as they do for `work edit`. The
`status` field is applied when import creates the item; on the update path it
is informational, because the Public API's edit body has no status field
(status changes go through the transition commands), and the `--dry-run`
preview calls this out.

`schedule list|save|delete` store periodic snapshot definitions as plain TOML
files under `<config-dir>/schedules/`. The CLI ships no daemon: an external
scheduler (cron, systemd timers, Task Scheduler) invokes
`hamstik schedule run <NAME>`, which re-executes the saved command through the
same binary and context, so a scheduled run is byte-identical to the manual
invocation. Definitions hold only a `hamstik` argument vector — never
credentials.

```bash
# Export a Work Item as a portable document (links always, comments on request)
hamstik work export HAM-42 --format markdown --comments > HAM-42.md

# Snapshot a query to CSV, then schedule the same command
hamstik --org acme work export --query 'status = todo' --format csv > todos.csv
hamstik schedule save nightly -- work export --org acme --query 'status = todo' \
  --format csv --output todos.csv

# Import it elsewhere; re-running the same file is idempotent
hamstik --org acme --project HAM work import --file HAM-42.md --json

# Preview the mapped create/edit without touching the server
hamstik work import --file HAM-42.md --dry-run --json
```

## Public API passthrough

`hamstik api request` (and the shorthand `hamstik api /api/v1/...`) calls any
Public API v1 route through the official CLI — in the spirit of `gh api` —
so scripts and agents can reach documented routes even when the installed
CLI has no typed command for them (new server routes stay callable):

```bash
# GET is the default
hamstik api request /api/v1/organizations
hamstik api /api/v1/organizations/acme

# Mutations with a structured body and an automatic idempotency key
# (POST/DELETE; reused across internal retries; --idempotency-key overrides)
hamstik api request --method POST \
  --field name=Acme --field slug=acme /api/v1/organizations

# Body from a file or stdin; If-Match for revision-guarded updates
hamstik api request --method PATCH \
  --body-file patch.json /api/v1/organizations/acme

# Preview the exact request without sending it
hamstik --json --dry-run api request --method POST \
  --field name=Acme /api/v1/organizations
```

Only `/api/v1/...` paths are accepted — private browser routes, non-v1
paths, traversal, embedded credentials, and query strings in the path are
rejected locally before any network access. Header overrides are restricted
to a safe allowlist (`Accept`, `Content-Type`, `If-Match`); `Authorization`
and credential-bearing headers are always rejected. `--json` output is an
envelope: `{ method, path, data, meta? }` where `data` is the server body
verbatim and `meta` carries `requestId`, `etag`, `idempotencyReplayed`,
`location`, and the `RateLimit-*` snapshot when present. The OpenAPI
snapshot (`hamstik api openapi`) is never an allowlist: newer server routes
remain callable.

`hamstik api rate-limit` performs one cheap authenticated read (`GET
/api/v1/me`) and prints the current `RateLimit-*` snapshot, so long-running
scripts and agents can see headroom before hitting a 429:

```bash
hamstik --json --no-input api rate-limit
# {"rateLimit":{"limit":100,"remaining":37,"resetIn":12},"requestId":"..."}
```

The `--json` shape mirrors `api request`'s `meta.rateLimit` fields exactly
(`limit`, `remaining`, `resetIn`) and is `null` when the server sent no
`RateLimit-*` headers. It is a snapshot, not a promise: limits can change
between calls. Unlike typed reads, the probe skips the proactive
wait on a successful-but-depleted response, so headroom is reported
immediately; a hard `429` is still retried under the same bounded retry
policy as any other command (use `--no-retry` to surface it instantly).

## Build from source

Prerequisites:

- Git
- A Rust toolchain — install it with [rustup](https://rustup.rs/); the
  repository pins its toolchain through `rust-toolchain.toml`, which `cargo`
  picks up automatically

No Node.js, Python, or other language runtime is required.

The optional live OpenAPI drift workflow additionally requires a POSIX shell,
`curl`, `jq`, `cmp`, and `install`. The optional `scripts/do-prechecks.py`
helper needs Python 3 with the `rich` package. Neither is part of normal builds
or tests.

```bash
cargo build --workspace
cargo test --workspace
cargo run -p hamstik-cli -- --help
```

## Repository structure

```text
.github/       CI workflows and repository governance
assets/        README, social-preview, and mascot artwork
crates/        Rust workspace (hamstik-cli, hamstik-api-client)
design/        Product requirements, technical specification, versioning policy
openapi/       Frozen Hamstik Public API v1 OpenAPI snapshot
skills/        Hamstik Agent Skill materials
CHANGELOG.md   Release notes (Keep a Changelog format)
```

## API contract

Hamstik CLI uses only the public API under `/api/v1`. The checked-in
[`openapi/hamstik-v1.json`](openapi/hamstik-v1.json) is the frozen development
snapshot of the Hamstik Public API v1 contract used for CLI development and
contract testing — see [`openapi/README.md`](openapi/README.md) for how it is
maintained.

[`openapi/api-parity.json`](openapi/api-parity.json) accounts for every
`operationId` with its API-client method, CLI command, and test classification.
The parsed parity test fails on unclassified operations. Developers can run
`scripts/update-openapi.sh --check` to report live drift or `--update` to
deliberately refresh the byte-for-byte snapshot; normal builds remain offline.

CLI code must not silently depend on private Hamstik web-app or server
internals, and contract changes are synchronized intentionally rather than
casually.

## Development

Quality gates, matching CI exactly:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo build --workspace --release
```

Tests use Rust's built-in test framework. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the full development setup.

### README example verification

The shell examples in this README are verified against the real binary so they
cannot drift from actual flags, enum values, or conventions:

```bash
cargo build --release          # the harness parses with the built binary
python3 scripts/readme_examples.py --verbose
```

The harness parses fenced code blocks structurally, joins backslash
continuations, strips inline comments, and runs every `hamstik` line through
the real argument parser with `--help` (fully offline — no network, no
credentials). Examples embedding `$HAMSTIK_PAT` substitution are treated as
prose (never executed), but their static `hamstik` lines are still validated.
When you add or change an example, run the harness; when an example is
illustrative rather than executable, keep the placeholder obvious (uppercase
UUIDs like `COMMENT_UUID`) so the parse-only verification stays meaningful.
The Rust test `readme_examples` additionally guards the harness itself and the
required example families listed above.

## Design documents

The detailed product and technical direction lives in:

- [design/PRD.md](design/PRD.md) — product requirements
- [design/SPEC.md](design/SPEC.md) — technical specification
- [design/VERSIONING.md](design/VERSIONING.md) — semantic-versioning policy
  (how MAJOR/MINOR/PATCH are chosen)
- [design/RELEASE.md](design/RELEASE.md) — release build system: supported
  target matrix, tag-triggered versioned archives, the version/tag
  consistency gate, and artifact smoke tests
- [design/SIGNING.md](design/SIGNING.md) — release integrity, provenance
  attestations, SBOM, the signing threat model, and verification steps
- [design/INSTALL.md](design/INSTALL.md) — installation channels,
  upgrade/uninstall/rollback procedures, broken-release handling, and
  credential-safe troubleshooting
- [design/AUTOMATION_RULES.md](design/AUTOMATION_RULES.md) — gated design stub
  for CLI-owned workflow automation rules (no command or network behavior ships
  until the Public API exposes a documented event/subscription surface)
- [design/WEBHOOKS.md](design/WEBHOOKS.md) — gated design stub for the
  `hamstik webhook` management family (no command or network behavior ships
  until the Public API exposes documented webhook/subscription operations;
  `hamstik api request` remains the manual escape hatch)

Notable changes are tracked in [CHANGELOG.md](CHANGELOG.md) following the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

These documents are authoritative for the CLI's architecture and command
surface. This README intentionally stays higher level.

## Verifying release downloads

Every release ships per-file sha256 checksums, an aggregate `sha256.sum`,
CycloneDX SBOMs, and Sigstore build-provenance attestations. Before running a
downloaded binary, verify it:

```bash
# Integrity: checksums (run inside the folder of downloaded files)
sha256sum -c sha256.sum

# Authenticity: provenance (requires the GitHub CLI, gh >= 2.57)
gh attestation verify hamstik-cli-x86_64-unknown-linux-gnu.tar.gz \
  --repo blackboardstudios/hamstik-cli

# Identity: the binary reports its exact build
./hamstik-cli-x86_64-unknown-linux-gnu/hamstik version
```

A download is trustworthy when all three hold. `gh attestation verify` fails
on missing, tampered, or foreign attestations; the release pipeline itself
runs the same checks before publishing. Full details, including the threat
model and the deferred platform-signing decision:
[design/SIGNING.md](design/SIGNING.md).

## Contributing

Contributions are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Coding
agents must read [AGENTS.md](AGENTS.md) before making changes.

## Security

Do not report vulnerabilities in public GitHub issues. See
[SECURITY.md](SECURITY.md) for the private reporting process.

## License

Hamstik CLI is open-source software licensed under [Apache-2.0](LICENSE).

The Hamstik service and web application are separate products and are not
licensed under this repository's Apache-2.0 license.

## Trademark

Hamstik and the Hamstik logo are trademarks of Blackboard Studios.
