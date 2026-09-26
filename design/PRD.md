# Hamstik CLI
## Product Requirements Document

**Status:** Draft  
**Product:** Official Hamstik Command-Line Interface  
**Binary:** `hamstik`  
**Primary implementation language:** Rust  
**Repository:** Separate `hamstik-cli` repository  
**Initial API dependency:** Hamstik Public API v1  
**Canonical service:** `https://hamstik.com`

---

# 1. Summary

Hamstik MUST provide a first-class, cross-platform command-line interface named:

```text
hamstik
```

The Hamstik CLI is not merely a collection of HTTP wrappers.

It is a primary Hamstik product interface intended for:

- developers;
- DevOps engineers;
- administrators;
- shell automation;
- CI/CD;
- coding agents;
- future Agent Skills;
- advanced Hamstik users who prefer terminal workflows.

The CLI should eventually provide a rich experience comparable in ambition to mature CLIs such as GitHub CLI and Atlassian CLI.

The initial release will target the Hamstik Programmatic Access v1 API already implemented and hardened.

---

# 2. Product Vision

Hamstik should be usable through several equally legitimate interfaces:

```text
                     Hamstik
                        │
                    /api/v1
                        │
          ┌─────────────┼─────────────┐
          │             │             │
       Web UI       Hamstik CLI    API clients
                        │
                        ▼
                 Agent Skills
                        │
          ┌─────────────┼─────────────┐
          ▼             ▼             ▼
         Pi           Codex      Copilot/Kilo/etc.
```

The CLI should feel like a native system-development tool:

```bash
hamstik auth status
hamstik org list
hamstik project list
hamstik work list
hamstik work view HAM-42
```

Users should not need to understand REST endpoints, pagination cursors, ETags, idempotency keys, or token-refresh implementation details to use Hamstik effectively.

---

# 3. Core Product Principles

## 3.1 Native executable

Installing Hamstik CLI MUST NOT require:

- Node.js;
- Python;
- Java;
- .NET;
- Rust;
- another language runtime.

The distributed CLI is a native executable.

---

## 3.2 Cross-platform

The CLI MUST be first-class on:

```text
Windows
macOS
Linux
```

Initial supported architectures SHOULD include:

```text
Windows x86-64
Windows ARM64

macOS x86-64
macOS ARM64

Linux x86-64
Linux ARM64
```

Linux musl builds SHOULD be provided where release tooling makes this practical.

---

## 3.3 Human-friendly and machine-friendly

Every important workflow must work well for both:

```text
human terminal user
```

and:

```text
coding agent / script
```

Human output should be attractive and concise.

Machine output must be deterministic.

---

## 3.4 Stable machine contract

The CLI's:

```text
--json
```

behavior, exit codes, and non-interactive semantics are product contracts.

Coding agents must not need to scrape formatted terminal tables.

---

## 3.5 Public API only

The official CLI MUST use:

```text
/api/v1
```

It MUST NOT depend on Hamstik's private browser `/api/*` routes.

---

## 3.6 No duplicated domain logic

The CLI must not attempt to recreate Hamstik business rules.

The server remains authoritative for:

- authorization;
- workflow;
- validation;
- tenant/Organization isolation;
- Work Item revision;
- idempotency;
- allowed transitions.

The CLI provides ergonomics around those rules.

---

# 4. Primary Users

## 4.1 Interactive developer

Example:

```bash
hamstik work list --assignee me --scope open
hamstik work view HAM-112
hamstik work start HAM-112
```

---

## 4.2 Coding agent

Example:

```bash
hamstik work view HAM-112 --json
hamstik work transition HAM-112 in_progress --json
hamstik work comment add HAM-112 --body-file /tmp/result.md --json
```

The agent should receive structured results and structured failures.

---

## 4.3 Automation script

Example:

```bash
hamstik work list --status done --updated-after 2026-09-01T00:00:00Z --json
```

---

## 4.4 CI/CD system

Initially CI may authenticate using:

```text
HAMSTIK_TOKEN
```

Future releases may use integration/workload identities.

---

## 4.5 Hamstik administrator/developer

The CLI should eventually expose broader Hamstik functionality as the Public API grows.

