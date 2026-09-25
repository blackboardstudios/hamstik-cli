# Webhook Management — Gated Design Stub

**Status:** Gated — design only. Not implemented, no network behavior, no command
surface. This document is the deliverable until the gate below opens.
**Work Item:** CLI-78 (`hamstik webhook` / webhook management command family)
**Epic:** CLI-49 — Agent and automation support
**Gating model:** the same "do not build until the Public API exists" model used
for CLI-5 (OAuth), CLI-17 (MCP), and CLI-77 (workflow automation rules).

---

## 1. Gate condition

CLI-78 MUST NOT ship executable behavior until all of the following hold:

1. Hamstik **Public API v1** (`/api/v1`) exposes documented webhook or
   subscription-management operations in the frozen contract snapshot
   (`openapi/hamstik-v1.json`) and its explanations (`openapi/README.md`).
2. Those operations are covered by the parity ledger
   (`openapi/api-parity.json`) and a CLI/API-client method plus mock tests,
   exactly like every other operation.

Until then:

- No `hamstik webhook` command is added to the CLI, not even a thin passthrough
  or alias over `api request`.
- No webhook registration, delivery, retry, signature-verification, or
  endpoint-probing behavior is added.
- `work await` and `work watch` remain **observation-only** polling of existing
  activity/comment endpoints. They MUST NOT be repurposed into a webhook
  receiver or an event stream.
- Manual webhook use stays possible through the generic, contract-driven
  `hamstik api request` passthrough once the server exposes the routes; that is
  a documented escape hatch, not a typed command family.

The current contract snapshot exposes no webhook or subscription surface. The
only path matching "event" is the read-only
`/api/v1/organizations/{organizationSlug}/milestones/{milestoneId}/events`
history listing; it is not a webhook, subscription, push channel, or trigger
source.

## 2. Outcome

A typed `hamstik webhook` command family that lets a user or agent inspect and
manage Hamstik **outbound webhook subscriptions** — the delivery targets that
notify an external endpoint when Work Items, comments, or other resources
change. It is the natural complement to `work await`, which observes state by
polling; webhooks would let automation react without polling.

## 3. Scope (to build only after the gate opens)

The command family is a conventional typed resource surface, subject to the
normal command-grammar, output, pagination, idempotency, and error-mapping
rules that every other command follows:

- **List / view** webhook subscriptions, including their target URL (redacted
  per the server contract), subscribed event types, active state, and timestamps.
- **Create** a webhook subscription from a declarative definition (file or
  stdin), with an idempotency key so retries are safe.
- **Edit / enable / disable / delete** a subscription, honoring ETags and the
  server's optimistic-concurrency rules.
- **Delivery history / recent deliveries** for a subscription, if and only if
  the Public API exposes that read surface.
- **Test delivery / ping**, if and only if the Public API exposes an explicit
  operation for it. The CLI will not synthesize a test by posting to the target
  URL itself.

Everything the server owns stays server-owned. Authorization, validation,
Organization isolation, subscription and event-type capability checks, delivery
semantics (ordering, retries, at-least-once/at-most-once guarantees), signing
and secret material, and ETags/revisions are performed and defined by the
Hamstik server exactly as for any other command. The CLI only supplies typed
ergonomics around those rules.

## 4. Proposed command surface (not implemented)

Sketch only; final spelling is settled when the gate opens and is subject to
the normal command-grammar and generated-docs process.

```text
hamstik webhook list [PAGINATION]
hamstik webhook view <id>
hamstik webhook create --file <path|-> [--idempotency-key <key>]
hamstik webhook edit <id> --file <path|-> [--force] [--idempotency-key <key>]
hamstik webhook enable <id> [--force] [--idempotency-key <key>]
hamstik webhook disable <id> [--force] [--idempotency-key <key>]
hamstik webhook delete <id> [--force] [--idempotency-key <key>]
hamstik webhook deliveries <id> [PAGINATION]   # only if the API exposes it
hamstik webhook test <id>                       # only if the API exposes it
```

`list` and `view` are read-only and require no local secret. Every mutating
subcommand follows the CLI's existing mutation conventions: `--no-input` for
agents, destructive-operation consent, idempotency keys for retried creates and
edits, and `--force`/ETag handling for optimistic concurrency.

## 5. Non-goals

- No command surface, network request, or webhook behavior before the gate
  opens.
- No passthrough-only command indirection: a `hamstik webhook` command that
  merely forwards to `api request` is explicitly out of scope. The typed
  family ships only with real contract-backed behavior.
- No webhook receiver, HTTP listener, tunnel, or local server.
- No delivery, retry, or signature-verification logic implemented in the
  client; those are server contract properties.
- No polling repurposed as a webhook substitute.
- No duplication of server business logic (authorization, validation,
  Organization isolation, event-type checks, delivery guarantees, revisions).
- No secret material, signing keys, or target credentials stored locally by the
  CLI beyond what the server contract requires.
- No event delivery guarantees invented client-side (ordering, at-least-once,
  etc.); those must be documented in the Public API first.

## 6. Activation checklist (when the gate opens)

1. Add the documented webhook/subscription operations and schemas to
   `openapi/hamstik-v1.json`, `openapi/README.md`, and
   `openapi/api-parity.json`.
2. Add API-client methods and mock/fixture tests for the new operations, with
   the same error-mapping, ETag, idempotency, opt-in safety, and output rules as
   every other command.
3. Implement the read-only `webhook list` / `webhook view` commands first.
4. Implement the mutating `create` / `edit` / `enable` / `disable` / `delete`
   commands against the documented contract, including consent, idempotency,
   and optimistic concurrency.
5. Add `deliveries` and `test` only for operations the contract actually
   exposes.
6. Add contract/golden tests, update the Agent Skill, changelog, and generated
   command docs in the same change.
