---
name: capability-discovery
description: >
  Find existing LunCoSim capabilities before declaring a feature missing,
  unsupported, or impossible. Use when a request sounds like a gap, a command
  or UI action is hard to locate, a build or runtime error suggests an API is
  absent, or an agent is about to propose a new mechanism or fallback.
---

# Discover LunCoSim capabilities

Use this runbook before writing “LunCoSim cannot do X”, “X is not implemented”,
or adding a replacement mechanism. A failed first search is not evidence of
absence: the capability may be named after its owner, exposed only through a
registration, available in a skill, or implemented in a different format.

## 1. Start from the current checkout

Confirm the worktree, branch, and dirty state. Treat old reports, another
worktree, a stale binary, and a failed build as leads rather than current
capability evidence. Preserve unrelated changes.

Translate the request into several search terms: the user's phrase, the likely
architecture noun, a probable crate or schema, and a command/query/tool name.
Then search generated-output-free paths:

```bash
rg -n -i --glob '!target/**' '<terms>' \
  AGENTS.md skills docs specs crates assets scripts
rg -n -i --glob '!target/**' '<symbol|schema|command|query>' \
  skills docs crates assets scripts
```

Read only the routed material, but always begin with:

- [`skills/README.md`](../README.md) for skill routing and cross-cutting rules;
- [`docs/README.md`](../../docs/README.md) for the documentation map;
- [`docs/crates-index.md`](../../docs/crates-index.md) for ownership;
- [`docs/apps/README.md`](../../docs/apps/README.md) for binaries and launch modes;
- the relevant architecture document, specification, skill, crate README, and
  authored asset examples.

If the first vocabulary is empty, search synonyms, the owning domain, standard
USD schema/property names, and likely `Command`, query, observer, registration,
or Rhai tool names. Do not conclude from one README, one crate, or one failed
symbol lookup.

## 2. Trace the authoritative owner

For each promising result, follow the complete path:

```text
format or authored asset
  -> loader/composer/parser
  -> owner and registration
  -> public caller/API/query/command
  -> runtime projection or execution
  -> test, example, or acceptance evidence
```

Check the maintained dependency when the question concerns OpenUSD, Bevy,
Avian, Modelica/Rumoca, wgpu, or another library. Search Cargo features and
registration macros as well as function names. A source file without a
registration or caller is not the same as a usable capability; a missing name
in one crate is not proof that the capability is absent.

For live behavior, use the production binary and its API. Start a rebuilt
`target/debug/luncosim` with an explicit free `--api PORT`, then use:

```bash
curl -s http://127.0.0.1:PORT/api/commands/schema | jq .
curl -s -X POST http://127.0.0.1:PORT/api/commands \
  -H 'content-type: application/json' \
  -d '{"type":"DiscoverSchema"}' | jq .
```

Use `ListEntities`, `ScriptingCatalog`, `ListToolLibraries`,
`GetToolLibrary`, and the relevant typed query/command to verify a live surface.
`--validate` proves parsing/preflight only; it does not prove runtime behavior.
Use a production authored scene test, API verdict, or headful visual capture
when that is what the requested capability requires.

## 3. Know the format boundary

Choose the format whose owner already matches the requested fact:

| Format or layer | Use it for | Do not use it for |
|---|---|---|
| `twin.toml` | Twin identity and active default stage | equations or mission policy |
| USD (`.usda`, composed stage) | parts, identity, transforms, variants, materials, topology, typed ports, connections, and authored physics | continuous integration or scenario sequencing |
| Modelica (`.mo`) | continuous equations, state, energy balance, and solved outputs | USD traversal or mission policy |
| Rhai (`.rhai`) | events, commands, sequencing, policy, authored tests, and reusable tools | continuous dynamics or a second engine core |
| Rust crates | generic engine mechanisms, projection, scheduling, and hot paths | model-specific names, hidden policy, or a duplicate authoring API |
| WGSL | shader stages and visual computation | simulation state ownership |
| runtime HTML/CSS-like UI | Twin-authored presentation and semantic UI actions | direct mutation of domain state |
| API JSON / MCP | transport of typed commands, queries, and discovery | a second persistent domain format |

A composed USD stage is data; it does not by itself execute Modelica, Rhai,
physics, or rendering. Standard USD schemas and properties own authored facts
before a custom `lunco:` field is considered. Asset identity and storage use
the existing asset resolver and `@lunco://...@` references.

## 4. Use dynamic Rhai tools when the capability is policy

LunCoSim can gain reusable authoring, inspection, lint, and test behavior
without a Rust rebuild. Before creating a tool, query `DiscoverSchema`,
`ListToolLibraries`, and `GetToolLibrary`, then search:

- shared libraries in `assets/scripting/tools/`;
- Twin-scoped libraries in `<twin>/tools/`;
- prelude helpers and the relevant `author-rhai-tool` skill;
- `docs/scripting-guide.md` and its tool-library section.

Compose or minimally extend an existing library when possible. Use a new
library only for a reusable contract, missing generic operation composition, or
repeatable report/test boundary. Keep one owner: a tool may choose policy and
produce typed plans, but USD/document owners apply edits, Rust owns generic
engine mechanisms, and Modelica owns continuous equations. A one-off mission
belongs in a scenario, not a shared tool.

