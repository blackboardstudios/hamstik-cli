# Server-Side Workflow Automation Rules

**Status:** Implemented against Hamstik Public API v1.0.0.
**Work Item:** CLI-77 (`hamstik rule`)
**Epic:** CLI-49 — Agent and automation support

---

## 1. Gate activation and revised ownership

The original design proposed CLI-owned local rules because no server contract
existed. Public API v1.0.0 now exposes Organization automation rules directly
under `/api/v1/organizations/{organizationSlug}/automation-rules`, so that
proposal is superseded: rules, triggers, actions, ownership, enablement, run
history, validation, and execution are server-owned.

Every automation-rule operation is frozen in `openapi/hamstik-v1.json`,
classified in `openapi/api-parity.json`, and covered by the authenticated API
client plus contract/wire tests. The CLI does not create a parallel local rule
store or duplicate the server evaluator.

## 2. Command surface

```text
hamstik rule list [--query name=value ...]
hamstik rule view <rule-id>
hamstik rule create (--body-file <path|-> | --field key=value ...) [--idempotency-key <key>]
hamstik rule edit <rule-id> (--body-file <path|-> | --field key=value ...) --if-match <etag> [--idempotency-key <key>]
hamstik rule delete <rule-id> [--idempotency-key <key>]
hamstik rule runs <rule-id> [--query name=value ...]
hamstik rule preview <rule-id> (--body-file <path|-> | --field key=value ...)
hamstik rule toggle <rule-id> (--body-file <path|-> | --field key=value ...) [--idempotency-key <key>]
hamstik rule accept-owner <rule-id> --if-match <etag> [--idempotency-key <key>]
```

`organizationSlug` resolves through normal context precedence. Query names,
required bodies, revision preconditions, and idempotency behavior come from the
frozen operation definition. `preview` calls the server's non-executing preview
operation; it does not run actions locally. Delete requires explicit local
destructive consent.

## 3. Server authority

The server remains authoritative for authorization, Organization isolation,
trigger and action validation, ownership transfer, execution, transitions,
revisions/ETags, idempotency, and run history. The CLI supplies request and
output ergonomics only.

## 4. Non-goals

- No local rule database or independently interpreted rule format.
- No background daemon, scheduler, watcher process, or polling loop.
- No arbitrary code, shell, or plugin execution from a rule definition.
- No synthesized event stream and no client-side action executor.
- No secrets or credentials stored in rule definitions.
- No private Hamstik browser route.

`work watch` and `work await` remain observation-only polling. The automation
service operates entirely through its documented Public API v1 resources.
