#!/usr/bin/env python3
# Copyright 2026 Blackboard Studios
# SPDX-License-Identifier: Apache-2.0
"""Smoke-test release archives produced by cargo-dist (CLI-27).

Runs the *released artifact itself* — the binary unpacked from the archive a
user would download — and checks that it starts, reports the release version,
and passes an offline-safe diagnostic. This catches: binaries that fail to
start, archives shipping the wrong executable, dynamic-runtime
incompatibilities, version/tag mismatches, and malformed packaging.

Usage (also wired into .github/workflows/release.yml after `dist build`):

    python3 scripts/release_smoke.py --manifest dist-manifest.json

The manifest is the JSON emitted by `dist build` (cargo-dist). Every
`.tar.*`/`.zip` file it lists under `upload_files` is extracted to a
temporary directory and exercised. Exits 0 when every check passes on every
archive, 1 otherwise. Stdlib only; works on Linux, macOS, and Windows
runners (`.zip` on Windows, `.tar.xz` elsewhere).
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path
from typing import Callable

BINARY_NAMES = ("hamstik", "hamstik.exe")
ARCHIVE_SUFFIXES = (".tar.xz", ".tar.gz", ".zip")
COMMAND_TIMEOUT_SECONDS = 60


class Failure(Exception):
    """A failed smoke check with a user-facing explanation."""


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Smoke-test release archives produced by cargo-dist."
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


def load_manifest(path: Path) -> dict:
    try:
        with path.open(encoding="utf-8") as handle:
            manifest = json.load(handle)
    except (OSError, json.JSONDecodeError) as error:
        raise Failure(f"cannot read dist manifest {path}: {error}") from error
    releases = manifest.get("releases") or []
    if not releases:
        raise Failure("dist manifest has no releases entry; nothing to smoke test")
    return manifest


def expected_version(manifest: dict) -> str:
    version = manifest["releases"][0].get("app_version")
    if not version:
        raise Failure("dist manifest releases[0].app_version is missing")
    return str(version)


def archive_paths(manifest: dict, distrib_dir: Path) -> list[Path]:
    """Resolves the archives this build produced, from absolute
    `upload_files` paths with a `--distrib-dir` fallback."""
    uploads = manifest.get("upload_files") or []
    archives: list[Path] = []
    for entry in uploads:
        path = Path(entry)
        if "".join(path.suffixes).endswith(ARCHIVE_SUFFIXES):
            archives.append(path if path.exists() else distrib_dir / path.name)
    if not archives:
        raise Failure(
            "no release archive (.tar.xz/.tar.gz/.zip) listed in the dist "
            f"manifest upload_files ({len(uploads)} files listed)"
        )
    for path in archives:
        if not path.exists():
            raise Failure(f"archive listed in the manifest does not exist: {path}")
    return archives


def extract_archive(archive: Path, destination: Path) -> None:
    suffix = "".join(archive.suffixes)
    try:
        if archive.name.endswith(".zip"):
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(destination)
        elif suffix.endswith((".tar.xz", ".tar.gz")):
            with tarfile.open(archive) as bundle:
                bundle.extractall(destination, filter="data")
        else:
            raise Failure(f"unsupported archive format: {archive.name}")
    except (tarfile.TarError, zipfile.BadZipFile, OSError, EOFError) as error:
        raise Failure(f"archive {archive.name} is malformed: {error}") from error


def find_binary(directory: Path) -> Path:
    candidates = [
        path
        for path in directory.rglob("*")
        if path.is_file() and path.name in BINARY_NAMES
    ]
    if not candidates:
        raise Failure(
            f"no {'/'.join(BINARY_NAMES)} executable found in the extracted archive"
        )
    if len(candidates) > 1:
        names = ", ".join(str(path) for path in candidates)
        raise Failure(f"multiple executables found in the archive: {names}")
    binary = candidates[0]
    if os.name == "posix" and not os.access(binary, os.X_OK):
        # zip archives do not carry the POSIX exec bit; restore it so the
        # check mirrors what a user must be able to do after unpacking.
        binary.chmod(0o755)
        if not os.access(binary, os.X_OK):
            raise Failure(f"extracted binary is not executable: {binary}")
    return binary


def clean_environment(home: Path) -> dict[str, str]:
    """A hermetic environment: no user Hamstik config, credentials, or
    context can influence the artifact under test."""
    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("HAMSTIK_")
    }
    env["HOME"] = str(home)
    env["USERPROFILE"] = str(home)
    return env


def run_command(binary: Path, args: list[str], env: dict[str, str]) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(
            [str(binary), *args],
            env=env,
            capture_output=True,
            timeout=COMMAND_TIMEOUT_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired as error:
        raise Failure(f"`hamstik {' '.join(args)}` timed out") from error
    except OSError as error:
        raise Failure(f"cannot execute {binary}: {error}") from error


def check(
    binary: Path,
    args: list[str],
    expect: Callable[[subprocess.CompletedProcess, str], str | None],
    description: str,
    env: dict[str, str],
) -> None:
    result = run_command(binary, args, env)
    stdout = result.stdout.decode("utf-8", errors="replace")
    failure = expect(result, stdout)
    if failure:
        detail = (result.stderr or result.stdout).decode("utf-8", errors="replace").strip()
        raise Failure(f"{description} failed ({failure}): exit={result.returncode}\n{detail}")
    print(f"  [ok] {description}")


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        manifest = load_manifest(Path(args.manifest))
        version = expected_version(manifest)
        archives = archive_paths(manifest, Path(args.distrib_dir))
        print(f"smoke-testing {len(archives)} archive(s) for hamstik {version}")

        for archive in archives:
            print(f"[archive] {archive.name}")
            workdir = Path(tempfile.mkdtemp(prefix="hamstik-smoke-"))
            try:
                extract_archive(archive, workdir)
                binary = find_binary(workdir)
                env = clean_environment(workdir / "home")
                (workdir / "home").mkdir(parents=True, exist_ok=True)

                check(
                    binary,
                    ["--help"],
                    lambda result, out: (
                        None if result.returncode == 0 and "Usage" in out else "expected exit 0 with Usage text"
                    ),
                    "hamstik --help starts",
                    env,
                )
                check(
                    binary,
                    ["version"],
                    lambda result, out: (
                        None
                        if result.returncode == 0 and f"v{version}" in out
                        else f"expected banner version v{version}"
                    ),
                    "hamstik version reports the release",
                    env,
                )
                check(
                    binary,
                    ["--json", "--no-input", "version"],
                    lambda result, out: (
                        None
                        if result.returncode == 0
                        and json.loads(out).get("version") == version
                        else f"expected JSON version {version}"
                    ),
                    "hamstik --json version matches the release version",
                    env,
                )
                check(
                    binary,
                    ["--json", "--no-input", "doctor", "--local-only"],
                    lambda result, out: (
                        None
                        if result.returncode == 0
                        and json.loads(out).get("summary", {}).get("fail", 1) == 0
                        else "expected exit 0 with zero failed checks"
                    ),
                    "hamstik doctor --local-only passes offline",
                    env,
                )
            finally:
                shutil.rmtree(workdir, ignore_errors=True)

        print(f"[ok] all artifact smoke tests passed for {len(archives)} archive(s)")
        return 0
    except Failure as error:
        print(f"smoke test FAILED: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
