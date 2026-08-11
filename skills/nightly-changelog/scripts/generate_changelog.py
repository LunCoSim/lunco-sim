#!/usr/bin/env python3
"""Generate a commit-derived LunCoSim nightly changelist."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path


TIMESTAMPED_NIGHTLY = re.compile(r"nightly-(\d{8}T\d{6}Z)$")

CATEGORIES = (
    ("feat", "Features"),
    ("fix", "Bug fixes"),
    ("refactor", "Performance & architecture"),
    ("perf", "Performance & architecture"),
    ("test", "Tests & verification"),
    ("docs", "Docs, assets & tooling"),
    ("chore", "Docs, assets & tooling"),
    ("ci", "Docs, assets & tooling"),
    ("build", "Docs, assets & tooling"),
)

CATEGORY_LABELS = tuple(dict.fromkeys(label for _, label in CATEGORIES)) + (
    "Other changes",
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
        commit_type = subject.split(":", 1)[0].lower() if ":" in subject else "other"
        category = next(
            (label for prefix, label in CATEGORIES if commit_type == prefix),
            "Other changes",
        )
        title = subject.split(":", 1)[1].strip() if ":" in subject else subject
        commits.append(
            {
                "full": full,
                "short": short,
                "date": date,
                "category": category,
                "title": title,
            }
        )
    return commits


def render(
    *,
    from_ref: str,
    to_ref: str,
    release_date: str,
    release_tag: str,
    repo: str,
    commits: list[dict[str, str]],
) -> str:
    target_sha = git("rev-parse", to_ref)
    categories = CATEGORY_LABELS
    grouped = {category: [] for category in categories}
    for commit in commits:
        grouped[commit["category"]].append(commit)

    lines = [
        f"# LunCoSim nightly changelist — {release_date}",
        "",
        "> Status: Release snapshot · Audience: nightly testers and maintainers",
        "> This file is generated from Git history; runtime and GitHub Actions results are recorded separately.",
        "",
        f"- Previous nightly: [`{from_ref}`]({repo}/releases/tag/{from_ref})",
        f"- Build target: [`{target_sha[:12]}`]({repo}/commit/{target_sha}) (`{release_tag}`)",
        f"- Source range: [{from_ref}…{to_ref}]({repo}/compare/{from_ref}...{to_ref})",
        f"- Included commits: **{len(commits)} non-merge commits**",
        "",
        "## Download",
        "",
        "Download exactly one installer for your computer:",
        "",
        "- **Windows x86_64:** `LunCoSim-Windows-x86_64-Setup.exe`",
        "- **macOS Apple Silicon:** `LunCoSim-macOS-Apple-Silicon.pkg`",
        "- **macOS Intel:** `LunCoSim-macOS-Intel.pkg`",
        "- **Linux x86_64:** `LunCoSim-Linux-x86_64.AppImage`",
        "",
        "GitHub also shows two automatic source archives; they are not desktop installers.",
        "",
        "## Summary",
        "",
        *[
            f"- {category}: **{len(grouped[category])}** commit(s)"
            for category in categories
            if grouped[category]
        ],
        "",
        "## Changes",
        "",
    ]

    for category in categories:
        entries = grouped[category]
        if not entries:
            continue
        lines.extend([f"### {category}", ""])
        for commit in entries:
            lines.append(
                f"- {commit['title']} ([`{commit['short']}`]({repo}/commit/{commit['full']}), {commit['date']})"
            )
        lines.append("")

    if not commits:
        lines.extend(["No committed changes were found in the selected range.", ""])

    lines.extend(
        [
            "## Verification status",
            "",
            "- Commit range is mechanically verified by Git history.",
            "- Runtime, scene/API, package, artifact-upload, and release verification must be added after those checks actually run.",
            "- Uncommitted work in a local checkout is not included in this changelist or in a clean GitHub Actions checkout.",
            "",
            "## Worktree boundary",
            "",
            f"- This snapshot covers only commits in `{from_ref}..{to_ref}`; local edits outside that range are unshipped.",
            "- Do not infer runtime behavior, package success, or release publication from the commit list.",
            "",
            "These unsigned development builds are intended for testing. macOS may require `xattr -cr .`; Windows may require **More info → Run anyway**.",
            "",
        ]
    )
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
    parser.add_argument("--release-tag", required=True, help="nightly release tag")
    parser.add_argument("--repo-url", help="GitHub repository URL, inferred from origin")
    parser.add_argument("--output", type=Path, help="write Markdown here instead of stdout")
    args = parser.parse_args()

    try:
        from_ref = latest_nightly() if args.from_ref == "latest" else args.from_ref
        git("rev-parse", "--verify", f"{from_ref}^{{}}")
        git("rev-parse", "--verify", f"{args.to_ref}^{{}}")
        changelog = render(
            from_ref=from_ref,
            to_ref=args.to_ref,
            release_date=args.release_date,
            release_tag=args.release_tag,
            repo=github_url(args.repo_url),
            commits=parse_commits(from_ref, args.to_ref),
        )
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(changelog + "\n", encoding="utf-8")
    else:
        print(changelog)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
