#!/usr/bin/env python3
"""Generate concise notes for a manually dispatched LunCoSim Dev Build."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


TIMESTAMPED_NIGHTLY = re.compile(r"nightly-(\d{8}T\d{6}Z)$")
VERSIONED_RELEASE = re.compile(
    r"(?:v)?(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-dev\.[1-9][0-9]*)?$"
)


def git(*args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        detail = result.stderr.strip() or "git command failed"
        raise RuntimeError(detail)
    return result.stdout.strip()


def latest_release() -> str:
    refs = git(
        "for-each-ref",
        "--format=%(refname:short)%09%(creatordate:unix)",
        "refs/tags",
    ).splitlines()
    candidates: list[tuple[int, str]] = []
    for record in refs:
        tag, separator, timestamp = record.partition("\t")
        if not separator or not (
            TIMESTAMPED_NIGHTLY.fullmatch(tag) or VERSIONED_RELEASE.fullmatch(tag)
        ):
            continue
        try:
            created_at = int(timestamp)
        except ValueError:
            continue
        candidates.append((created_at, tag))
    if not candidates:
        raise RuntimeError("no previous LunCoSim release tag was found")
    return max(candidates)[1]


def github_url(explicit: str | None) -> str:
    if explicit:
        return explicit.rstrip("/")
    remote = git("config", "--get", "remote.origin.url")
    if remote.startswith("git@github.com:"):
        remote = remote.removeprefix("git@github.com:")
    elif remote.startswith("https://github.com/"):
        remote = remote.removeprefix("https://github.com/")
    else:
        raise RuntimeError(
            "remote.origin.url is not a GitHub URL; pass --repo-url explicitly"
        )
    return "https://github.com/" + remote.removesuffix(".git").rstrip("/")


def render_release_notes(*, release_date: str, changelog_url: str) -> str:
    lines = [
        f"# LunCoSim development build — {release_date}",
        "",
        "The Windows x86_64 installer is Authenticode-signed.",
        "",
        "Download and install the file for your computer:",
        "",
        "- **Windows x86_64:** `LunCoSim-Windows-x86_64.msi`",
        "- **macOS Apple Silicon:** `LunCoSim-macOS-Apple-Silicon.pkg`",
        "- **macOS Intel:** `LunCoSim-macOS-Intel.pkg`",
        "- **Linux x86_64:** `LunCoSim-Linux-x86_64.AppImage` — make it executable, then open it.",
        "",
        "For Dev installations, the built-in autoupdater can check for and download new development builds.",
        "",
        "Install and open LunCoSim, then open your favorite AI agent and paste:",
        "",
        "```text",
        "Find the installed LunCoSim application and read its bundled documentation, skills, and examples before doing anything. Ask me what mission I want, then use those resources to build and validate the mission in LunCoSim. Create the mission files and explain how to run them.",
        "```",
        "",
        f"View the [changelog]({changelog_url}).",
        "",
        "This development build is intended for testing.",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--from-ref",
        default="latest",
        help="baseline ref, or 'latest' for the newest LunCoSim release tag",
    )
    parser.add_argument("--to-ref", default="HEAD", help="commit/ref being built")
    parser.add_argument("--release-date", required=True, help="date shown in the title")
    parser.add_argument(
        "--repo-url", help="GitHub repository URL, inferred from origin"
    )
    parser.add_argument("--output", type=Path, help="write Markdown here instead of stdout")
    args = parser.parse_args()

    try:
        from_ref = latest_release() if args.from_ref == "latest" else args.from_ref
        git("rev-parse", "--verify", f"{from_ref}^{{}}")
        git("rev-parse", "--verify", f"{args.to_ref}^{{}}")
        repo = github_url(args.repo_url)
        notes = render_release_notes(
            release_date=args.release_date,
            changelog_url=f"{repo}/compare/{from_ref}...{args.to_ref}",
        )
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.output:
        args.output.write_text(notes, encoding="utf-8")
    else:
        print(notes, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
