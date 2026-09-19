# Hamstik CLI
## Technical Specification

**Status:** Implemented (authoritative — see `AGENTS.md`)\
**Related:** `design/PRD.md`  
**Implementation:** Rust  
**Binary:** `hamstik`  
**Initial server contract:** Hamstik Public API v1

---

# 1. Repository Structure

Recommended repository:

```text
hamstik-cli/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
│
├── crates/
│   ├── hamstik-cli/
│   │   └── src/
│   └── hamstik-api-client/
│       └── src/
│
├── design/
│   ├── PRD.md
│   └── SPEC.md
│
├── openapi/
│   └── hamstik-v1.json
│
├── skills/
│   └── hamstik/
│       └── SKILL.md
│
├── scripts/
└── README.md
```

Distribution packaging (`packaging/`) is introduced by a later milestone, not by
the Dogfooding Alpha.

Avoid premature micro-crates.

Two Rust crates are sufficient initially:

```text
hamstik-cli
hamstik-api-client
```

---

# 2. Crate Responsibilities

## hamstik-api-client

Owns:

- Public API HTTP transport;
- API request/response types;
- bearer injection;
- API error parsing;
- request IDs;
- cursor handling as opaque values;
- ETag extraction;
- `Retry-After`;
- retry-capable request execution.

It MUST NOT know about:

- terminal tables;
- interactive prompts;
- local context files;
- OS keyrings;
- shell completion.

---

## hamstik-cli

Owns:

- command tree;
- config;
- profiles;
- credential-store access;
- context resolution;
- human formatting;
- JSON formatting;
- prompts;
- environment variables;
- exit codes;
- shell completion;
- Agent Skill installation.

---

# 3. Rust Ecosystem

Preferred components:

```text
clap
tokio
reqwest
serde
serde_json
toml
keyring
directories
uuid
thiserror
anyhow
inquire
clap_complete
tracing
```

Use dependencies only where they materially reduce implementation complexity.

Do not introduce multiple libraries for the same job.

---

# 4. Command Parser

Use:

```text
clap
```

with derive-based command structures.

The command tree should be declarative enough to generate:

- help;
- shell completion;
- command reference metadata;
- user documentation artifacts.

---

# 5. Async Runtime

Use:

```text
tokio
```

The API client should be asynchronous.

Do not create a blocking HTTP implementation around an otherwise async CLI.

The CLI executes one command per process and never requires task concurrency,
so the entrypoint uses a `current_thread` runtime; keyring operations that
cannot run inside a runtime hop to a short-lived worker thread.

---

# 6. HTTP Client

Use:

```text
reqwest
```

Requirements:

- HTTPS by default;
- system proxy support;
- `HTTP_PROXY`;
- `HTTPS_PROXY`;
- `NO_PROXY`;
- connection timeout;
- request timeout;
- custom User-Agent;
- custom CA bundle support where practical;
- redirects are NOT followed: the client is bound to one validated origin,
  and following a redirect would replay the `Authorization` handshake to
  another host while presenting the response as if it came from the original.

User-Agent:

```text
hamstik-cli/<version> (<os>; <arch>)
```

---

# 7. TLS

Production hosts MUST use HTTPS.

Exception: plain HTTP is allowed only for *true loopback hosts*, decided on
the parsed host — never on a string prefix, so lookalikes such as
`127.0.0.1.evil.com` or `localhost.evil.com` are never treated as loopback:

```text
http://localhost          (case-insensitive exact name)
http://127.0.0.0/8        (any address in the block)
http://[::1]              (and IPv4-mapped IPv6 loopback, e.g. ::ffff:127.0.0.1)
```

may be allowed for local development.

Plain HTTP to a non-loopback host MUST be rejected unless an explicit advanced insecure override exists.

Do not disable certificate verification by default.

Support for enterprise/custom CAs SHOULD be designed through:

```text
--ca-bundle
HAMSTIK_CA_BUNDLE
```

rather than telling enterprise users to disable TLS verification.

---

# 8. Default Host

Default:

```text
https://hamstik.com
```

Configuration and profiles may override it.

Normalize hosts so:

```text
https://hamstik.com/
```

and:

```text
https://hamstik.com
```

represent the same host.

Do not permit embedded credentials in configured host URLs.

---

# 9. API Prefix

The API client targets:

```text
/api/v1
```

No CLI code may call private browser `/api/*` routes.

---

# 10. OpenAPI Snapshot