The command architecture must accommodate this without major restructuring.

---

# 5. Initial Product Scope

The first useful CLI release must support the entire Public API v1 dogfooding workflow.

Required command families:

```text
hamstik auth
hamstik context
hamstik org
hamstik project
hamstik report
hamstik dashboard
hamstik work
hamstik completion
hamstik doctor
```

---

# 6. Authentication

## 6.1 Initial authentication

The first CLI release uses Hamstik Personal Access Tokens.

Normal interactive flow:

```bash
hamstik auth login
```

The CLI:

1. asks for the Hamstik host if necessary;
2. securely prompts for a PAT;
3. validates the PAT against `/api/v1/me`;
4. determines the Hamstik user identity;
5. stores the credential using the operating system's secure credential store;
6. creates/updates the local non-secret profile.

The PAT MUST NOT be accepted as a normal command-line argument that would appear in shell history.

---

## 6.2 Token through stdin

For scripted setup:

```bash
printf '%s' "$TOKEN" | hamstik auth login --with-token
```

The token is read from stdin.

---

## 6.3 Environment authentication

Automation can provide:

```text
HAMSTIK_TOKEN
```

An environment token:

- is not persisted;
- overrides stored credentials for the invocation;
- is appropriate for CI and ephemeral agent environments.

---

## 6.4 Future OAuth

The architecture MUST support later replacement of normal interactive PAT login with:

```text
OAuth Authorization Code + PKCE
```

and a device-oriented flow for SSH/headless use.

The CLI command surface SHOULD remain:

```bash
hamstik auth login
```

when that happens.

Users should not need a new CLI workflow merely because authentication technology improves.

---

# 7. Credential Security

Normal credential storage:

```text
Windows → Windows Credential Manager
macOS   → Keychain Services
Linux   → Secret Service
```

Secrets MUST NOT be stored in the normal CLI configuration file.

If a secure credential service is unavailable, the CLI MUST NOT silently fall back to plaintext storage.

Instead it should explain alternatives such as:

```text
HAMSTIK_TOKEN
```

for headless automation.

An explicitly requested future plaintext/file credential store may be considered separately, but is not the secure default.

---

# 8. Multiple Accounts and Hosts

The CLI MUST support multiple profiles.

Examples:

```text
hamstik.com / personal account
hamstik.com / work account
local development Hamstik
future private Hamstik host
```

Commands:

```bash
hamstik auth list
hamstik auth switch
hamstik auth status
hamstik auth logout
```

Default production host:

```text
https://hamstik.com
```

The architecture MUST NOT assume there can only ever be one Hamstik host.

---

# 9. Context

Authentication answers:

> Who am I?

Context answers:

> Where am I working?

A CLI context may contain:

```text
host
Organization
Project
```

Example:

```bash
hamstik context show
```

could display:

```text
Host:          https://hamstik.com
User:          steven@example.com
Organization:  blackboard-studios
Project:       HAM
```

---

# 10. Repository-Local Context

Hamstik SHOULD support a non-secret project context file:

```text
.hamstik.toml
```

Example:

```toml
version = 1
host = "https://hamstik.com"
organization = "blackboard-studios"
project = "HAM"
```

This file is safe to commit to a source repository.

It MUST NOT contain:

- PATs;
- OAuth tokens;
- passwords;
- other secrets;
- personal account identity.

This allows:

```bash
cd hamstik/
hamstik work list
```

to automatically know which Hamstik Project is relevant.

This is especially valuable for coding agents.

---

# 11. Context Commands

Initial commands SHOULD include:

```bash
hamstik context show
hamstik context show --explain

hamstik context set --org blackboard-studios
hamstik context set --org blackboard-studios --project HAM

hamstik context clear

hamstik context init
```

Convenience commands SHOULD also exist:

```bash
hamstik org use blackboard-studios
hamstik project use HAM
```

---

# 12. Context Resolution Priority

Explicit invocation intent always wins.

Conceptually:

```text
CLI flags
    ↓
environment variables
    ↓
repository .hamstik.toml
    ↓
active profile defaults
```

Users must be able to inspect where each context value came from.

---

# 13. Organization Commands

Initial commands:

```bash
hamstik org list
hamstik org view <slug>
hamstik org use <slug>
```

