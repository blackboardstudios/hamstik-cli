# `hamstik doctor`

Verify configuration, credentials, connectivity, API compatibility, and selected Organization/Project context

### `--local-only`

Check only local configuration, context, credential\-store access, terminal behavior, and bundled compatibility metadata; remote checks are marked skipped and no network traffic is generated

Choices: `true`, `false`

### `--bundle`

Write a versioned support bundle to the given path. The bundle is a ZIP containing: `bundle\-manifest.json` (layout version and file list), `doctor\-report.json` (redacted diagnostic results), `context\-explain.json` (redacted context resolution chains), `cli\-info.json` (version, target, build profile), `api\-compatibility.json` (required/additive operation counts), and `config\-metadata.json` (safe config summary, no secrets)

### `--diff`

Compare a freshly generated local\-only support bundle against a previously saved bundle and report only what changed. The named diff covers the redacted report contents: host, CLI version, context scopes, checks, and config metadata. `\-\-diff` implies `\-\-local\-only`, so no network traffic is generated and no new sensitive data is read; an unchanged bundle reports an explicit "no differences" result


Supports: `--json`, `--no-input`
