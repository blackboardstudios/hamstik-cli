# Live Acceptance Suite (opt-in)

`tests/live_acceptance.rs` drives the real `hamstik` binary against a **real**
Hamstik deployment over the documented Public API v1 (`/api/v1`). It exists to
prove that authentication, TLS, context resolution, and the CLI's typed
commands work end-to-end against a live server — nothing else in this
repository needs network reachability.

## Safety properties

- **Opt-in.** Both tests are `#[ignore]`d and additionally early-return when
  the required environment variables are absent. Plain `cargo test --workspace`
  (offline, no reachability) never runs them; it only compiles them.
- **Dedicated credentials only.** Use a dedicated test Personal Access Token
  with at most the scopes the scenario needs — never a real operator PAT.
- **Isolated test data.** The suite requires a dedicated test Organization and
  Project (env vars below). It never touches production data.
- **No secret leakage.** Every CLI invocation receives `HAMSTIK_TOKEN` through
  the process environment (never argv, never a file); diagnostics log the argv
  shape and the command's stdout/stderr but never the token or any
  Authorization material. `HAMSTIK_CONFIG` points at a throwaway temp dir and
  `HAMSTIK_PROFILE` is removed, so nothing reads or writes a real profile.
- **Run-unique marker.** All created resources are stamped with
  `acc-<unix-ts>-<pid>`, so cleanup removes exactly what the run created and
  two runs never collide.
- **Cleanup without masking.** The mutation scenario wraps its assertions in
  `catch_unwind`, deletes every created Work Item (delete first, archive as
  fallback), then resumes the panic — a cleanup failure adds a panic but never
  hides the primary assertion failure.

## Required environment

| Variable | Meaning |
| --- | --- |
| `HAMSTIK_TOKEN` | Dedicated test PAT (ephemeral; never persisted). |
| `HAMSTIK_HOST` | Test deployment origin, e.g. `https://hamstik.example.test`. |
| `HAMSTIK_ACCEPTANCE_ORG` | Dedicated test Organization slug. |
| `HAMSTIK_ACCEPTANCE_PROJECT` | Dedicated test Project key. |
| `HAMSTIK_ACCEPTANCE_MUTATIONS` | `true` to enable the write scenario (read-only otherwise). |

## Scenarios

1. **`live_smoke_read_only`** — no writes at all: `me`, `context show`,
   Organization/Project/Sprint/Label/Work Item read collections and views,
   SqueakQL validation, `api openapi`, and one `api request /api/v1/me`.
2. **`live_acceptance_mutations`** (needs `HAMSTIK_ACCEPTANCE_MUTATIONS=true`
   and a PAT with write scopes) — the full lifecycle:
   create with an explicit idempotency key plus replay, view/edit, a **real
   ETag conflict** (stale `If-Match` is rejected by the server; `--force`
   succeeds), transitions, labels, comments (add/reply/edit/delete), links,
   watcher state, attachments (byte-for-byte download check), a sprint walked
   to `done`, bulk create/update/transition, and a SqueakQL search.

## Known residue (by design)

The Public API v1 documents no delete routes for **labels** and **sprints**.
The mutation scenario therefore:

- reuses an existing project label when the project has one; if it must create
  one, the label is named `<marker>-label` and remains;
- creates exactly one sprint named `<marker> sprint` and leaves it in `done`.

Everything else the run creates is removed by cleanup.

## Running

```bash
HAMSTIK_TOKEN=<test-pat> \
HAMSTIK_HOST=https://<test-instance> \
HAMSTIK_ACCEPTANCE_ORG=<test-org> \
HAMSTIK_ACCEPTANCE_PROJECT=<test-project> \
cargo test --test live_acceptance -- --test-threads=1 --ignored
```

Use `--test-threads=1` (the runbook default): the scenario is strictly
sequential and shares one throwaway config dir. Add
`HAMSTIK_ACCEPTANCE_MUTATIONS=true` to run the write scenario.

## CI

CI compiles the suite on every push but never runs it (the env vars are
absent). `.github/workflows/live-acceptance.yml` adds a **manual**
(`workflow_dispatch`) job that runs the suite against a configured test
deployment when repository variables/secrets opt in — see that file's header
for the exact configuration contract.
