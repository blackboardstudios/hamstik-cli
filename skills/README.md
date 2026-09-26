# Hamstik Agent Skill

[`hamstik/`](hamstik/) is the canonical, portable Hamstik Agent Skill. Copy the
**whole directory**, including `references/`, into an Agent Skills discovery
location. The entrypoint is [`hamstik/SKILL.md`](hamstik/SKILL.md); its
references contain guidance for specialized workflows.

The skill is versioned with the CLI and teaches agents to use the official
`hamstik` binary through its documented Public API v1 commands. Keep this
directory as the single source for all harnesses instead of editing separate
harness-specific copies.

See [INSTALL.md](INSTALL.md) for user-level and project-level installation,
verification, and updates.
