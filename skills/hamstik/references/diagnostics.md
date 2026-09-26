# Hamstik CLI diagnostics and API reference

## Local configuration

Use:

```bash
hamstik --json config list
hamstik --json config get editor
hamstik --json config set editor <command>
hamstik --json config unset editor
hamstik --json --no-input context explain
```

Configuration contains non-secret defaults only. Never store a token/password there.

Changing `profile`, `organization`, or `project` changes where later commands may act,
so prefer explicit flags for agent mutations and confirm before modifying global
defaults.

## Public API passthrough

Use typed commands when they exist. For Public API v1 routes without a typed command:

```bash
hamstik api /api/v1/organizations
hamstik api request --method POST --field name=Acme /api/v1/organizations
hamstik api request --method PATCH --body-file body.json /api/v1/organizations/acme
```

Only `/api/v1/...` paths are accepted. Private/non-v1/traversal/credential paths are
invalid.

GET is the default. Mutating passthrough requests use the CLI's idempotency/revision
protections. Header overrides are restricted; never attempt to inject Authorization or
other credential-bearing headers.

`--dry-run` can preview a mutation request and sends nothing.

Structured passthrough output contains server data and metadata such as request ID,
ETag, idempotency replay status, location, and rate-limit information. Preserve request
IDs on failure.

## Rate limits

Use:

```bash
hamstik --json --no-input api rate-limit
```

This is a point-in-time snapshot, not a guarantee against future 429 responses.

On `RATE_LIMITED`, respect CLI retry behavior and server `Retry-After`; never tight-loop.

## Doctor and support bundles

```bash
hamstik --no-input doctor
hamstik --no-input doctor --local-only
hamstik --json --no-input doctor
hamstik --no-input doctor --bundle bundle.zip
hamstik --json --no-input doctor --diff bundle.zip
```

`doctor --local-only` avoids DNS/HTTP traffic and checks local configuration, context,
credential-store access, terminal behavior, and compatibility metadata.

Support bundles are redacted. `doctor --diff` compares current local-only diagnostics to
a saved bundle without contacting the network unless another requested operation
requires it.

Doctor reports pass/warn/fail/skipped totals and classifies network failures by stage.

## Failed-request journal

Recent failed requests can be inspected locally:

```bash
hamstik --json --no-input replay --last 5
```

The journal contains redacted request metadata and never re-sends a request.

## Agent harness validation

Validate the skill/configuration against the CLI:

```bash
hamstik --no-input agent validate --offline
hamstik --json --no-input agent validate
```

The offline form does not make network requests. The online form can additionally check
the bundled OpenAPI snapshot against the server.

Treat an outdated skill, suspicious credential-shaped config content, or stale snapshot
as something to surface rather than ignore.

## Safe failure reporting

On failure, report:

- stable API error code;
- HTTP status when present;
- server request ID when present;
- concise safe remediation.

Never include:

- PATs/tokens;
- Authorization headers;
- credential-bearing proxy URLs;
- request bodies or other material that may contain secrets unless the user explicitly
  supplied and needs that non-secret content.
