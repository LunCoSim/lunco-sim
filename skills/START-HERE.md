# Using LunCoSim skills

This is the cross-host entry point for humans and AI agents working with
LunCoSim. A skill is a portable Markdown runbook: it explains how to perform a
task, where the authoritative owner lives, and how to prove the result. It is
not a runtime plugin, a replacement for the LunCoSim API, or a claim that the
host can discover every capability automatically.

Use this page when you are unsure which skill to choose. Use
[`README.md`](README.md) for the complete catalogue and the project
[`AGENTS.md`](../AGENTS.md) for the engineering contract.

## The five-minute workflow

1. State the outcome, not an implementation guess: “I want a rover with a
   self-driving controller” or “find out whether requirements verification is
   already supported.”
2. Choose one primary skill from the routing table in
   [`README.md`](README.md). If you do not know whether a
   capability exists, start with
   [`capability-discovery`](capability-discovery/SKILL.md), not a feature-
   implementation skill.
3. Load the primary runbook explicitly when the host does not auto-discover
   skills. The portable instruction is: **use `skills/<name>/SKILL.md` as the
   primary runbook for this task**. Hosts may provide a shorter invocation such
   as `$skill-name`, but that syntax is not required by LunCoSim.
4. Follow the primary skill's “read first”, ownership, and verification rules.
   Defer to a neighbouring skill only when the primary runbook names it and its
   contract is actually needed. Do not load the whole catalogue into every
   task.
5. Hand off exact files, commands, results, limits, and the current branch or
   commit. A green parse check is not runtime or visual evidence.

The normal development cycle is:

```text
outcome -> primary skill -> owner/source -> smallest authored change
        -> validate -> production/API or visual evidence -> handoff
```

## Quick routing

The complete request-to-primary/deferred routing table is maintained in
[`README.md`](README.md), with machine-readable metadata in
[`manifest.toml`](manifest.toml). This page intentionally stays a stable
cross-host entry point instead of copying that table into a second source.

- Unknown capability or owner: [`capability-discovery`](capability-discovery/SKILL.md)
- New reusable component or scene: [`author-usd-component`](author-usd-component/SKILL.md)
- Component-by-component live Editor cycle: [`interactive-component-authoring`](interactive-component-authoring/SKILL.md)
- Vehicle or mission: [`build-vehicle`](build-vehicle/SKILL.md)
- SysML requirements and verification: [`sysml-requirements`](sysml-requirements/SKILL.md)
- Authored behavior or asset-backed tests: [`author-rhai-tests`](author-rhai-tests/SKILL.md)
- Live observation or proof: [`inspect-simulation`](inspect-simulation/SKILL.md)
- FPS, physics time, Builder stalls, or Tracy: [`performance-profiling`](performance-profiling/SKILL.md)
- Terrain albedo, shadows, or visual quality: [`render-quality`](render-quality/SKILL.md)
- MCP installation or host integration: [`mcp-integration`](mcp-integration/SKILL.md)

## Formats and development boundaries

| Format or layer | Use it for | Do not move into it |
|---|---|---|
| `twin.toml` | Twin identity and entry-point selection | Physical equations or scene policy |
| USD (`.usda`) | Parts, transforms, materials, standard physics, topology, ports, and connections | Continuous integration or mission sequencing |
| Modelica (`.mo`) | Continuous equations, state, energy, and acausal domain networks | USD traversal or tutorial policy |
| Rhai (`.rhai`) | Scenario policy, commands, sequencing, reusable tools, and authored verdicts | Continuous dynamics or hidden engine state |
| SysML/KerML (`.sysml`, `.kerml`) | Twin-owned requirements, verification cases, and system structure | Full runtime expression execution or automatic USD projection |
| Rust / Avian / Bevy | Generic engine mechanisms and projections | Product-specific policy that existing Rhai/USD/Modelica contracts can express |

Python is an optional feature, not the normal LunCoSim development path. The
standard workflow uses Rhai for scenario and verification policy and Modelica
for continuous models. Do not report Python as required merely because an
optional integration exists.

When the existing generic scripting surface is enough, a missing workflow can
usually be added as a dynamic Rhai tool and reloaded in the same production
session. Start with [`author-rhai-tool`](author-rhai-tool/SKILL.md) and inspect
the existing libraries under `assets/scripting/tools/`. Add Rust only for a
generic engine mechanism that cannot be composed from the current API. This
keeps a new tool from becoming a second owner or a model-specific builder.

## Prompt templates

These work in any MCP-compatible host and make the routing decision explicit:

```text
Use skills/build-vehicle/SKILL.md as the primary runbook. Build a rover from
the existing component library in <Twin path>. Keep policy in Rhai and report
the production test evidence.
```

```text
Use skills/capability-discovery/SKILL.md first. Determine whether LunCoSim can
<requested outcome>; search skills, docs, source, registrations, dependencies,
and the live API before saying it is unavailable. If it exists, route me to
the owning skill and show the exact entry point.
```

```text
Use skills/compose-multidomain-twin/SKILL.md as the primary runbook. Work in
<Twin path>, use the installed production binary if available, and defer to
skills/sysml-requirements/SKILL.md for the requirement registry and verdicts.
```

## The handoff contract

Every skill session should end with these fields, even when the result is a
bounded negative or an external blocker:

```text
Primary skill:
Supporting skill(s):
Authoritative owner/source:
Files changed:
Checks and runtime/visual evidence:
Known limits or blocker:
Branch/commit and next action:
```

“Not found” means not found in the searched scope, with the search recorded. It
does not mean “impossible”. A failed first command, one missing symbol, or an
empty UI search is a reason to continue capability discovery, not to invent a
duplicate API or silently add a fallback.

## Installation and host boundaries

Skills and the simulator are separate deliverables:

- In a checkout, `AGENTS.md`, this page, and [`README.md`](README.md) are the
  source of truth. The `.claude/skills` compatibility link points at this
  directory for Claude Code.
- Other MCP-compatible hosts can load the same `SKILL.md` files by path. They
  do not need Codex-specific metadata or invocation syntax. If a host supports
  automatic skill discovery, its adapter may use the frontmatter descriptions.
- A GitHub-installed `luncosim` binary does not imply that the skill files are
  installed, and installing an MCP server does not automatically install this
  repository's skills. Set `LUNCOSIM_BIN` to the installed command or absolute
  binary path as documented in [`README.md`](README.md).
- This repository currently provides compatibility links, not a promised
  one-command installer for every host. A distribution installer can compose
  MCP registration and skill installation later; do not claim that boundary
  until it is implemented and tested.

Before handoff, run the lightweight catalogue check from the repository root:

```bash
python3 scripts/validate_skills.py
```

It checks the structural contract and local links; it does not replace
capability-specific validation or a production runtime test. The Python command
is only a repository documentation check and does not enable the optional
LunCoSim Python integration.

For the first authored Twin, continue with
[`00 — Create your first Twin`](../docs/tutorials/00-create-a-twin.md). For
architecture and ownership rules, use [`AGENTS.md`](../AGENTS.md) and the
[Documentation Hub](../docs/README.md).