Retain a versioned snapshot:

```text
openapi/hamstik-v1.json
```

This is used for:

- development reference;
- contract testing;
- detecting unexpected client/server drift.

The CLI build MUST NOT require network access to retrieve OpenAPI.

---

# 11. API Client Strategy

Initial implementation SHOULD use a deliberate hand-written typed client rather than generating the entire CLI directly from OpenAPI.

Reasons:

- the API surface is currently small;
- CLI semantics differ from raw HTTP;
- ETag/idempotency behavior needs deliberate handling;
- generated clients often leak transport structure into UX.

OpenAPI remains authoritative for server contract verification.

---

# 12. API Client Operations

Initial typed operations:

```text
getMe

listOrganizations
getOrganization

listProjects
getProject

listWorkItems
getWorkItem
createWorkItem
updateWorkItem

listTransitions
transitionWorkItem

listComments
createComment
```

---

# 13. API Error Type

Define one canonical client error structure.

Conceptually:

```rust
struct ApiError {
    status: u16,
    code: String,
    message: String,
    request_id: Option<String>,
    field_errors: BTreeMap<String, Vec<String>>,
    retry_after: Option<Duration>,
}
```

Preserve the server's stable error code.

Do not convert:

```text
REVISION_CONFLICT
```

into generic:

```text
request failed
```

Server-supplied text (`message`, `code`, `requestId`, field-error keys and
values) is attacker-influenced and MUST have control characters stripped
before it is rendered, so terminal escape sequences cannot survive into CLI
output. Empty values fall back to the status-derived defaults.

---

# 14. API Metadata

Successful client operations SHOULD retain relevant transport metadata separately from the resource.

At minimum:

```text
request ID
ETag
Idempotency-Replayed
Retry-After where applicable
```

Conceptually:

```rust
struct ApiResponse<T> {
    value: T,
    request_id: Option<String>,
    etag: Option<String>,
    idempotency_replayed: bool,
}
```

---

# 15. Retry Policy

## GET

May automatically retry:

- connection failure before useful response;
- selected transient 502/503/504;
- 429 honoring `Retry-After`.

---

## Idempotency-key-protected mutations

For every operation where the checked-in Public API contract requires an
`Idempotency-Key` (including POST, PATCH, and DELETE operations), the CLI
generates a key once per logical operation.

Internal retry MUST reuse that exact key and request body.

May retry the same transient classes as GET.

Read-only POST operations such as SqueakQL search and validation do not receive
an idempotency key; they may be retried as reads.

---

## PATCH

An ambiguous PATCH may be retried only when that operation requires an
idempotency key, reusing the same key and request body. A revision-sensitive
PATCH without an idempotency key (currently Work Item update) MUST surface the
failure instead of replaying it.

---

# 16. Retry Limits

Default retry attempts SHOULD be bounded.

Suggested:

```text
3 attempts total
```

Use bounded exponential backoff with jitter where no server `Retry-After` exists.

`Retry-After` takes precedence.

Provide:

```text
--no-retry
```

for troubleshooting/advanced automation if useful.

---

# 17. Idempotency Key Generation

Use cryptographically strong UUID-style identifiers.

A single logical operation gets one key.

Example internal behavior:

```text
generate K
↓
POST create
↓
network failure
↓
retry POST with same K
```

Do NOT generate a new key for every HTTP attempt.

---

# 18. Explicit Idempotency Key

Mutation commands SHOULD expose an advanced:

```text
--idempotency-key
```

option.

This is useful for external orchestrators that need deterministic replay behavior.

Validate syntax client-side according to the API contract.

---

# 19. Work Item ETag Handling

`getWorkItem` MUST preserve the ETag.

Normal:

```bash
hamstik work edit HAM-42 --priority high
```

performs:

```text
GET Work Item
→ ETag "wi-4"

PATCH
If-Match: "wi-4"
```

A `412 REVISION_CONFLICT` becomes a concurrency error.

Do not automatically refetch and overwrite.

---

# 20. Force Behavior

```text
--force
```

maps to:

```text
If-Match: *
```

Only Work Item operations that explicitly support it should expose the option.

The help text MUST clearly state that it bypasses revision conflict protection.

---

# 21. Transition Behavior

Before a transition, the CLI SHOULD retrieve current Work Item state and/or transition discovery.

Commands:

```bash
hamstik work transitions HAM-42
hamstik work transition HAM-42 in_review
```

Convenience:

