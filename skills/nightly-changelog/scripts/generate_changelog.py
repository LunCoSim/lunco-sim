#!/usr/bin/env python3
"""Generate a LunCoSim nightly changelog or concise release notes."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


TIMESTAMPED_NIGHTLY = re.compile(r"nightly-(\d{8}T\d{6}Z)$")


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


def latest_nightly() -> str:
    refs = git(
        "for-each-ref",
        "--format=%(refname:short)",
        "refs/tags/nightly-*",
    ).splitlines()
    candidates = [ref for ref in refs if TIMESTAMPED_NIGHTLY.fullmatch(ref)]
    if not candidates:
        raise RuntimeError("no timestamped nightly tag was found")
    return max(candidates, key=lambda ref: TIMESTAMPED_NIGHTLY.fullmatch(ref).group(1))


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


def parse_commits(from_ref: str, to_ref: str) -> list[dict[str, str]]:
    record_sep = "\x1f"
    output = git(
        "log",
        "--no-merges",
        "--reverse",
        f"--format=%H{record_sep}%h{record_sep}%ad{record_sep}%s",
        "--date=short",
        f"{from_ref}..{to_ref}",
    )
    if not output:
        return []
    commits = []
    for record in output.splitlines():
        full, short, date, subject = record.split(record_sep, 3)
        if subject.lower().startswith("merge "):
            continue
        title = subject.split(":", 1)[1].strip() if ":" in subject else subject
        commits.append(
            {"full": full, "short": short, "date": date, "title": title}
        )
    return commits


def render_changelog(
    *,
    from_ref: str,
    to_ref: str,
    release_date: str,
    repo: str,
    commits: list[dict[str, str]],
) -> str:
    lines = [
        f"# LunCoSim nightly changelog — {release_date}",
        "",
        f"- Previous nightly: [`{from_ref}`]({repo}/releases/tag/{from_ref})",
        f"- Source range: [{from_ref}…{to_ref}]({repo}/compare/{from_ref}...{to_ref})",
        f"- Included commits: **{len(commits)} non-merge commits**",
        "",
        "## Changes since previous nightly",
        "",
    ]
    for commit in commits:
        lines.append(
            f"- {commit['title']} ([`{commit['short']}`]({repo}/commit/{commit['full']}), {commit['date']})"
        )
    if not commits:
        lines.append("No committed changes were found in the selected range.")
    lines.extend(
        [
            "",
            f"View the [changelog comparison]({repo}/compare/{from_ref}...{to_ref}).",
            "",
        ]
    )
    return "\n".join(lines)


def render_release_notes(
    *,
    release_date: str,
    changelog_url: str,
) -> str:
    lines = [
        f"# LunCoSim nightly — {release_date}",
        "",
        "Download and install the file for your computer:",
        "",
        "- **Windows x86_64:** `LunCoSim-Windows-x86_64-Setup.exe`",
        "- **macOS Apple Silicon:** `LunCoSim-macOS-Apple-Silicon.pkg`",
        "- **macOS Intel:** `LunCoSim-macOS-Intel.pkg`",
        "- **Linux x86_64:** `LunCoSim-Linux-x86_64.AppImage` — make it executable, then open it.",
        "",
        "LunCoSim includes an autoupdater that checks for and can download the latest nightly build.",
        "",
        "Install and open LunCoSim, then open your favorite AI agent and paste:",
        "",
        "```text",
        "Find the installed LunCoSim application and read its bundled documentation, skills, and examples before doing anything. Ask me what mission I want, then use those resources to build and validate the mission in LunCoSim. Create the mission files and explain how to run them.",
        "```",
        "",
        f"View the [changelog]({changelog_url}).",
        "",
        "These unsigned nightly builds are intended for testing.",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--from-ref",
        default="latest",
        help="baseline ref, or 'latest' for the newest timestamped nightly tag",
    )
    parser.add_argument("--to-ref", default="HEAD", help="commit/ref being built")
    parser.add_argument("--release-date", required=True, help="date shown in the title")
    parser.add_argument(
        "--format",
        choices=("changelog", "release-notes"),
        default="changelog",
        help="output a detailed changelog or short GitHub release notes",
    )
    parser.add_argument(
        "--changelog-url",
        help="link used by release notes; defaults to the exact source comparison",
    )
    parser.add_argument("--repo-url", help="GitHub repository URL, inferred from origin")
    parser.add_argument("--output", type=Path, help="write Markdown here instead of stdout")
    args = parser.parse_args()

    try:
        from_ref = latest_nightly() if args.from_ref == "latest" else args.from_ref
        git("rev-parse", "--verify", f"{from_ref}^{{}}")
        git("rev-parse", "--verify", f"{args.to_ref}^{{}}")
        repo = github_url(args.repo_url)
        if args.format == "release-notes":
            output = render_release_notes(
                release_date=args.release_date,
                changelog_url=args.changelog_url
                or f"{repo}/compare/{from_ref}...{args.to_ref}",
            )
        else:
            output = render_changelog(
                from_ref=from_ref,
                to_ref=args.to_ref,
                release_date=args.release_date,
                repo=repo,
                commits=parse_commits(from_ref, args.to_ref),
            )
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(output + "\n", encoding="utf-8")
    else:
        print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
