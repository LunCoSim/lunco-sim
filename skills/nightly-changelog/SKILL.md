---
name: nightly-changelog
description: Generate concise LunCoSim nightly GitHub release notes with platform downloads, installation guidance, an AI-agent mission prompt, and a changelog link. Use when preparing a nightly build, updating docs/releases Markdown, or wiring release notes into .github/workflows/nightly.yml. Preserve dirty worktrees and do not edit Rust source.
---

# Nightly Changelog

Create short, human-facing notes for the exact commit being built. Keep the committed Markdown snapshot in `docs/releases/` and use the generator in this skill for future GitHub Release bodies.

## Workflow

1. Inspect repository state without changing it:

   ```bash
   git status --short --branch
   git tag --list 'nightly-*'
   git log -1 --format='%H %cI %s' HEAD
   ```

   Preserve unrelated dirty files. The changelist describes committed changes in the release range; report uncommitted work separately and never silently include it.

2. Establish the range. By default, use the newest tag matching `nightly-YYYYMMDDTHHMMSSZ` as the base and `HEAD` as the target. For a rerun, pass explicit refs. Do not use the legacy rolling `nightly` tag as the baseline.

3. Generate the Markdown with the bundled script:

   ```bash
   python3 skills/nightly-changelog/scripts/generate_changelog.py \
     --from-ref latest \
     --to-ref HEAD \
     --release-date 2026-08-02 \
     --output docs/releases/nightly-20260802.md
   ```

   The default format emits the standalone changelog with all non-merge changes since the previous nightly. The `release-notes` format emits only platform download/install instructions, a brief autoupdater availability note, a prompt for an AI agent to use the installed documentation, skills, and examples to build a mission, and a changelog link. It does not inspect or modify source files.

4. Review the standalone changelog for the complete previous-nightly change range. Review release notes separately for correct installer names, concise wording, the brief autoupdater note, and a working changelog link. Do not add updater mechanics or claim runtime validation to the GitHub release body.

5. Add or update the release workflow only in `.github/workflows/nightly.yml`. The release job must fetch timestamped tags, generate the GitHub body with `--format release-notes` from the exact `github.sha`, and publish those notes with the artifacts. Keep the detailed changelog output in its separate `docs/releases/` snapshot; never pass that output as the GitHub release body. A local Markdown file is GitHub-visible only after the containing commit is pushed; a release body is the immediate GitHub-facing copy.

   The metadata job must produce one validated `release_tag` together with the
   shared UTC timestamp, and the release job must propagate that output
   explicitly. Fail before any `gh release create` call if the value is empty or
   begins with `untagged-`; GitHub's generated untagged draft is not a valid
   nightly release and cannot receive the Velopack upload contract.

6. Validate without Rust edits:

   ```bash
   python3 skills/nightly-changelog/scripts/generate_changelog.py --help
   quick_validate.py skills/nightly-changelog
   git diff --check
   git status --short
   ```

   Run the repository's normal CI or packaging checks only when requested. Do not claim that a nightly build, upload, tag, or release exists until its GitHub Actions result is observed.

## Output Contract

- Store committed snapshots at `docs/releases/nightly-YYYYMMDD.md`.
- Generate changelog snapshots with the default format; generate GitHub release bodies with `--format release-notes`.
- Include exactly the four supported human installer names.
- Mention that the built-in autoupdater can download the latest nightly build.
- Include the short mission-building prompt for the user's favorite AI agent.
- Include all non-merge changes since the previous timestamped nightly in the standalone changelog.
- Link the exact source range from the release body without copying its changes into that body.
- Keep updater feeds, updater mechanics, verification claims, and worktree details out of the human-facing release body.

The generator is intentionally dependency-free beyond Python 3 and Git. Keep the logic deterministic so a rerun over the same refs produces the same changelist.