`organization` MAY be accepted as a long alias, but `org` is the preferred ergonomic command noun.

---

# 14. Project Commands

Initial commands:

```bash
hamstik project list
hamstik project view <key>
hamstik project use <key>
```

Project commands should use the active Organization context where not explicitly provided.

Example:

```bash
hamstik project list --org blackboard-studios
```

---

# 15. Work Item Command Namespace

The CLI command noun is:

```text
work
```

not:

```text
work-item
```

The product still calls the domain object a **Work Item**.

`work` is simply the ergonomic CLI command.

---

# 14A. Advanced Reporting Commands

The CLI exposes the Public API v1 Advanced Reporting application through two
Organization-scoped command families:

```bash
hamstik report list
hamstik report view <id>
hamstik report create --file report.json
hamstik report edit <id> --file report.json
hamstik report delete <id>
hamstik report run <id>
hamstik report selection-items <run-id> <cell-id>

hamstik dashboard list
hamstik dashboard view <id>
hamstik dashboard run <id> [--filters-file filters.json]
```

These commands are capability-gated server features. The CLI MUST surface the
server's plan, application-enablement, authorization, and PAT-scope errors; it
must not try to predict or reproduce those rules locally.

Advanced report definitions and dashboard-run filters are structured documents,
not a growing collection of CLI flags. Creation and replacement therefore read
the documented Public API request shape from JSON files (or `-` for stdin).
The typed client validates the outer document shape while the server remains
authoritative for reporting semantics and business validation.

Report edits and deletes use the current resource ETag by default and MUST NOT
silently overwrite revision conflicts. `--force` is the explicit optimistic-
concurrency bypass. Report and dashboard runs read the current revision before
evaluation, so users do not need to manage `expectedRevision` manually.

List and selection-item commands use the CLI's standard cursor behavior and
machine output. Human rendering presents server-returned data only; the CLI
does not recompute datasets, selections, dashboard widgets, or aggregations.

---

# 16. Work Item Read Commands

Initial commands:

```bash
hamstik work list
hamstik work view HAM-42
hamstik work transitions HAM-42
```

---

# 17. Work Item List Filtering

The CLI should expose the Public API filters in an ergonomic form.

Examples:

```bash
hamstik work list --status in_progress
hamstik work list --status todo --status in_progress

hamstik work list --scope open

hamstik work list --type bug
hamstik work list --priority urgent

hamstik work list --assignee me
hamstik work list --assignee none

hamstik work list --sprint none

hamstik work list --label-name api

hamstik work list --top-level

hamstik work list --search "OAuth"

hamstik work list --updated-after 2026-09-01T00:00:00Z
```

Convenience options MAY map to standard filters, for example:

```bash
hamstik work list --mine
```

as an alias for:

```text
--assignee me
```

---

# 18. Work Item Creation

Example:

```bash
hamstik work create \
  --title "Implement OAuth login" \
  --type feature \
  --priority high
```

Optional fields should map to the Public API's supported creation fields.

The CLI should support description input through:

```text
--description
--description-file
```

For long text, file/stdin input should be preferred.

---

# 19. Work Item Editing

Example:

```bash
hamstik work edit HAM-42 \
  --priority urgent \
  --story-points 8
```

The CLI MUST automatically handle normal Work Item ETag retrieval.

A normal edit should:

1. retrieve the current Work Item;
2. retain its ETag/revision;
3. send PATCH with the exact `If-Match`;
4. surface a clear conflict if another client changed the item.

The CLI MUST NOT silently overwrite revision conflicts.

---

# 20. Forced Editing

An explicit force option MAY map to:

```text
If-Match: *
```

Example:

```bash
hamstik work edit HAM-42 --priority high --force
```

This should be clearly documented as overriding optimistic concurrency.

Coding agents should not use `--force` by default.

---

# 21. Status Transitions

Generic:

```bash
hamstik work transition HAM-42 in_review
```

Convenience:

```bash
hamstik work start HAM-42
hamstik work close HAM-42
```

Mappings:

```text
start → in_progress
close → done
```

The CLI SHOULD query the server's transition discovery endpoint rather than permanently hard-code workflow assumptions.

