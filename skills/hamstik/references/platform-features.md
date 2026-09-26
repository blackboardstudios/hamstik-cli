# Hamstik CLI platform feature reference

Use this file when the task goes beyond routine Work Item operations. Exact command
syntax and currently available flags come from the installed CLI.

## Organization Attributes

Attributes are Organization-governed metadata with immutable machine keys. Supported
types are `single_select`, `multi_select`, and `boolean`.

Prefer typed Attribute commands over raw API routes:

```bash
hamstik --json --no-input --org <ORG> attribute list --include-retired
hamstik --json --no-input --org <ORG> attribute view <ATTRIBUTE-KEY>
hamstik --json --no-input --org <ORG> attribute create \
  --key product_area --name "Product Area" --type multi_select
hamstik --json --no-input --org <ORG> attribute option add product_area \
  --key mobile --label "Mobile"
hamstik --json --no-input --org <ORG> --project <KEY> attribute project list
```

Governance writes require the applicable server authority and `attribute:write` scope.
Select options use stable keys, not labels.

Work Item Attribute mutations use stable keys. Omission preserves an existing value,
`false` is a real boolean value, and clearing is explicit.

The server is authoritative for Attribute type, Project enablement, options, retained
values, revisions, and authorization.

## Releases and milestones

Release workflows follow the read → transitions → transition pattern:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> release list --all
hamstik --json --no-input --org <ORG> --project <KEY> release view <RELEASE-ID>
hamstik --json --no-input --org <ORG> --project <KEY> release transitions <RELEASE-ID>
```

Releasing incomplete scope requires explicit confirmation. Archiving a release is
destructive/consent-gated.

Release Work Item associations, announcement drafts, and audit packages are available
through typed commands. ZIP audit packages must be written with `--output`.

Organization milestones are Organization-scoped, not Project-scoped:

```bash
hamstik --json --no-input --org <ORG> milestone list --all
hamstik --json --no-input --org <ORG> milestone view <MILESTONE-ID>
```

Removing a release from a milestone requires explicit removal confirmation. Archiving a
milestone is consent-gated.

## Project and Sprint reports

Server reports are authoritative for server-computed metrics:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> project report velocity
hamstik --json --no-input --org <ORG> --project <KEY> sprint report <SPRINT-ID>
```

Do not locally recreate velocity, burndown, scope changes, or similar server metrics.
Preserve server-reported limitations in the answer.

Project report types currently include server-supported values such as velocity,
cumulative-flow, control-chart, ageing-wip, created-vs-resolved, distribution, and
epic-progress. Use `--help` rather than maintaining a local allowlist.

## Client-side stats

`project stats` and `sprint stats` are the sanctioned simple client-side aggregate
summaries over fetched Work Items:

```bash
hamstik --json --no-input --org <ORG> --project <KEY> project stats <KEY> --all
hamstik --json --no-input --org <ORG> --project <KEY> sprint stats <SPRINT-ID> --all
```

Use these for counts/sums by status/type/priority/assignee and story points, not as a
replacement for server reports.

Without `--all`, results can be truncated. Say explicitly when an answer uses
client-computed counts over fetched items rather than server-reported metrics.

## Advanced Reports and Dashboards

These are Organization-scoped and capability-gated:

```bash
hamstik --json --no-input --org <ORG> report list --visibility all --all
hamstik --json --no-input --org <ORG> report view <REPORT-ID>
hamstik --json --no-input --org <ORG> report run <REPORT-ID>
hamstik --json --no-input --org <ORG> dashboard list --visibility all --all
hamstik --json --no-input --org <ORG> dashboard run <DASHBOARD-ID>
```

Preserve server plan/App/capability/scope errors. Never infer entitlement.

Report definitions use the full Public API JSON document for create/edit. Do not invent a
reduced client-side schema.

Dashboard create/edit/delete are not available when the Public API exposes only
list/view/run. Verify with the installed CLI/OpenAPI before assuming otherwise.

## SqueakQL

Use typed validation/search/save commands and files for complex queries:

```bash
hamstik --json --no-input --org <ORG> squeakql validate --file QUERY.sqql
hamstik --no-input --org <ORG> squeakql save my-query --file QUERY.sqql
hamstik --json --no-input --org <ORG> work search --saved my-query
```

`work search --explain` forwards a server-provided plan if one exists; the CLI does not
invent a cost estimate.

The saved-query cache is local:

```bash
hamstik --json squeakql cache size
hamstik squeakql cache clear
```

## External subcommand plugins

An executable named `hamstik-<name>` on `PATH` can be invoked as:

```bash
hamstik <name> [args...]
```

Built-ins take precedence. The core CLI discovers external plugins from `PATH`; it does
not install, register, or update them.

The CLI passes resolved non-secret context/output-mode variables to plugins but strips
credentials such as `HAMSTIK_TOKEN`. A plugin that needs API access must use its own
authenticated transport.

Discover external plugins without executing them via:

```bash
hamstik --help
hamstik commands --json
```
