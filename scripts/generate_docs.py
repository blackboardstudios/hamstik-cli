#!/usr/bin/env python3
# Copyright 2026 Blackboard Studios
# SPDX-License-Identifier: Apache-2.0
"""Regenerate command-reference and man-page artifacts (CLI-2).

Everything is derived from `hamstik commands --json`, which is itself
generated deterministically from the authoritative Clap command tree. There
is no hand-maintained duplication: rerun this after any command-surface
change and commit the checked-in artifacts. CI fails when they differ.

Usage:

    python3 scripts/generate_docs.py --binary target/release/hamstik \
        --out docs/reference

Artifacts:

    docs/reference/manifest.json       raw command manifest
    docs/reference/<command>.md        per-command reference (for nested
                                       commands, paths use '-' separators)
    docs/man/hamstik.1                 the root man page
    docs/man/hamstik-<cmd>.1           one man page per subcommand
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

ROFF_ESCAPES = str.maketrans({"-": r"\-", "\\": r"\\"})


def fail(message: str) -> None:
    print(f"generate_docs FAILED: {message}", file=sys.stderr)
    sys.exit(1)


def run_manifest(binary: Path) -> dict:
    completed = subprocess.run(
        [str(binary), "commands", "--json"],
        capture_output=True,
        text=True,
        timeout=60,
    )
    if completed.returncode != 0:
        fail(f"`{binary} commands --json` exited {completed.returncode}: {completed.stderr}")
        raise SystemExit(1)  # unreachable; fail() exits
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError as error:
        fail(f"manifest is not valid JSON: {error}")


def command_slug(command_path: str) -> str:
    """`hamstik work attachment list` -> `work-attachment-list`."""
    return "-".join(command_path.split()[1:]) or "hamstik"


def render_reference(command: dict) -> str:
    """Markdown reference for one command."""
    lines: list[str] = []
    name = command["command"]
    title = name[len("hamstik ") :].strip() or "hamstik"
    lines.append(f"# `{name}`")
    lines.append("")
    about = command.get("about", "")
    if about:
        lines.append(about)
        lines.append("")
    if command.get("longAbout"):
        lines.append(command["longAbout"])
        lines.append("")
    if command.get("hasSubcommands"):
        lines.append("This command has subcommands; see their own pages.")
        lines.append("")
    aliases = command.get("aliases") or []
    if aliases:
        lines.append(f"Aliases: {', '.join(f'`{alias}`' for alias in aliases)}")
        lines.append("")
    for argument in command.get("arguments") or []:
        kind = argument.get("kind")
        if kind == "positional":
            label = f"`{argument.get('name', '')}`"
        else:
            label = argument.get("long") or ""
            if argument.get("short"):
                label = f"`{argument['short']}`, {label}" if label else f"`{argument['short']}`"
            label = f"`{label}`" if label else label
        if not label:
            continue
        lines.append(f"### {label}")
        lines.append("")
        help_text = argument.get("help", "")
        if help_text:
            lines.append(help_text.translate(ROFF_ESCAPES))
            lines.append("")
        if argument.get("valueName"):
            lines.append(f"Value: `{argument['valueName']}`")
            lines.append("")
        if argument.get("choices"):
            lines.append("Choices: " + ", ".join(f"`{c}`" for c in argument["choices"]))
            lines.append("")
        if argument.get("default"):
            lines.append(f"Default: `{argument['default']}`")
            lines.append("")
    capabilities = command.get("capabilities") or {}
    lines.append("")
    lines.append(
        "Supports: "
        + ", ".join(
            flag
            for flag, supported in (
                ("`--json`", capabilities.get("json")),
                ("`--no-input`", capabilities.get("noInput")),
            )
            if supported
        )
    )
    lines.append("")
    return "\n".join(lines)


def render_manpage(command: dict) -> str:
    """roff man page for one command."""
    name = command["command"]
    title = name.upper().replace("-", " ")
    about = command.get("about", "").translate(ROFF_ESCAPES)
    lines: list[str] = [f'.TH HAMSTIK 1 "{title}"', ".SH NAME", f"{name} \\- {about}"]
    lines.append(".SH SYNOPSIS")
    synopsis = name
    if command.get("hasSubcommands"):
        synopsis += " <COMMAND>"
    else:
        synopsis += " [OPTIONS]"
    lines.append(f".B {synopsis}")
    lines.append(".SH DESCRIPTION")
    lines.append(about)
    aliases = command.get("aliases") or []
    if aliases:
        lines.append(".SH ALIASES")
        lines.append(", ".join(aliases))
    arguments = command.get("arguments") or []
    options = [a for a in arguments if a.get("kind") == "option"]
    positionals = [a for a in arguments if a.get("kind") == "positional"]
    if positionals:
        lines.append(".SH ARGUMENTS")
        for argument in positionals:
            lines.append(f'.TP\n.I {argument.get("valueName") or argument.get("name")}\n{argument.get("help", "")}')
    if options:
        lines.append(".SH OPTIONS")
        for argument in options:
            label = argument.get("long") or ""
            if argument.get("short"):
                label = f"{argument['short']}, {label}"
            lines.append(f'.TP\n.B {label}\n{argument.get("help", "")}')
            if argument.get("choices"):
                lines.append(
                    "Choices: "
                    + ", ".join(choice for choice in argument["choices"])
                )
            if argument.get("default"):
                lines.append(f"Default: {argument['default']}")
    subcommands = [
        c["command"]
        for c in command.get("subcommands", [])
    ]
    if subcommands_hint := (command.get("hasSubcommands") and "See subcommand pages for details."):
        lines.append(f".SH SUBCOMMANDS\n{subcommands_hint}")
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    import argparse

    parser = argparse.ArgumentParser(
        description="Regenerate command-reference and man-page artifacts."
    )
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/hamstik"),
        help="path to the hamstik binary (default: target/release/hamstik)",
    )
    parser.add_argument(
        "--out", type=Path, default=Path("docs/reference"),
        help="output root (default: docs/reference; man pages land in docs/man)",
    )
    options = parser.parse_args(argv)

    if not options.binary.is_file():
        fail(f"binary not found: {options.binary}; build it first (cargo build --release)")

    document = run_manifest(options.binary)
    reference_dir = options.out
    man_dir = reference_dir.parent / "man"
    reference_dir = reference_dir
    reference_dir.mkdir(parents=True, exist_ok=True)
    man_dir.mkdir(parents=True, exist_ok=True)

    count = 0
    for command in document["commands"]:
        slug = command_slug(command["command"])
        (reference_dir / f"{slug}.md").write_text(
            render_reference(command), encoding="utf-8"
        )
        (man_dir / f"hamstik-{slug}.1" if slug != "hamstik" else man_dir / "hamstik.1").write_text(
            render_manpage(command), encoding="utf-8"
        )
        count += 1
    (reference_dir / "manifest.json").write_text(
        json.dumps(document, indent=1, sort_keys=False) + "\n", encoding="utf-8"
    )
    print(f"generated {count} reference pages and man pages in {reference_dir} / {man_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())