Future workflow changes must not require redesigning these commands.

---

# 22. Comments

Initial command structure:

```bash
hamstik work comment list HAM-42

hamstik work comment add HAM-42 \
  --body "Implementation complete."
```

Long comments:

```bash
hamstik work comment add HAM-42 \
  --body-file result.md
```

Stdin SHOULD be supported:

```bash
cat result.md | hamstik work comment add HAM-42 --body-file -
```

This is especially important for coding agents.

---

# 23. Human Output

Default output should optimize for terminal reading.

Examples:

```text
KEY      STATUS       PRIORITY  ASSIGNEE  TITLE
HAM-42   In Progress  High      Steven    Implement Public API
HAM-43   Todo         Medium    —         Build Hamstik CLI
```

Detail output should be readable without requiring JSON knowledge.

The CLI SHOULD:

- detect terminal width;
- avoid excessive horizontal overflow;
- use color only when appropriate;
- respect `NO_COLOR`;
- avoid decorative output when piped.

---

# 24. Machine Output

All data-producing commands MUST support:

```text
--json
```

Example:

```bash
hamstik work view HAM-42 --json
```

Machine output requirements:

- stdout contains only requested JSON data;
- diagnostics go to stderr;
- no spinner;
- no ANSI color;
- no prompts;
- no decorative banners;
- stable field names;
- valid UTF-8 JSON.

---

# 25. JSON Error Behavior

When `--json` is active and a command fails, stderr SHOULD contain a structured object such as:

```json
{
  "error": {
    "code": "REVISION_CONFLICT",
    "message": "The Work Item changed since it was read.",
    "requestId": "..."
  }
}
```

stdout should remain empty on failure.

Coding agents must not need to parse English terminal prose to understand an API failure.

---

# 26. Pagination

Collection commands SHOULD expose:

```text
--limit
--cursor           (--since-cursor is an accepted alias)
--all
```

Default behavior should request a bounded first page.

`--limit` bounds the *result* (total items emitted), not the transport page
size. When the cap spans more than one page it is satisfied by following
cursors; it never causes an unbounded read.

`--all` automatically follows cursors until complete.

`--cursor` / `--since-cursor` names the opaque cursor a result starts after and
MUST be honored for the first request of a traversal, including a traversal
continued with `--all`. A pipeline that crashed after consuming a page resumes
from the cursor it recorded with no re-reads and no gaps, within the
consistency guarantees of the server's own ordering.

The CLI MUST treat cursors as opaque.

The CLI MUST NOT construct Public API cursors itself. When a cap or a resume
point lands partway through a server page, the CLI reports that no resume cursor
is available rather than inventing one.

---

# 27. Idempotency

Users should not normally have to know about Public API idempotency keys.

For retry-sensitive operations, the CLI MUST generate a strong unique key automatically.

Examples:

```text
Work Item creation
Work Item transition
Comment creation
```

If the CLI retries the same logical operation internally, it MUST reuse the same idempotency key.

Advanced callers MAY be given an explicit `--idempotency-key` option for deterministic automation.

---

# 28. Retry Behavior

The CLI should safely retry only operations where doing so is valid.

Safe automatic retry candidates include:

- GET requests;
- mutations protected by an idempotency key, reusing that same key;
- read-only POST requests such as SqueakQL search and validation;
- HTTP 429 when `Retry-After` is present;
- selected transient server/network failures.

A revision-sensitive PATCH that does not use the Public API idempotency
mechanism must not be retried after an ambiguous network failure. PATCH
operations that require an idempotency key may be retried only with that same
key and request body.

---

# 29. Exit Codes

The CLI MUST provide stable exit-code categories suitable for scripting.

At minimum distinguish:

- success;
- command/usage error;
- authentication failure;
- authorization failure;
- not found;
- concurrency/conflict;
- rate limit;
- network/API server failure;
- local configuration failure.

The exact numeric assignments are defined in the technical SPEC.

---

# 30. Non-Interactive Behavior

The CLI MUST never hang an agent or CI job waiting for an interactive prompt.

When:

```text
--json
```

or:

```text
--no-input
```

is active, required missing values cause a deterministic error.