The normal dynamic-tool cycle is:

```text
edit .rhai
  -> RegisterToolLibrary { name, source }
  -> ListToolLibraries / GetToolLibrary
  -> minimal RunRhai or RunScenario call
  -> authored scene test or runtime evidence
```

Registration validates the source in the production Rhai engine and, with an
active Twin, persists it in the Twin tool directory. Tool/source changes do not
need a Rust rebuild; Rust observers, command types, schemas, and projection
changes do. Discovery is not invocation proof: verify the exact function from
the actual execution context.

## 5. Follow the normal development cycle

Use this sequence for a capability request or implementation:

1. **Frame the intent.** Define the observable result, format/domain, and
   evidence needed. Choose the primary skill from `skills/README.md`.
2. **Discover before designing.** Search skills/docs, then owner source,
   registrations/callers, maintained dependencies, assets, and live API
   surfaces. Record the existing mechanism or the bounded gap.
3. **Establish a baseline.** Reproduce the current behavior with the narrowest
   relevant test, production binary, API call, or visual capture.
4. **Change the owner.** Extend the existing mechanism in its authoritative
   layer. Keep policy in Rhai, authored identity/topology in USD, equations in
   Modelica, and generic engine work in Rust.
5. **Validate the smallest sufficient path.** Use `--validate` for asset
   preflight, the owning Rust target for low-level mechanisms, authored Rhai
   scene tests for observable behavior, and the production `luncosim` binary
   for runtime/visual evidence. Reuse valid evidence when inputs are unchanged.
6. **Review integration.** Search callers, docs, skills, registrations, tests,
   and examples for stale names. Check `git diff --check`; remove retired
   paths, shims, fallbacks, and duplicate owners.
7. **Hand off.** Report exact files/symbols, commands, revision, evidence type,
   and remaining blockers. For non-trivial LunCoSim work, keep the Trello card
   in the correct lifecycle state and do not claim acceptance without observed
   evidence.

## Bounded answers to common questions

**“I cannot find the feature. Is it missing?”**

Not yet established. Search the routed skills and docs, trace the owner and
registration, search alternate vocabulary and maintained dependencies, then
check the live API or production test when applicable. Report what was searched.

**“The API command or UI action is not in the source file I opened.”**

Use `DiscoverSchema`; commands and providers self-register and may be owned by
another plugin or adapter crate. Trace the registration and public caller before
adding a new command or UI path.

**“The docs say one thing and runtime does another.”**

Verify the current branch, build the production binary from that checkout, and
prefer current source plus observed registration/runtime evidence. Update the
canonical documentation in the same change; do not preserve two contradictory
contracts.

**“Can I add a quick Rust workaround?”**

Only if the missing piece is a generic engine mechanism that cannot be composed
through an existing authored/API surface. First check the relevant skill,
standard USD owner, existing Rhai tool, and maintained dependency. Do not add a
model-specific Rust path, compatibility alias, silent fallback, or second owner.

**“Do I need a rebuild?”**

Usually not for authored `.rhai` scenarios/tools or WGSL shader source when the
existing reload surface covers the change. A new Rust command, observer,
projection path, schema, or dependency requires a focused rebuild and a
replacement production session. Use API `Exit` before replacing a session.

**“Which file format should I use?”**

Use the ownership table above and the existing authoring skill. If the fact is
scene identity/topology, start in USD; if it is continuous state, Modelica; if
it is mission policy or an authored verdict, Rhai. Keep API JSON as transport,
not as a new source of truth.

**“Does `--validate` prove that it works?”**

No. It proves asset parsing, resolution, and authored preflight/lints. Runtime
behavior needs a production scene test, API observation, or visual evidence.

**“Where should the test go?”**

Put behavior and policy assertions in `assets/scenarios/tests/*.rhai` and run
them through the production scene-test binary. Reserve Rust tests for generic
mechanisms that the public Rhai/API surface cannot observe, such as parsing,
serialization, schema/composition, or pure lowering/math.

**“When may I say ‘impossible’?”**

Only after the relevant current owner, registrations/callers, maintained
dependency, and runtime surface have been checked and a concrete contract,
dependency, platform, or permission limit is demonstrated. Otherwise say “not
found in the searched scope”, “implemented but unwired”, “not verified”, or
“externally blocked”, with the evidence.

## Result categories

Every discovery report should use one of these bounded outcomes:

- **Found and usable:** exact path, symbol, command/query, tool, or workflow.
- **Implemented but unwired:** owner exists; name the missing registration,
  feature, caller, asset reference, or runtime integration.
- **Present elsewhere:** exact branch, commit, worktree, or version mismatch.
- **Not found in the searched scope:** list paths, terms, owner, and evidence
  checked; do not generalize to the whole product.
- **Externally blocked:** exact dependency, platform, permission, or environment
  error, plus the owner that would resolve it.

Link the relevant skill and documentation in the handoff. If discovery finds an
existing capability, switch to that capability's implementation or usage skill
instead of creating a parallel mechanism.
