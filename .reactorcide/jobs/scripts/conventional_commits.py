#!/usr/bin/env python3
"""Reject PRs whose commit subjects don't follow Conventional Commits.

Runs as the job_command for the csilctl-conventional-commits job, in the
working directory runnerlib already checked the source out into.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path
from typing import Sequence

CONVENTIONAL_COMMIT = re.compile(
    r"^(?:BREAKING CHANGE"
    r"|(?:feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)"
    r"(?:\([^)]+\))?!?)"
    r": .+"
)


def _run(args: Sequence[str], *, cwd: Path, check: bool = True) -> subprocess.CompletedProcess[str]:
    command = tuple(args)
    print(f"+ {' '.join(command)}", flush=True)
    return subprocess.run(
        command,
        cwd=cwd,
        check=check,
        shell=False,
        text=True,
        capture_output=True,
    )


def _ensure_diff_base_fetched(root: Path, diff_base: str) -> None:
    check = subprocess.run(
        ("git", "cat-file", "-e", diff_base),
        cwd=root,
        shell=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if check.returncode == 0:
        return
    _run(("git", "fetch", "origin", diff_base), cwd=root)


def main() -> None:
    root = Path.cwd()
    diff_base = os.environ.get("REACTORCIDE_DIFF_BASE")
    if not diff_base:
        raise RuntimeError("REACTORCIDE_DIFF_BASE is required to validate PR commits")

    _ensure_diff_base_fetched(root, diff_base)

    result = _run(
        ("git", "log", f"{diff_base}..HEAD", "--pretty=format:%H%x00%s"),
        cwd=root,
    )

    failures = []
    for line in result.stdout.splitlines():
        commit_hash, _, subject = line.partition("\x00")
        if CONVENTIONAL_COMMIT.fullmatch(subject):
            print(f"OK: {subject}", flush=True)
        else:
            failures.append(f"{commit_hash[:12]} {subject}")

    if failures:
        details = "\n".join(failures)
        raise RuntimeError(
            "Commit subjects must use Conventional Commits "
            "(type(scope)?: description). Invalid commits:\n"
            f"{details}\n\n"
            "Valid types: feat, fix, docs, style, refactor, perf, test, "
            "build, ci, chore, revert\n"
            "Breaking changes: add a '!' after the type/scope "
            "(e.g. feat!: ...) or use 'BREAKING CHANGE: ...'"
        )

    print("All commits follow Conventional Commits.", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:  # noqa: BLE001
        print(f"error: {exc}", file=sys.stderr, flush=True)
        sys.exit(1)