Non-TTY stdin SHOULD also disable prompts unless explicitly requested.

---

# 31. Diagnostics

Global diagnostic options SHOULD include:

```text
--verbose
--quiet
--no-color
--no-input
```

Verbose diagnostics MUST NOT expose credentials.

---

# 32. Doctor Command

The initial CLI SHOULD include:

```bash
hamstik doctor
```

It should diagnose:

- config location;
- selected profile;
- host;
- credential-store availability;
- authentication;
- `/api/v1` reachability;
- API compatibility;
- Organization/Project context validity;
- proxy/network issues.

Also:

```bash
hamstik doctor --json
```

for automated troubleshooting.

---

# 33. Shell Completion

The CLI MUST support completion generation for:

```text
Bash
Zsh
Fish
PowerShell
```

Example:

```bash
hamstik completion bash
```

Install packages SHOULD configure completion automatically where practical.

---

# 34. Agent Skill

The `hamstik-cli` repository SHOULD own the canonical Hamstik Agent Skill because the Skill must remain synchronized with CLI behavior.

Conceptual structure:

```text
skills/
└── hamstik/
    ├── SKILL.md
    └── references/
```

The canonical entry point is always `SKILL.md`. Supporting files under
`skills/hamstik/references/` cover specialized workflows; installation must
keep them with the entry point.

The Skill teaches:

- when to use Hamstik;
- how to discover context;
- how to read a Work Item before implementation;
- how to transition work safely;
- when to comment;
- how to use `--json`;
- how to react to concurrency failures.

It should NOT duplicate the REST API specification.

---

# 35. Skill Installation

The CLI supports:

```bash
hamstik agent skill install
hamstik agent skill install --global
```

The default target is the current project's `.agents/skills/hamstik/` directory;
`--global` targets the user-level `.agents/skills/hamstik/` directory.

The canonical skill directory remains one source.

Do not maintain four independently edited copies for Pi, Codex, Kilo, and Copilot.

---

# 36. Documentation Ownership

Canonical user-facing documentation for the Hamstik CLI MUST remain in the **main Hamstik web application repository**, alongside all other Hamstik user documentation.

The CLI repository may contain:

- developer README;
- contributing/build instructions;
- generated command metadata;
- Agent Skill;
- implementation design;
- packaging documentation.

It MUST NOT become a separate canonical user-documentation site.

---

# 37. Documentation Synchronization

A CLI change that alters user-visible behavior is not release-complete until the main Hamstik documentation has been updated.

The CLI release process SHOULD generate a machine-readable or Markdown command reference from the actual command tree.

Release automation SHOULD be capable of producing or opening the corresponding documentation update for the main Hamstik repository.

Narrative guides remain hand-authored in the main application repository.

---

# 38. Installation Experience

Users should be able to install Hamstik naturally for their platform.

## Windows

Preferred:

```powershell
winget install Hamstik.CLI
```

Also:

- PowerShell installer;
- direct signed executable/archive.

---

## macOS

Preferred:

```bash
brew install hamstik/tap/hamstik
```

Eventually:

```bash
brew install hamstik
```

if appropriate.

Direct signed/notarized archives should also be provided.

---

## Linux

Provide:

- shell installer;
- `.deb`;
- `.rpm`;
- native tarballs;
- x86-64 and ARM64 builds.

APT/RPM repositories may be introduced later as usage warrants.

---

## npm

An optional distribution channel SHOULD be considered:

```bash
npm install -g @hamstik/cli
```

The npm package MUST install/use the native Hamstik executable.

Node.js MUST NOT become Hamstik CLI's runtime.

---

# 39. Release Integrity

Official releases SHOULD provide:

- checksums;
- reproducible/traceable CI artifacts;
- artifact provenance/attestations where available;
- signed Windows binaries;
- signed/notarized macOS binaries;
- release notes.

The release process should be automated.

---

# 40. Versioning

Hamstik CLI uses semantic versioning independently from Hamstik server releases.

During initial dogfooding:

```text
0.x
```

is appropriate.

`1.0.0` should represent the point at which:

- core command contracts are stable;
- JSON output is considered stable;
- installation channels are established;
- Agent Skill is supported;
- API compatibility is proven through dogfooding.

