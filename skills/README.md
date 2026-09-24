# Hamstik Agent Skill

The canonical Hamstik Agent Skill lives in [`hamstik/SKILL.md`](hamstik/SKILL.md).
It is versioned with the CLI so its command, output, concurrency, and safety guidance
can remain synchronized with the implementation.

The skill teaches coding agents to use the official `hamstik` CLI—not to duplicate the
Hamstik REST API. It covers safe authentication and context discovery, Organization
Attributes, reading Work Items, concurrency-aware mutations, transitions, comments,
bulk operations, structured output, and failure handling.

An automatic skill installer remains future work. Until it exists, agent harnesses can
load this canonical file directly or copy the `skills/hamstik/` directory into their
supported project/global skills location. Do not maintain independently edited
harness-specific copies.