```bash
hamstik work start HAM-42
hamstik work close HAM-42
```

Targets:

```text
start → in_progress
close → done
```

If already in the target state, convenience commands SHOULD behave as successful no-ops and return/display current state rather than creating meaningless transitions.

---

# 22. Configuration Locations

Use platform conventions.

Conceptually:

## Linux

```text
$XDG_CONFIG_HOME/hamstik/config.toml
```

fallback:

```text
~/.config/hamstik/config.toml
```

## macOS

Use the standard per-user application support/config location selected by the configuration-path library.

## Windows

Use the standard roaming/local application configuration location selected by the configuration-path library.

Provide:

```bash
hamstik config path
```

or expose the location through:

```bash
hamstik doctor
```

so users never need to guess.

---

# 23. Global Config

Conceptual:

```toml
version = 1
active_profile = "hamstik.com-steven"

[profiles.hamstik.com-steven]
host = "https://hamstik.com"
user_id = "..."
email = "steven@example.com"
default_organization = "blackboard-studios"
default_project = "HAM"
```

No secret values are permitted.

A missing config file is a normal state and behaves like an empty document. A
file that exists but cannot be understood — malformed TOML, an unknown field, an
unsupported `version`, or an unreadable path — must fail loudly with the
effective configuration exit code, naming the file in the message; silently
starting over would discard profiles and context defaults. Rejecting an unknown
`version` is what makes drift from a newer CLI visible instead of corrosive.
`hamstik doctor` reports the same failure as a failing check rather than aborting,
so diagnostics that do not depend on config (notably the credential store) remain
usable.

---

# 24. Credential Store Keys

Use a stable service identifier such as:

```text
Hamstik CLI
```

or a reverse-domain equivalent.

Key credentials using enough identity to avoid collisions:

```text
host
+
Hamstik user ID
```

Do not key only by email.

---

# 25. Keyring

Use a maintained cross-platform credential-store abstraction.

Normal mappings:

```text
Windows → Credential Manager
macOS   → Keychain
Linux   → Secret Service
```

Keyring errors must be clearly distinguished from Hamstik authentication errors.

---

# 26. Headless Linux

A machine without an available Secret Service must not silently write a plaintext token.

Example failure guidance:

```text
No secure credential store is available.

For headless automation, set HAMSTIK_TOKEN for the process
or configure an OS credential service.
```

---

# 27. Authentication Precedence

Credential source:

```text
HAMSTIK_TOKEN
    ↓
selected profile credential store
```

An environment token is ephemeral and MUST NOT be written back to the keyring.

---

# 28. Profile Selection

Profile precedence:

```text
--profile
↓
HAMSTIK_PROFILE
↓
active profile
```

Host-specific context MAY select the matching profile when unambiguous.

Ambiguity should produce a useful error rather than silently choosing an account.

---

# 29. Auth Commands

Required:

```bash
hamstik auth login
hamstik auth login --with-token

hamstik auth status
hamstik auth list
hamstik auth switch <profile>
hamstik auth logout
hamstik auth forget [profile]
```

`auth logout` removes the local credential.

`auth forget` additionally removes the profile entry from the global config, and
is the only auth command that edits config state beyond `login`/`switch`. It
targets the named profile, or the selected profile when no name is given. Like
`auth logout` it never revokes the PAT server-side. Forgetting a profile that was
active must not silently move the user to a different host: `active_profile` is
reused only when exactly one profile remains, otherwise it is cleared.

It does not necessarily revoke the PAT server-side unless a future public PAT-management API supports that action.

Help text must distinguish:

```text
local logout
```

from:

```text
server-side PAT revocation
```

---

# 30. Login Validation

A token is not stored until:

```text
GET /api/v1/me
```

succeeds.

Store returned:

```text
user ID
email
host
default Organization
```

as non-secret profile metadata.

---

# 31. Environment Variables

Initial supported environment variables:

```text
HAMSTIK_TOKEN
HAMSTIK_HOST
HAMSTIK_PROFILE
HAMSTIK_ORG
HAMSTIK_PROJECT
HAMSTIK_CA_BUNDLE
NO_COLOR
```

Potential:

```text
HAMSTIK_NO_INPUT
```

if useful.

Do not create dozens of environment knobs initially.

---

# 32. Context File

Filename:

```text
.hamstik.toml
```

Versioned schema:

```toml
version = 1
host = "https://hamstik.com"
organization = "blackboard-studios"
project = "HAM"
```

