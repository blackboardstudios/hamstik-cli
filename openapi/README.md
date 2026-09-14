# Hamstik Public API v1 contract snapshot

`hamstik-v1.json` is a **frozen development snapshot** of the Hamstik Public API v1
OpenAPI 3.1.1 contract. It exists to support Hamstik CLI development and contract
testing.

## Authority

The authoritative runtime contract is always the live server document at:

```text
https://www.hamstik.com/api/v1/openapi.json
```

Do not treat this snapshot as more authoritative than the server implementation. Additive
server changes are expected; the snapshot is refreshed intentionally when the Public API
contract changes in a way the CLI must track.

## Purpose

- development reference for implementing the typed Rust API client;
- contract testing against expected operation inventory;
- detecting unexpected client/server drift.

`api-parity.json` is the machine-readable operation support manifest. The
`openapi_parity` integration test parses both JSON documents and fails whenever
an `operationId` is added, removed, moved, or changed without a deliberate
client/CLI/test classification.

## Deep schema parity

Beyond the operationId/method/path manifest, parsed-schema guards
(`crates/hamstik-api-client/tests/schema_parity.rs`) fail whenever the snapshot
changes request/response semantics the client or CLI must account for:

- parameters (names, `in`, required), request content types (including the
  multipart attachment upload), and request schemas;
- response status codes, content types (including binary image downloads),
  and response schemas;
- header parameters (`If-Match` on revision-protected mutations,
  `Idempotency-Key` on retriable mutations) and repeated `form`/`explode`
  query filters plus the comma-separated `fields` fieldset;
- component schema drift: required vs. optional vs. nullable fields, enum
  values, formats, and property removals (structural comparison, no key
  ordering);
- bulk envelope bounds (`minItems: 1`, `maxItems: 50`) and
  `additionalProperties: false` on operation schemas.

Additive compatible changes (new optional properties, enum values) produce a
reviewable additive classification; breaking drift fails the test until the
Rust models, CLI, and wire tests deliberately account for it and the manifest
is reclassified in the same change. Failure output names the operationId or
JSON pointer of every mismatch.

## Checking and updating

Live access is explicit and never part of an offline build:

```bash
scripts/update-openapi.sh --check
scripts/update-openapi.sh --update
cargo test -p hamstik-api-client --test openapi_parity
```

`--check` reports drift without changing the working tree. `--update` first
downloads and validates the live JSON, then replaces the snapshot byte-for-byte;
the snapshot is never edited by hand. After an update:

1. run `cargo test -p hamstik-api-client --test schema_parity` and the
   `openapi_parity` tests; every failure names the changed operation or schema
   pointer;
2. implement the new or changed semantics in the typed client and CLI, and
   extend the wire tests for the affected behavior;
3. update `api-parity.json` (client method, CLI commands, tests, status) in
   the same change;
4. note the change in the changelog — a `### Breaking` entry when a
   documented CLI surface changed — so the compatibility sign-off is explicit
   in review.

## Rules

- Refresh `hamstik-v1.json` deliberately, never casually.
- Download the snapshot; do not manually edit it.
- CLI builds must not fetch OpenAPI from the network.
- No OpenAPI code generation runs against this snapshot; the design calls for a
  deliberate hand-written typed client.
