# Contributing to LunCoSim

LunCoSim is built by a global community of engineers and researchers. Bug
reports, feature ideas, docs fixes, and pull requests are all welcome —
this file is the front door.

## Ways to contribute

- Report a bug or propose a feature (open an issue)
- Propose an architecture or dependency change (open an issue, see the ADR
  flow below)
- Submit a pull request — code, docs, tests, tutorials, or a skill
- Answer questions and help other users in [Discord](https://discord.gg/A6U3GdvQum)
- Want to join the core team? [Apply here](https://tally.so/r/3jX6aE)

## Get set up

Clone and build — see the README's **[Fast Track](README.md#-fast-track)** for
the exact commands for `luncosim` (the simulator) and `lunica` (the Modelica
workbench).

Before changing code, skim:

- **[`docs/principles.md`](docs/principles.md)** — the project's non-negotiable
  mandates (TDD-First, Headless-First, Tunability)
- **[`docs/architecture/README.md`](docs/architecture/README.md)** — how the
  system is designed and why
- **[`docs/crates-index.md`](docs/crates-index.md)** — what each of the 60+
  crates owns
- **[`AGENTS.md`](AGENTS.md)** — the detailed engineering contract (build
  flags, test commands, lint rules); written for AI coding agents but equally
  the reference for a human contributor

## Reporting a bug or proposing a feature

Search first — a lot of what looks missing already has an implementation and
an architecture doc behind it:

```bash
rg -i "<your topic>" crates docs specs   # exclude target/
```

An issue is far more likely to get picked up quickly if it points at a real
file — a crate, module, test, or doc — even loosely. That's a strong
recommendation, not a submission requirement: open issues however you like,
including blank ones.

Three shapes tend to cover almost everything:

| Shape | Use it for |
|---|---|
| **Bug** | Something behaves differently from what the code or a doc says it should. A reproduction helps a lot. |
| **Task** | One concrete change we've already decided to make. Say what "done" looks like. |
| **Proposal** | A change of direction: a new dependency, a new subsystem, a different architecture. Needs the problem and the alternatives, not a full implementation plan. |

A few things tend not to work well as an issue, and mostly just sit longer
before anyone acts on them:

- **A general tutorial** for a crate or technique — not a unit of work, can't
  really be "closed." A Discord message or a doc link works better.
- **An architecture decision written as a long issue body** — see the ADR
  flow below; it gets reviewed faster as a diff.
- **A delivery plan with day/week estimates** for work nobody has started —
  estimates land better after a first spike, from whoever runs it.
- **A large proposal where a small PR would do** — if you can just write the
  change, write it.

### Proposals: the ADR option

For anything that changes direction rather than fixing or adding one thing:

1. Open a **Proposal** issue, kept short: the problem, the options you see,
   why now, and what it touches in this repo.
2. If there's agreement, open a PR adding `docs/architecture/NN-<slug>.md`
   with the next free number
   (`ls docs/architecture/ | grep -E '^[0-9]' | sort -n | tail -3`) and the
   status line convention from
   [`docs/architecture/README.md`](docs/architecture/README.md). The
   decision gets reviewed there, in a diff.
3. The issue then tracks just the first executable slice — usually a build
   spike or a feature-flagged skeleton.

### If you used an AI assistant to draft

AI assistance is normal here and welcome. What costs everyone time is posting
generated text that hasn't been checked against this codebase. Before
posting: run the search above and read what already exists; replace generic
examples with this repo's actual crate/module/file names; delete every
section that's already true; check version claims against `Cargo.toml` (this
project is often ahead of a dependency's latest release and tracks git
branches on purpose). If what remains is one paragraph, post one paragraph.

If you're drafting with a coding agent, point it at
[`skills/write-github-issue/SKILL.md`](skills/write-github-issue/SKILL.md) —
it runs this same search before writing anything.

## Submitting a pull request

- Branch from `main`; conventional commit subjects (`feat:`, `fix:`,
  `refactor:`, `docs:`, `chore:`) — match the existing `git log`.
- Before pushing: `cargo fmt --all`, `cargo clippy --workspace --all-targets`,
  and the tests relevant to what you touched (`cargo test -p <crate>`). Build
  with `-j 4` and the repository `target/`; see `AGENTS.md`.
- A behaviour change needs a test. A green gate needs a negative fixture.
- If you change what an architecture doc describes, update that doc in the
  same PR. If you add or rename a crate, update `docs/crates-index.md`.
- Never report a result you did not observe.

## License

LunCoSim is Apache 2.0 (`LICENSE`, `NOTICE`). By contributing, you agree your
contribution is licensed under the same terms.

## Questions?

[Discord](https://discord.gg/A6U3GdvQum) is the fastest way to reach the
team and other contributors — cheaper than a back-and-forth in issue
comments.