All fields other than `version` MAY be optional.

---

# 33. Context Discovery

Search upward from the current working directory for the nearest:

```text
.hamstik.toml
```

The nearest file wins.

`context show --explain` MUST display:

```text
value
source
```

Example:

```text
Host:         https://hamstik.com   [.hamstik.toml]
Organization: blackboard-studios    [.hamstik.toml]
Project:      HAM                   [.hamstik.toml]
Profile:      hamstik.com-steven    [global config]
Credential:   keyring               [profile]
```

Never show the credential itself.

---

# 34. Context Precedence

For Organization/Project:

```text
explicit command flag
↓
environment variable
↓
nearest .hamstik.toml
↓
profile default
```

For host:

```text
--host
↓
HAMSTIK_HOST
↓
.hamstik.toml
↓
profile host
↓
https://hamstik.com
```

---

# 35. Context Validation

`context set` and `org/project use` SHOULD validate the selected resource through the Public API before persisting it.

Project validation must occur inside the resolved Organization.

Under an ephemeral `HAMSTIK_TOKEN` with no stored credential and no profile,
`org use` and `project use` create a profile on the fly from `GET /api/v1/me`
(the same validation and metadata flow as login) and make it active before
writing the default; otherwise the written default could never be selected
again (§28). The token itself is never persisted.

---

# 36. Command Grammar

Root:

```text
hamstik [GLOBAL OPTIONS] <COMMAND>
```

Required root commands:

```text
auth
context
org
project
work
doctor
completion
version
```

Potential later:

```text
api
agent
config
```

---

# 37. Global Options

At minimum:

```text
--host
--profile
--org
--project

--json
--quiet
--verbose
--no-color
--no-input
```

Do not let every subcommand reinvent these.

---

# 38. Output Mode

Default:

```text
human
```

`--json`:

```text
machine
```

Do not infer JSON merely because stdout is redirected.

A script should explicitly request the stable machine contract.

Piped human output should simply disable interactive decoration.

---

# 39. stdout / stderr Contract

## Success

stdout:

```text
requested output
```

stderr:

```text
progress/warnings/diagnostics only
```

## Failure

stdout:

```text
empty
```

stderr:

```text
human error
```

or JSON error when `--json`.

---

# 40. Color

Color mode:

```text
auto
always
never
```

Initial public option MAY expose only:

```text
--no-color
```

while honoring `NO_COLOR`.

No ANSI codes in `--json`.

---

# 41. Interactive Prompts

Use a maintained terminal-prompt library.

Interactive prompts are allowed only when:

- stdin/stdout environment is interactive;
- `--no-input` is not set;
- `--json` is not set.

Otherwise missing required input produces a usage/configuration error.

---

# 42. Work List Command

Conceptual signature:

```text
hamstik work list

--search
--status <STATUS>...
--scope <all|open|closed>
--type <TYPE>...
--priority <PRIORITY>...
--assignee <me|none|UUID>
--sprint <none|UUID>
--label <UUID>...
--label-name <NAME>...
--parent <KEY>
--top-level
--updated-after <RFC3339>

--limit <N>
--cursor <CURSOR>        (alias: --since-cursor)
--all
```

`--limit` is the total result cap, distinct from the server page size: the page
size sent on the wire is `min(limit, endpoint maximum)` — 200 items
(`PaginationArgs::MAX_PAGE_SIZE`) by default, 100 (`PaginationArgs::DIRECTORY_PAGE_SIZE`)
on the organization, project, sprint, label, and link endpoints the contract caps
lower — and caps above one page are met by following cursors (with `--all`)
rather than by asking the server for an oversized page.

`--all` follows `nextCursor` until the server reports no more pages, subject
to client-side budgets (1,000 pages / 50,000 items). A hostile or looping
server therefore fails the command with a protocol error instead of spinning
forever; the error tells the user to narrow the query.

Resume and ordering guarantees:

- `--cursor` / `--since-cursor` is forwarded verbatim as the first request of
  the traversal, so `--since-cursor <checkpoint> --all [--limit N]` continues a
  stream a previous invocation did not finish. Cursors stay opaque; the CLI
  never constructs or edits one.
- A cap or resume point that cuts through a server page yields
  `page.nextCursor: null` (with `page.hasMore` unchanged), because the Public
  API has no cursor for a position inside a page and reporting the page-end
  cursor would silently skip items.
