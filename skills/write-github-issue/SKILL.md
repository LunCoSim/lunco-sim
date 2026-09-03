---
name: write-github-issue
description: >
  Draft, open, or review a GitHub issue on lunco-sim — file a bug, write up a
  feature idea, turn a chat message or voice note into an issue, propose a new
  dependency or subsystem, or judge whether an already-open issue is worth
  working on. Trap: drafting from general knowledge of a crate or technique
  instead of checking whether it's already built here, and posting
  AI-generated text that hasn't been checked against this codebase.
---

# Write a GitHub issue

## 1. Search before writing a word

Never draft issue text from general knowledge of a library, technique, or
"how this is usually done." First:

```bash
rg -i "<topic keywords>" crates docs specs   # exclude target/
```

Also check [`docs/architecture/README.md`](../../docs/architecture/README.md)
(index of design docs) and [`docs/crates-index.md`](../../docs/crates-index.md)
(what each crate owns). Most "missing feature" ideas already have an
implementation, a partial implementation, or a doc explaining why it was
rejected. Assume the same until search says otherwise.

If the topic is already implemented: say so, point at the file or doc, and
don't draft an issue. That's the useful outcome, not a failure to produce one.

## 2. Shape the draft

- **Bug** — behaviour differs from what code/docs say it should. Include a
  reproduction: exact commands, scene/scenario path, observed vs expected.
- **Task** — one concrete, already-decided change, small enough for one PR.
  State the acceptance criterion: a test name or an observable behaviour.
- **Proposal** — a new dependency, subsystem, or architectural direction. Keep
  it SHORT (problem, what it touches here, options considered, cost/risk,
  first executable slice) and point at opening a PR that adds
  `docs/architecture/NN-<slug>.md` instead of settling the architecture in the
  issue body. Find the next free number:

  ```bash
  ls docs/architecture/ | grep -E '^[0-9]' | sort -n | tail -3
  ```

If the draft is a general tutorial, a how-to guide for a crate, or generic
advice with no repo reference and no acceptance criterion — it's none of the
three shapes. Compress it to what's actually new here, or suggest it as a
chat message instead.

## 3. Cite a real file

Point at least one real path in this repo — a crate, module, test, or doc the
issue concerns. Not a submission gate — a blank or free-text issue is fine —
but the single biggest thing that gets an issue picked up instead of skipped.

## 4. If the source is AI-drafted, strip these before posting

- delete every section that duplicates something already in the codebase
  (found in step 1) — keep only what's actually new or actually wrong;
- replace generic examples/paths with this repo's real crate, module, and
  file names;
- check version claims against `Cargo.toml` — this project is frequently
  ahead of a dependency's latest release and tracks git branches on purpose,
  so a generic version table is often already stale here;
- drop day/week delivery estimates for work nobody has started;
- if what survives is one paragraph, post one paragraph.

## 5. Reviewing someone else's issue

Apply steps 1–4 in reverse rather than redoing the author's work: does it cite
a file, does search turn up that it's already built, is it actually a
proposal that belongs in `docs/architecture/` instead. Report which checks
fail; that's enough to recommend close, keep, or reframe.
