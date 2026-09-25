# Workflow Automation Rules — Gated Design Stub

**Status:** Gated — design only. Not implemented, no network behavior, no command
surface. This document is the deliverable until the gate below opens.
**Work Item:** CLI-77 (`hamstik rule` / workflow automation rules)
**Epic:** CLI-49 — Agent and automation support
**Gating model:** the same "do not build until the Public API exists" model used
for CLI-5 (OAuth) and CLI-17 (MCP).

---

## 1. Gate condition

CLI-77 MUST NOT ship executable behavior until all of the following hold:

1. Hamstik **Public API v1** (`/api/v1`) exposes a documented event-stream or
   event-subscription surface in the frozen contract snapshot
   (`openapi/hamstik-v1.json`) and its explanations (`openapi/README.md`).
2. That surface is covered by the parity ledger (`openapi/api-parity.json`) and
   a CLI/API-client method plus mock tests, exactly like every other operation.

Until then:

- No `hamstik rule` command is added to the CLI.
- No background daemon, watcher process, scheduler, or polling loop is added.
- No connection is opened to any event, webhook, or subscription endpoint.
- `work watch` / `work await` are **not** repurposed into an event stream. They
  poll existing activity/comment endpoints for observation only; polling is not
  a subscription and MUST NOT be used to fake one.

The current contract snapshot exposes no event-subscription surface. The only
path matching "event" is the read-only
`/api/v1/organizations/{organizationSlug}/milestones/{milestoneId}/events`
history listing; it is not a subscription, push channel, or trigger source.

## 2. Outcome

CLI-owned **rule definitions** — for example "when a Work Item is closed, add
comment X" — that a future server event stream (or an explicit manual trigger)
can evaluate on the user's behalf. Rules let agents and humans express
automation as data instead of hard-coding the same behavior into every agent.

## 3. Scope (to build only after the gate opens)

- A local, non-secret rule store managed by `rule list`, `rule create`, and
  `rule delete`.
- Rules are declarative data: a trigger, an optional filter, and an action that
  names an existing typed CLI/Public API operation. A rule is never executable
  code.
- Execution is driven by exactly one of:
  - a server event subscription (primary), once the Public API provides one; or
  - an explicit manual trigger invoked by a user or agent.
- The rule store lives beside other non-secret CLI configuration and contains
  no credentials. Authentication continues to come from the normal credential
  sources (`HAMSTIK_TOKEN`, OS keyring) at execution time.

Everything the server owns stays server-owned. Authorization, validation,
Work Item status transitions, ETags/revisions, idempotency, Organization
isolation, and capability/subscription checks are performed by the Hamstik
server exactly as they are for any other command. The CLI only supplies rule
ergonomics around those rules.

## 4. Triggers

| Trigger | Availability | Notes |
| --- | --- | --- |
| Server event subscription | **Blocked** — needs Public API v1 event surface | Primary trigger. CLI subscribes to documented event types and evaluates matching local rules. Never implemented by polling. |
| Explicit manual trigger | Possible locally, but still gated | A user/agent asks the CLI to evaluate one rule against a named context/Work Item and perform its action through the existing typed command path. |

Both trigger paths funnel into the **same action executor** so behavior cannot
diverge between subscribed and manually triggered runs.

## 5. Proposed command surface (not implemented)

Sketch only; final spelling is settled when the gate opens and is subject to
the normal command-grammar and generated-docs process.

```text
hamstik rule list
hamstik rule create --file <path|->      # declarative rule definition
hamstik rule delete <name>
hamstik rule trigger <name> --work-item <KEY>   # explicit manual trigger
```

Rules MUST be offline-readable/listable without contacting the server; only
`rule trigger` and any future subscription path may perform network work.

## 6. Non-goals

- No background daemon, service, or long-lived process.
- No polling loops and no synthesized event stream.
- No rule execution, network request, or command surface before the gate opens.
- No arbitrary code, shell command, or plugin execution from a rule definition.
- No duplication of server business logic (transitions, validation, authz,
  idempotency, revisions, Organization isolation).
- No secrets, tokens, or credentials stored in the rule store.
- No cross-Organization evaluation or escalation.
- No event delivery guarantees invented client-side (ordering, at-least-once,
  etc.); those are server contract properties and must be documented there
  first.

## 7. Activation checklist (when the gate opens)

1. Add the documented event/subscription operations to
   `openapi/hamstik-v1.json`, `openapi/README.md`, and `openapi/api-parity.json`.
2. Add API-client methods and mock/fixture tests for the new operations.
3. Implement `rule` read/create/delete and the explicit manual trigger first,
   fully offline/local except for the action itself.
4. Implement the subscription execution path only against the documented
   contract, with the same idempotency, ETag, error-mapping, and output rules
   as every other command.
5. Add contract/golden tests, update the Agent Skill, changelog, and generated
   command docs in the same change.