- `--sort KEY[:DIR]` sends the documented key (`updated`, `dueDate`,
  `priority`, `rank`) and applies the direction the REST parameter does not
  carry. The CLI then makes the ordering of the fetched, bounded result total
  by breaking ties on `key` and then `id` — in both directions — which is what
  makes repeated invocations byte-identical; without a direction the server
  order is kept and only adjacent ties are stabilized. `rank` has no client-side
  representation, so `rank` and `rank:asc` keep the server order and `rank:desc`
  reverses the fetched order. Ordering is never applied to items the CLI did not
  fetch: a direction that must span a whole collection is paired with `--all`
  (a single page is only reordered within itself).

Convenience:

```text
--mine
```

may map to:

```text
--assignee me
```

---

# 43. Collection Output

Human default prints a table.

JSON default returns the API-style collection:

```json
{
  "items": [],
  "page": {
    "limit": 50,
    "hasMore": true,
    "nextCursor": "..."
  }
}
```

With:

```text
--all
```

the CLI follows all pages and SHOULD return:

```json
{
  "items": [...]
}
```

or another explicitly versioned all-results shape.

Choose one shape and test it as a compatibility contract.

---

# 44. Work View

```bash
hamstik work view HAM-42
```

Human output SHOULD include:

```text
key
title
status
type
priority
assignee
reporter
Sprint
parent
labels
story points
due date
description
revision
```

`--json` returns the stable Work Item resource.

---

# 45. Work Create

Required:

```text
--title
```

unless interactive mode prompts for it.

Supported:

```text
--description
--description-file

--type
--status
--priority
--assignee
--sprint
--parent
--story-points
--due-date
```

The CLI MUST NOT expose client-side `reporterId`.

---

# 46. Long Text Input

Mutually exclusive:

```text
--description
--description-file
```

and:

```text
--body
--body-file
```

A file argument of:

```text
-
```

means stdin.

Reads are size-capped so a misdirected stream cannot exhaust memory:

```text
token (prompt / --with-token / HAMSTIK_TOKEN):  4 KiB
description / comment bodies:                   1 MiB
--ca-bundle / HAMSTIK_CA_BUNDLE:                2 MiB
config.toml:                                    1 MiB
.hamstik.toml:                                 64 KiB
API response bodies:                           10 MiB
```

An oversized input is a local error, never a truncated read.

---

# 47. Work Edit

Conceptual:

```text
hamstik work edit <KEY>

--title
--description
--description-file
--type
--priority
--assignee
--sprint
--parent
--story-points
--due-date

--clear-description
--clear-assignee
--clear-sprint
--clear-parent
--clear-story-points
--clear-due-date

--force
```

Do not expose status here.

Help should direct users to:

```text
work transition
work start
work close
```

---

# 48. Work Transition

```bash
hamstik work transition HAM-42 in_review
```

Sequence:

```text
read current item
↓
retrieve/discover transitions
↓
verify requested target is offered
↓
generate idempotency key
↓
POST transition using current ETag
```

Server remains authoritative; client prevalidation improves UX only.

---

# 49. Work Start / Close

```text
work start <KEY>
work close <KEY>
```

Convenience wrappers around transition behavior.

Do not implement separate API semantics.

---

# 50. Comment Commands

```text
hamstik work comment list <KEY>

hamstik work comment add <KEY>
    --body <TEXT>
    --body-file <PATH|->
    --parent <COMMENT_UUID>
```

List supports:

```text
--limit
--cursor
--all
```

---

# 51. Comments and Revision

Comment creation does not alter the Work Item's revision.

The CLI must not assume it does.

---

# 52. Stable Exit Codes

Initial assignments:

```text
0   Success
1   General failure
2   Usage / invalid local command input
3   Authentication failure
4   Authorization / insufficient scope
5   Resource not found
6   Conflict / concurrency / idempotency conflict
7   Rate limited
8   Network / transport failure
9   Server/internal API failure
10  Local configuration / credential-store failure
```

Do not casually reassign these after CLI 1.0.

---

# 53. Server Error Mapping

Examples:

```text
AUTH_REQUIRED / INVALID_TOKEN
    → 3

INSUFFICIENT_SCOPE / FORBIDDEN / ORGANIZATION_SUSPENDED
    → 4

NOT_FOUND
    → 5

REVISION_CONFLICT
IDEMPOTENCY_KEY_REUSED
IDEMPOTENCY_REQUEST_IN_PROGRESS
    → 6

RATE_LIMITED
    → 7

5xx
    → 9
```

