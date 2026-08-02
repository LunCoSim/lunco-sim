---
name: nightly-changelog
description: Generate a traceable LunCoSim nightly changelist and GitHub release notes from the commits after the latest timestamped nightly tag. Use when preparing a nightly build, documenting changes and bug fixes since the previous nightly, updating docs/releases Markdown, or wiring changelists into .github/workflows/nightly.yml. Preserve dirty worktrees and do not edit Rust source.
---

# Nightly Changelog

Create one auditable changelist from a published nightly tag to the exact commit being built. Keep the committed Markdown snapshot in `docs/releases/` and use the generator in this skill for future GitHub release notes.

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
     --release-tag nightly-20260802T000000Z \
     --output docs/releases/nightly-20260802.md
   ```

   The script groups non-merge commits by their conventional type, links every item to GitHub, records the exact source range, and includes install instructions suitable for a GitHub Release. It does not inspect or modify source files.

4. Review the generated list against `git log <base>..<target>`. Keep commit wording factual. Add a short hand-written highlights section only when it is supported by the linked commits; do not claim runtime validation from commit subjects.

5. Add or update the release workflow only in `.github/workflows/nightly.yml`. The release job must fetch timestamped tags, generate notes from the exact `github.sha`, and publish those notes with the artifacts. A local Markdown file is GitHub-visible only after the containing commit is pushed; a release body is the immediate GitHub-facing copy.

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
- State the previous nightly tag, target commit, source range, and generated commit count.
- Group changes under `Features`, `Bug fixes`, `Performance & architecture`, `Tests & verification`, and `Docs, assets & tooling`.
- Omit merge commits from item lists to avoid duplicate release notes; retain the merge-inclusive range in the compare link.
- Keep local uncommitted work out of the release range and explicitly identify it as unshipped.
- Separate generated change evidence from runtime/API/Actions verification.

The generator is intentionally dependency-free beyond Python 3 and Git. Keep the logic deterministic so a rerun over the same refs produces the same changelist.
