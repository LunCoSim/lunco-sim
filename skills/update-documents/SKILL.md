---
name: update-documents
description: >
  Update LunCoSim's canonical documentation and agent guidance when behavior,
  ownership, APIs, scenes, tutorials, validation rules, or workflows change.
  USE THIS SKILL when asked to update docs, AGENTS.md, a README, an architecture
  page, a skill, a tutorial authoring guide, or references after a code or asset
  change; when a document is stale; or when the user asks where a rule or option
  should be explained. For the agent mid-code: docs/architecture, docs/README.md,
  skills/README.md, AGENTS.md, assets/tutorials/README.md, or a request to keep
  docs and callers synchronized.
---

# Update canonical LunCoSim documents

Use this skill for documentation changes that must remain aligned with the
implemented owner. A document is a maintained interface: it must say what is
true now, where the source of truth lives, and how to verify it.

## Find the one canonical home

Read the owning source before editing prose. Then choose the narrowest home:

| Question | Canonical home |
|---|---|
| How the architecture works and why | `docs/architecture/` |
| Which crate owns a responsibility | `docs/crates-index.md` |
| How to run an application | `docs/apps/<app>/` and `docs/apps/README.md` |
| How to author tutorials or assets | `assets/tutorials/README.md` or an applicable `skills/*/SKILL.md` |
| How an agent should perform a task | `skills/<name>/SKILL.md` and `skills/README.md` |
| Repository-wide operating contract | `AGENTS.md` |
| A feature's required behavior | the relevant `specs/<nnn>-*/spec.md` |

Prefer updating an existing page over adding a second explanation. Add a new
architecture page only when the topic has no owner; add a skill only when the
task is a repeatable agent workflow rather than design rationale. Link between
homes instead of copying long sections.

## Decide what kind of edit is needed

- **Correction:** replace the stale claim everywhere it is public. Remove the
  retired contract, aliases, fallback instructions, and examples in the same
  change.
- **New behavior:** document the owner, consumer, lifecycle, and acceptance
  evidence. Update the affected skill and index when an agent must learn a new
  workflow.
- **New option:** explain the decision boundary, the exact authored fields or
  commands, and when each option is appropriate. State what must not be mixed.
- **Removal:** remove navigation, references, examples, and obsolete skill
  instructions; do not preserve a second “legacy” procedure unless the runtime
  still supports it as a deliberate contract.

For USD, distinguish authored-layer facts from composed runtime behavior. For a
tutorial, document the curriculum prim, script, payload, lifecycle owner, and
whether the lesson uses a fixed-light world, an explicit ephemeris, an existing
world, or no payload. Do not make Rust or a script own a scene fact that USD
already authors.

## Write a useful skill

Use imperative instructions and keep `SKILL.md` under 500 lines. Its frontmatter
description is the trigger, so include user-facing phrases and mid-code signals
there. The body should be a runbook:

1. identify the owner and read the relevant architecture/source files;
2. choose among the valid options and state the reason;
3. make the smallest authoritative edit, updating callers and docs together;
4. verify with the project command that proves the behavior, not only parsing;
5. report exact paths, results, limits, and any blocker.

Put detailed, conditional material in one-level `references/` files only when the
body would otherwise become unwieldy. Do not create auxiliary README,
installation, changelog, or quick-reference files inside a skill.

When creating a new skill in this repository, create its directory under
`skills/`, give it a lowercase hyphenated name, add it to `skills/README.md`,
and add it to the relevant `docs/README.md` skill table if that table lists the
category. Generate `agents/openai.yaml` with the skill-creator helper when the
repository skill is intended for UI discovery.

## Verify the document change

Search for stale names and duplicate contracts:

```sh
rg -n "old_name|old command|retired field" AGENTS.md docs skills assets
git diff --check
```

For code or authored assets, run the owning focused test and the production
scene/API gate. Use `target/debug/luncosim --validate` for USD/Rhai preflight,
but do not call it runtime proof. For tutorial behavior, use an authored Rhai
observer under `assets/scenarios/tests/` and run the production scene-test
binary; keep Rust tests generic to seams that Rhai cannot observe.

Before handoff, review the complete diff for stale links, duplicated explanations,
unmarked design-vs-built claims, and instructions that preserve an obsolete
fallback. Say explicitly when a full runtime or packaged verification was not
run.