`VALIDATION_ERROR` originating from a legitimate server request normally maps to:

```text
2
```

unless it represents a server/API compatibility problem.

---

# 54. JSON Failure Schema

Stable CLI failure:

```json
{
  "error": {
    "kind": "api",
    "code": "REVISION_CONFLICT",
    "message": "The Work Item changed since it was read.",
    "requestId": "...",
    "status": 412
  }
}
```

Local errors may use:

```text
kind = configuration
kind = usage
kind = network
kind = credential_store
```

---

# 55. Quiet Mode

`--quiet` suppresses nonessential human output.

For creation commands it MAY emit only the primary identifier:

```text
HAM-42
```

Do not combine `--quiet` with `--json` ambiguously; either reject the combination or define JSON as taking precedence.

---

# 56. Verbose Mode

`--verbose` may show:

- selected host;
- resolved profile;
- request method/path;
- retry decisions;
- request ID;
- timing.

It MUST NEVER show:

- Authorization header;
- PAT;
- keyring secret.

---

# 57. Doctor

`hamstik doctor` tests in order:

1. config readability;
2. context resolution;
3. profile resolution;
4. credential-store accessibility;
5. host URL validation;
6. DNS/network connectivity;
7. TLS;
8. `/api/v1/openapi.json`;
9. `/api/v1/me`;
10. Organization context;
11. Project context.

Human output:

```text
✓ Config
✓ Credential store
✓ API reachable
✓ Authenticated
✓ Organization
✓ Project
```

Failure diagnostics should offer concrete remediation.

---

# 58. API Compatibility

CLI should understand:

```text
Public API v1
```

A missing expected v1 route or incompatible OpenAPI contract should produce a clear compatibility diagnostic.

Do not require exact OpenAPI document equality; additive server changes are expected.

---

# 59. Shell Completion

Use:

```text
clap_complete
```

Required generators:

```text
bash
zsh
fish
powershell
```

Command:

```bash
hamstik completion <shell>
```

---

# 60. Agent Skill Packaging

Canonical source:

```text
skills/hamstik/
```

The skill is versioned with the CLI source.

The Skill SHOULD tell agents to prefer:

```text
--json
--no-input
```

and explicit context.

It should teach handling of:

```text
REVISION_CONFLICT
RATE_LIMITED
INVALID_STATUS_TRANSITION
```

rather than telling the agent to retry blindly.

---

# 61. Agent Skill Installation

Initial installer design:

```bash
hamstik agent skill install
hamstik agent skill install --global
```

The default target is the current project's portable Agent Skills location;
a dedicated `--project` flag is unnecessary (and would collide with the
global `--project <KEY>` context argument). Project default target SHOULD
prefer:

```text
.agents/skills/hamstik/
```

where compatible with the target harness.

Global installation SHOULD prefer the current portable/harness-supported user-level Agent Skills location.

Because harness conventions evolve, concrete target support MUST be verified against current harness documentation at implementation time.

Re-running an install whose installed skill is byte-identical is idempotent
(no-op). Do not silently overwrite a modified local skill.

Support:

```text
--force
```

for explicit replacement. The binary carries the canonical skill and its
frontmatter metadata (`skill-version`, `minimum-cli-version`) so install and
`hamstik agent skill check` work without network access; `HAMSTIK_SKILL_HOME`
may override the global root directory (the directory containing the
portable `skills/` tree).

---

# 62. Skill Version Metadata

The installed skill SHOULD include or accompany metadata indicating:

```text
Hamstik Skill version
compatible CLI minimum version
```

The agent should be able to detect obviously stale instructions.

---

# 63. CLI Docs Generation

The `clap` command tree SHOULD be exportable into:

```text
Markdown command reference
machine-readable command manifest
```

Prefer a build-time `xtask` or dedicated internal documentation generator rather than hand-maintaining every option twice.

---

# 64. Canonical User Docs

Generated CLI reference and narrative docs ultimately belong in the **main Hamstik application repository**.

Suggested main-repo structure:

```text
docs/cli/
├── README.md
├── installation.md
├── authentication.md
├── contexts.md
├── agent-skills.md
└── reference/
```

Exact structure should follow the existing Hamstik documentation system.

---

# 65. Cross-Repository Documentation Gate

CLI releases that change:

- commands;
- options;
- output behavior;
- auth;
- config;
- Agent Skill installation;

MUST have a corresponding documentation change in the main Hamstik repository.

