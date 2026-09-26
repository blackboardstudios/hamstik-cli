# Install the consolidated Hamstik Agent Skill

This archive contains one portable, user-level Hamstik skill:

```text
hamstik/
├── SKILL.md
└── references/
    ├── automation.md
    ├── diagnostics.md
    └── platform-features.md
```

Install it under:

```text
~/.agents/skills/hamstik/
```

Example:

```bash
mkdir -p ~/.agents/skills
rm -rf ~/.agents/skills/hamstik
cp -a hamstik ~/.agents/skills/
```

After confirming your harness discovers this version, remove repository-local or
harness-specific duplicate Hamstik skills so that only one skill declares
`name: hamstik`.
