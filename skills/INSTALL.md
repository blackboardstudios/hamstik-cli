# Install the Hamstik Agent Skill

Install the `hamstik` CLI first. The portable skill directory contains
`SKILL.md` and three files under `references/`; keep them together.

## User-level installation

The CLI installs its bundled copy at `~/.agents/skills/hamstik/`:

```bash
hamstik agent skill install --global
hamstik agent skill check "$HOME/.agents/skills/hamstik/SKILL.md"
```

To install directly from a checkout, run this from the repository root on
macOS or Linux when no copy exists at the destination:

```bash
mkdir -p "$HOME/.agents/skills"
cp -R skills/hamstik "$HOME/.agents/skills/"
hamstik agent skill check "$HOME/.agents/skills/hamstik/SKILL.md"
```

On Windows PowerShell, from the repository root:

```powershell
New-Item -ItemType Directory -Force (Join-Path $HOME '.agents/skills') | Out-Null
Copy-Item -Recurse skills/hamstik (Join-Path $HOME '.agents/skills/')
hamstik agent skill check (Join-Path $HOME '.agents/skills/hamstik/SKILL.md')
```

`HAMSTIK_SKILL_HOME` can override the CLI installer/checker's user-level
`.agents` directory. For example, setting it to `/tmp/my-agents` installs under
`/tmp/my-agents/skills/hamstik/`.

## Project-level installation

From the project directory, `hamstik agent skill install` writes
`.agents/skills/hamstik/` for that project. Run `hamstik agent skill check`
from the same directory to validate the installed entrypoint. A project copy
takes precedence over the user-level copy for this CLI check.

## Updates and verification

Reinstall after upgrading the CLI. Reinstalling identical files is a no-op;
the CLI refuses to overwrite a modified `SKILL.md` **or reference file** unless
you pass `--force`. Review local changes before using that flag. Manual copy
commands above are for a fresh destination; preserve any customized existing
directory before replacing it.

`hamstik agent skill check` verifies the entrypoint's CLI version requirement
and command examples against the installed binary. It does not validate the
reference files or prove that a harness has loaded the skill. Confirm that your
harness discovers `name: hamstik` from this location. Remove duplicate copies
from other discovery locations after verifying the intended copy is active.