---

# 41. Compatibility

A newer Hamstik server should generally remain compatible with an older CLI within Public API v1.

The CLI should fail clearly when talking to an incompatible server.

The CLI MUST NOT require its own version number to exactly match the Hamstik web application's version.

---

# 42. Privacy and Telemetry

Initial Hamstik CLI MUST NOT send product telemetry merely by being installed or used.

Normal API calls obviously reach the configured Hamstik server.

Any future telemetry feature would require an explicit product decision and privacy documentation.

In-place self-update (`hamstik self update`) remains gated on the
packaging/release milestone and MUST stay explicit, opt-in/opt-out,
telemetry-free, and free of silent network calls; the design lives in
[`design/SELF_UPDATE.md`](SELF_UPDATE.md).

---

# 43. Initial Non-Goals

The initial CLI does not need:

- OAuth login yet;
- MCP;
- public Sprint edit/delete (creation and transitions have since shipped);
- public Board CRUD;
- public Label edit/delete (listing and creation have since shipped);
- billing management;
- Organization administration;
- App administration;
- Gitea management;
- Audit Compliance management;
- Advanced Dashboard creation or editing (the Public API currently exposes
  dashboard reads and runs only);
- workflow automation rules and any event/subscription execution (gated on a
  documented Public API event surface; see
  [`design/AUTOMATION_RULES.md`](AUTOMATION_RULES.md));
- webhook/subscription management (gated on a documented Public API webhook
  surface; see [`design/WEBHOOKS.md`](WEBHOOKS.md));
- in-place self-update (`hamstik self update`, `--channel stable|prerelease`),
  gated on the packaging/release milestone and required to remain opt-in,
  opt-out, and telemetry-free with no silent network calls (see
  [`design/SELF_UPDATE.md`](SELF_UPDATE.md));
- offline editing;
- local data synchronization.

Work Item soft-deletion (`work delete`) and the partial Sprint/Label surfaces
above were added after this list was written, following the Public API. The
remaining items can be added as the Public API expands.

---

# 44. Growth Model

The command hierarchy must be capable of eventually supporting:

```text
hamstik sprint
hamstik board
hamstik label
hamstik user
hamstik app
hamstik audit
hamstik repo
hamstik release
hamstik rule
hamstik webhook
hamstik admin
```

without redesigning root-level conventions.

`hamstik rule` is a gated future namespace: it is only added once the Public
API exposes a documented event/subscription surface. See
[`design/AUTOMATION_RULES.md`](AUTOMATION_RULES.md).

`hamstik webhook` is likewise gated: it is only added once the Public API
exposes documented webhook/subscription operations. See
[`design/WEBHOOKS.md`](WEBHOOKS.md).

---

# 45. Dogfooding Success Criteria

The first CLI milestone is complete when a developer or coding agent can:

1. install the CLI without installing a language runtime;
2. authenticate using a PAT;
3. verify auth status;
4. establish Organization/Project context;
5. list Work Items;
6. filter Work Items;
7. view complete Work Item details;
8. read Work Item comments;
9. create a Work Item;
10. edit a Work Item safely using optimistic concurrency;
11. discover valid transitions;
12. start a Work Item;
13. close a Work Item;
14. create a comment;
15. receive deterministic JSON;
16. receive deterministic errors;
17. operate non-interactively;
18. use the same behavior on Windows, macOS, and Linux.

---

# 46. Agent Dogfooding Success Criteria

A coding agent with the Hamstik Agent Skill must be able to receive:

> Implement HAM-42.

and independently perform:

```text
resolve Hamstik context
↓
read HAM-42
↓
read relevant comments
↓
mark HAM-42 in progress
↓
perform repository work
↓
run project validation
↓
post a useful completion comment
↓
transition HAM-42 appropriately
```

without manually handling:

- HTTP;
- PAT headers;
- cursors;
- ETags;
- idempotency keys.

---

# 47. Product Milestone

The first major CLI milestone is:

# Hamstik CLI Dogfooding Release

Definition:

> Hamstik development can be managed through the official native `hamstik` CLI, directly by humans or through portable Agent Skills, without requiring use of the Hamstik web UI for routine Work Item lifecycle operations.
