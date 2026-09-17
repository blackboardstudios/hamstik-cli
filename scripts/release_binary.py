#!/usr/bin/env python3
# Copyright 2026 Blackboard Studios
# SPDX-License-Identifier: Apache-2.0
"""Locate the release binary inside the archive `dist build` just produced.

Release builds run `dist build` (cargo-dist), not `cargo build`: the binary
exists inside the per-target archive under `target/distrib/`, and
`target/release/` may be absent or cross-compiled. The release workflow's
man-page step uses this helper to point `generate_docs.py` at the actual
released binary, so the shipped documentation is generated from the shipped
executable.

Usage:

    python3 scripts/release_binary.py --manifest dist-manifest.json

Prints the extracted binary's path on stdout, or exits 1 with a
`release_binary FAILED:` message. Stdlib only; works on Linux, macOS, and
Windows runners. Extraction logic mirrors `scripts/release_smoke.py`.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
import tarfile
import zipfile
from pathlib import Path

BINARY_NAMES = ("hamstik", "hamstik.exe")
ARCHIVE_SUFFIXES = (".tar.gz", ".tar.xz", ".zip")


def fail(message: str) -> None:
    print(f"release_binary FAILED: {message}", file=sys.stderr)
    sys.exit(1)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Extract the released binary from the dist archive."
    )
    parser.add_argument(
        "--manifest",
        required=True,
        help="path to the dist-manifest.json produced by `dist build`",
    )
    parser.add_argument(
        "--distrib-dir",
        default="target/distrib",
        help="directory holding dist build artifacts (default: target/distrib)",
    )
    return parser.parse_args(argv)


def archive_path(manifest: dict, distrib_dir: Path) -> Path:
    """Resolves the archive this build produced (exactly one per job)."""
    uploads = manifest.get("upload_files") or []
    for entry in uploads:
        path = Path(entry)
        if "".join(path.suffixes).endswith(ARCHIVE_SUFFIXES):
            path = path if path.exists() else distrib_dir / path.name
            if not path.exists():
                fail(f"archive listed in the manifest does not exist: {path}")
            return path
    fail(
        "no release archive (.tar.gz/.tar.xz/.zip) listed in the dist "
        f"manifest upload_files ({len(uploads)} files listed)"
    )
    raise AssertionError("unreachable: fail() exits")


def extract_binary(archive: Path, destination: Path) -> Path:
    try:
        if archive.name.endswith(".zip"):
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(destination)
        else:
            with tarfile.open(archive) as bundle:
                bundle.extractall(destination, filter="data")
    except Exception as error:  # noqa: BLE001 - surfaced verbatim
        fail(f"archive {archive.name} is malformed: {error}")
        raise AssertionError("unreachable: fail() exits")
    candidates = [
        path
        for path in destination.rglob("*")
        if path.is_file() and path.name in BINARY_NAMES
    ]
    if len(candidates) != 1:
        fail(
            f"expected exactly one {'/'.join(BINARY_NAMES)} in {archive.name}, "
            f"found {len(candidates)}"
        )
    binary = candidates[0]
    if os.name == "posix":
        binary.chmod(0o755)
    return binary


def load_manifest(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"manifest {path} is unreadable: {error}")
        raise AssertionError("unreachable: fail() exits")


def main() -> None:
    args = parse_args(sys.argv[1:])
    distrib_dir = Path(args.distrib_dir)
    manifest = load_manifest(Path(args.manifest))
    archive = archive_path(manifest, distrib_dir)
    # Extracted next to the archive so the caller can invoke it; the
    # previous extraction is replaced wholesale (stale binaries cannot
    # survive a fresh `dist build`, which rewrites the archive).
    work = distrib_dir / "release-binary"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
    print(extract_binary(archive, work))


if __name__ == "__main__":
    main()