# Webhook Management

**Status:** Implemented against Hamstik Public API v1.0.0.
**Work Item:** CLI-78 (`hamstik webhook`)
**Epic:** CLI-49 — Agent and automation support

---

## 1. Gate activation

The original feature gate is open. The frozen Public API contract in
`openapi/hamstik-v1.json` now exposes Organization webhook endpoint, delivery,
secret-rotation, test, and redelivery operations under
`/api/v1/organizations/{organizationSlug}/webhooks`.

Every operation is classified in `openapi/api-parity.json`, travels through the
authenticated Public API client, and is covered by contract and wire tests. The
command is therefore a real contract-backed resource family, not an alias for
`api request`.

## 2. Command surface

```text
hamstik webhook list [--query name=value ...]
hamstik webhook view <webhook-id>
hamstik webhook create (--body-file <path|-> | --field key=value ...) [--idempotency-key <key>]
hamstik webhook edit <webhook-id> (--body-file <path|-> | --field key=value ...) --if-match <etag> [--idempotency-key <key>]
hamstik webhook delete <webhook-id> [--idempotency-key <key>]
hamstik webhook rotate <webhook-id> [--idempotency-key <key>]
hamstik webhook test <webhook-id> [--idempotency-key <key>]
hamstik webhook deliveries <webhook-id> [--query name=value ...]
hamstik webhook redeliver <webhook-id> <delivery-id> [--idempotency-key <key>]
```

`organizationSlug` resolves through the normal context precedence. Query names,
required JSON bodies, `If-Match`, and `Idempotency-Key` behavior are validated
against the frozen operation definitions before the request is sent. Delete is
locally consent-gated like the CLI's other destructive operations.

The server returns a signing secret only from create or rotate. The CLI may
render that response to the caller but never writes the secret to configuration,
logs, audit records, or the failed-request journal.

## 3. Server authority

The server remains authoritative for authorization, Organization isolation,
endpoint validation, event capabilities, signing, delivery ordering and retry
semantics, revisions/ETags, and idempotency. Test delivery and redelivery call
the explicit Public API operations; the CLI never contacts a webhook target
directly.

## 4. Non-goals

- No webhook receiver, HTTP listener, tunnel, or local server.
- No client-side delivery, retry, signing, or signature-verification engine.
- No polling repurposed as webhook delivery.
- No secret persistence or display in diagnostic/audit output.
- No private Hamstik browser route.

`work await` and `work watch` remain observation-only polling commands. Raw
future routes remain reachable through `api request`, while frozen operations
without an ergonomic family are available through contract-aware `api invoke`.