Release automation SHOULD create/update the generated command reference automatically.

---

# 66. Build Targets

Release CI SHOULD build at least:

```text
x86_64-pc-windows-msvc
aarch64-pc-windows-msvc

x86_64-apple-darwin
aarch64-apple-darwin

x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
```

Optional:

```text
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
```

---

# 67. Release Automation

Use a Rust-native release automation system such as:

```text
cargo-dist
```

where it reduces custom CI/packaging work.

Release automation should produce:

- platform archives;
- checksums;
- installer scripts;
- release metadata;
- provenance/attestation where supported.

---

# 68. Windows Packaging

Initial:

```text
.zip
PowerShell installer
Winget manifest/package
```

Binary:

```text
hamstik.exe
```

Release binary SHOULD be Authenticode signed before broad GA distribution.

---

# 69. macOS Packaging

Initial:

```text
.tar.gz
Homebrew tap
```

Provide:

```text
Apple Silicon
Intel
```

Release binary SHOULD be code-signed and notarized for GA.

---

# 70. Linux Packaging

Initial:

```text
.tar.gz
shell installer
.deb
.rpm
```

APT/RPM hosted repositories are optional post-launch enhancements.

---

# 71. npm Distribution

Optional package:

```text
@hamstik/cli
```

Its responsibility is distribution only.

It installs/selects the appropriate native platform binary.

The native binary is still the actual CLI.

Do not ship a second TypeScript implementation.

---

# 72. Direct Download

Every release should provide standalone archives so package-manager availability never becomes a hard dependency.

Users must be able to verify downloads using published checksums.

---

# 73. Update Strategy

Initial updates occur through the installation channel:

```text
Winget
Homebrew
npm wrapper
package install
installer script
manual release download
```

Do not add silent self-update behavior.

A future:

```text
hamstik update
```

may be considered later.

---

# 74. Release Security

Release CI SHOULD include:

- locked Cargo dependencies;
- tests;
- Clippy;
- rustfmt;
- vulnerability/advisory scanning;
- license review;
- checksums;
- artifact attestation/provenance;
- platform signing where applicable.

---

# 75. Rust Quality Gates

At minimum:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo build --release
```

Add platform CI for:

```text
Windows
macOS
Linux
```

---

# 76. API Mock/Fixture Tests

The API client needs deterministic HTTP tests covering:

- success responses;
- structured errors;
- ETag;
- rate limits;
- retries;
- network failures;
- malformed server data;
- idempotency replay.

Use a local mock HTTP server in unit/integration tests.

---

# 77. Live Compatibility Tests

Where CI access permits, maintain a test against a controlled Hamstik test server.

Do not make ordinary developer unit tests depend on `hamstik.com`.

---

# 78. Context Tests

Cover:

- global profile default;
- environment override;
- `.hamstik.toml`;
- command flags;
- nested directories;
- multiple profiles;
- host mismatch;
- invalid Project;
- no context.

---

# 79. Credential Tests

Use credential-store mocks where possible.

Test:

- store;
- retrieve;
- delete;
- unavailable keyring;
- wrong account;
- environment token precedence;
- no secret in config.

Never use a developer's real OS credential store in ordinary unit tests.

---

# 80. Output Golden Tests

Human formatting MAY use snapshot/golden tests.

Machine `--json` output MUST use structural assertions.

Do not make ANSI terminal formatting part of the JSON contract.

---

# 81. Non-Interactive Tests

Every mutation must have tests proving:

```text
--json
```

and:

```text
--no-input
```

never wait for user input.

---

# 82. Exit-Code Tests

Every stable exit-code category needs direct tests.

Agent reliability depends on this contract.

---

# 83. Sensitive Data

Never log or display:

```text
HAMSTIK_TOKEN
stored PAT
Authorization header
credential-store secret
```

Error display may mention:

```text
credential expired
credential invalid
```

without echoing it.

## Local Mutation Audit Log

Every successful mutation additionally appends one JSON line to a per-user,
append-only local audit log:

```json
{"when":"...","command":"work.edit","target":"HAM-42","revisionBefore":7,"revisionAfter":8,"requestId":"..."}
```

Rules:

- exactly one record per successful mutation, never for reads or dry-runs;
- identifiers only — command path, target identifier, revision before/after when
  known (`null` otherwise), and the server request id;
- never a request/response body, work text, token, `Authorization` header, or
  any value from the sensitive list above;
- stored in the platform state directory (`$XDG_STATE_HOME/hamstik/audit.log`
  by default, the per-user application-support/local-application-data location
  on macOS/Windows), overridable with `HAMSTIK_AUDIT_LOG`; owner-only on Unix;
- best effort: an unwritable log warns and never fails the mutation, and losing
  the local trail is acceptable because the server-side audit record stays
  authoritative;
- opt-out is the `audit_log` setting (default enabled) and the effective path is
  reported by `hamstik doctor`;
- the CLI never rotates the log: one short line per mutation, and truncating or
  deleting the file is always safe.

---

# 84. Crash/Panic Policy

Expected runtime failures MUST return normal errors.

Do not panic for:

- missing config;
- invalid context;
- auth failure;
- network failure;
- malformed API response.

Panic is reserved for genuine programmer invariants.

---

# 85. Telemetry

No telemetry implementation in initial release.

Do not create hidden network calls beyond the configured Hamstik API.

---

# 86. Public Repository

The CLI repository is public. Repository licensing was decided explicitly:
the project is distributed under Apache-2.0 (see `LICENSE`), and every source
file carries an `SPDX-License-Identifier: Apache-2.0` header. Dependencies
must remain compatible with Apache-2.0 distribution.

No proprietary code from the main Hamstik application should be copied into
the CLI repository merely for convenience.

The CLI should depend only on the documented Public API contract.

---

# 87. Implementation Milestone A — Dogfooding Alpha

Required:

```text
Rust project
API client
PAT auth
profiles
context
.hamstik.toml

