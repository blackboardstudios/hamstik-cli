#!/usr/bin/env python3
"""Classify and verify the shell examples in README.md (CLI-16).

The harness parses the README structurally (fenced code blocks with their
info string), classifies every shell block, and verifies that each executable
`hamstik` line parses against the real binary — offline, with no credentials:

- executable blocks are parsed with the real argument parser
  (`--help`-compatible offline parse); flags, subcommands, and enum values
  must exist;
- CI-secret blocks (`$HAMSTIK_PAT` substitution) are prose-only and exempt
  from execution, but their static `hamstik` lines are still validated;
- `cargo`/`git` blocks belong to the repository toolchain gates and are not
  duplicated here.

Mutating examples are verified parse-only by design: they must run against
mocks/fixtures — which the Rust integration tests already do — and never
against a real service from this harness. No network access and no
credentials are involved.

Exit 0 when every classified line passes; exit 1 with a per-line report
otherwise. Usage:

    python3 scripts/readme_examples.py [--verbose]
"""

from __future__ import annotations

import argparse
import re
import shlex
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
README = REPO_ROOT / "README.md"

FENCE_PATTERN = re.compile(r"^```([\w+-]*)\s*$")
INLINE_COMMENT_PATTERN = re.compile(r"\s#\s")


@dataclass(frozen=True)
class Block:
    """One fenced README code block."""

    index: int
    line: int
    language: str
    content: str


def extract_blocks(text: str) -> list[Block]:
    """Extracts fenced blocks with their starting line (1-based)."""
    blocks: list[Block] = []
    language: str | None = None
    start = 0
    lines: list[str] = []
    index = 0
    for line_number, line in enumerate(text.splitlines(), start=1):
        fence = FENCE_PATTERN.fullmatch(line.strip())
        if fence and language is None:
            language = fence.group(1) or ""
            start = line_number
            lines = []
            index += 1
        elif fence and language is not None:
            if language in ("bash", "sh", "shell"):
                blocks.append(
                    Block(
                        index=index,
                        line=start + 1,
                        language=language,
                        content="\n".join(lines),
                    )
                )
            language = None
            lines = []
        elif language is not None:
            lines.append(line)
    return blocks


def logical_commands(block: Block) -> list[tuple[int, list[str]]]:
    """The executable `hamstik` commands with their README line numbers.

    Backslash continuations are joined; comment lines are skipped; inline
    comments (a ` # ` outside quotes) are stripped with shell-aware tokenizing.
    """
    results: list[tuple[int, str]] = []
    pending: list[str] = []
    pending_line = 0
    for offset, raw in enumerate(block.content.splitlines()):
        line_number = block.line + offset
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if not pending:
            pending_line = line_number
        pending.append(stripped)
        if stripped.endswith("\\"):
            continue
        joined = " ".join(part[:-1].rstrip() if part.endswith("\\") else part for part in pending)
        pending = []
        try:
            tokens = shlex.split(joined)
        except ValueError as error:
            results.append((pending_line, f"__unparseable__:{error}"))
            continue
        # Drop trailing inline comments the shell would strip anyway.
        while any(" #" in token or token == "#" for token in tokens):
            for position, token in enumerate(tokens):
                if token == "#":
                    tokens = tokens[:position]
                    break
                if token.startswith("#"):
                    tokens = tokens[:position]
                    break
                if " #" in token:
                    head = token.split(" #", 1)[0]
                    tokens = tokens[:position] + ([head] if head else [])
                    break
            else:
                break
        if not tokens:
            continue
        # The leading token is the program name (`hamstik`); the remainder is
        # the argument vector handed to the real parser.
        program, arguments = tokens[0], tokens[1:]
        if program != "hamstik":
            continue  # cargo/git/printf lines belong to other checks
        results.append((pending_line, arguments))
    if pending:
        results.append((pending_line, "__unparseable__:unterminated continuation at end of block"))
    return results


def parse_check(binary: Path, arguments: list[str]) -> str | None:
    """Returns an error message when the argument vector does not parse."""
    try:
        completed = subprocess.run(
            [str(binary), *arguments, "--help"],
            capture_output=True,
            text=True,
            timeout=60,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return f"could not execute: {error}"
    if completed.returncode != 0:
        stderr_lines = completed.stderr.strip().splitlines()
        detail = stderr_lines[0] if stderr_lines else f"exit {completed.returncode}"
        return f"does not parse: {detail}"
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verbose", action="store_true", help="list every checked line")
    options = parser.parse_args()

    if not README.is_file():
        print(f"README.md not found at {README}", file=sys.stderr)
        return 1
    text = README.read_text(encoding="utf-8")
    blocks = extract_blocks(text)

    binary = REPO_ROOT / "target" / "release" / "hamstik"
    if not binary.is_file():
        debug = REPO_ROOT / "target" / "debug" / "hamstik"
        if debug.is_file():
            binary = debug
        else:
            print("no hamstik binary found; run `cargo build --release` first", file=sys.stderr)
            return 1

    failures: list[str] = []
    checked = 0
    for block in blocks:
        # Prose-only blocks embed CI-secret substitution; their hamstik lines
        # are still verified, so classification only skips execution.
        for line_number, joined in logical_commands(block):
            checked += 1
            if isinstance(joined, str) and joined.startswith("__unparseable__:"):
                reason = joined.split(":", 1)[1]
                failures.append(
                    f"README block {block.index} (line {line_number}): shell syntax error: {reason}"
                )
                continue
            error = parse_check(binary, joined)
            if options.verbose:
                status = "ok" if error is None else "FAIL"
                print(f"[{status:>4}] line {line_number}: hamstik {' '.join(joined)}")
            if error is not None:
                failures.append(
                    f"README block {block.index} (line {line_number}): `hamstik {' '.join(joined)}` {error}"
                )

    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        print(
            f"\n{len(failures)} README example(s) failed verification; fix the command "
            "or mark the block prose-only per the contributor guide.",
            file=sys.stderr,
        )
        return 1
    if options.verbose:
        print(f"\n{checked} executable README example(s) verified against {binary.name}.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())