auth status/login/logout
org list/view/use
project list/view/use

work list/view/create/edit
work transitions/transition/start/close
work comment list/add

--json
--no-input
exit codes

doctor
completion
```

This is enough to begin using Hamstik CLI to develop Hamstik.

---

# 88. Implementation Milestone B — Cross-Platform Release

Add/complete:

```text
Windows/macOS/Linux CI
release automation
install scripts
Winget
Homebrew
Linux packages
npm native wrapper if retained
signing/provenance
generated command docs
Agent Skill
```

---

# 89. CLI 1.0 Definition

CLI `1.0.0` requires:

- stable command hierarchy;
- stable machine JSON behavior;
- stable exit codes;
- reliable PAT auth;
- cross-platform distribution;
- context handling;
- complete Public API v1 dogfooding workflow;
- canonical Agent Skill;
- main Hamstik documentation updated;
- successful real-world dogfooding.

---

# 90. Architectural Invariants

1. The official binary is `hamstik`.
2. The CLI is written in Rust.
3. Users do not need a language runtime.
4. Windows, macOS, and Linux are first-class.
5. The CLI uses only Public API `/api/v1`.
6. The CLI does not duplicate server domain rules.
7. PAT is the initial authentication mechanism.
8. Interactive PATs are stored in OS credential stores.
9. Secrets never live in normal config.
10. `HAMSTIK_TOKEN` supports ephemeral/headless auth.
11. Future OAuth must fit behind `hamstik auth login`.
12. Multiple profiles/hosts are supported.
13. Repository-local `.hamstik.toml` is non-secret.
14. `--json` is a stable machine interface.
15. stdout contains requested data; diagnostics use stderr.
16. `--json` and `--no-input` never prompt.
17. Server error codes are preserved.
18. Stable exit codes exist.
19. Idempotency keys are automatically generated and reused for safe retries.
20. Work Item edits use ETags and do not silently overwrite conflicts.
21. Cursors remain opaque.
22. Package-manager choice does not change CLI behavior.
23. npm, if supported, distributes the native binary rather than a Node CLI.
24. Canonical user-facing CLI docs remain in the main Hamstik repository.
25. The canonical Agent Skill lives with the CLI source.
26. MCP is not part of the initial CLI architecture.

---

# 91. Initial Acceptance Scenario

The CLI is ready for Hamstik dogfooding when the following succeeds on Linux, Windows, and macOS:

```bash
hamstik auth login

hamstik context set \
  --org blackboard-studios \
  --project HAM

hamstik auth status

hamstik work list --scope open

hamstik work view HAM-42

hamstik work comment list HAM-42

hamstik work start HAM-42

hamstik work edit HAM-42 \
  --priority high

hamstik work comment add HAM-42 \
  --body-file completion.md

hamstik work close HAM-42
```

The equivalent workflow must also work non-interactively using:

```text
HAMSTIK_TOKEN
--json
--no-input
```

for coding agents.
