---
name: rust-rhai-modelica-boundary
description: Decide whether a LunCoSim feature belongs in USD, Modelica, Rhai, or Rust before implementing it, while reusing existing facts, APIs, and policy hooks.
---

# Rust, Rhai, Modelica, and USD boundary

Use this skill before adding behavior, a parser, a setting, a test, or a new
Rust dependency. The outcome is one authoritative owner and the smallest
rebuild/test surface that can express the contract.

## Read first

- [`AGENTS.md`](../../AGENTS.md) for the project contract and test boundary.
- [`00-overview.md`](../../docs/architecture/00-overview.md) and
  [`20-domain-modelica.md`](../../docs/architecture/20-domain-modelica.md) for
  the current architecture.
- [`capability-discovery`](../capability-discovery/SKILL.md) before calling a
  mechanism absent.
- [`luncosim-architecture`](../luncosim-architecture/SKILL.md) for cross-domain
  ownership and standard-USD review.

Defer to [`use-asset-library`](../use-asset-library/SKILL.md) for authored
asset placement, [`author-rhai-tool`](../author-rhai-tool/SKILL.md) for a new
Rhai tool, and [`validate-assets`](../validate-assets/SKILL.md) for authored
scene/scenario tests.

## Decision table

| Layer | Put here | Keep out |
| --- | --- | --- |
| USD | Identity, topology, hierarchy, relationships, authored transforms, variants, and standard facts such as `UsdGeom`, `UsdPhysics`, `UsdShade`, and `UsdLux`. | Runtime policy, continuous equations, and guessed topology. |
| Modelica | Continuous equations, differential/algebraic state, energy and domain math, and reusable physical networks. Reuse the Modelica/Rumoca surface before porting equations. | USD traversal, UI behavior, mission sequencing, and app policy. |
| Rust | Generic engine mechanisms, lifecycle and ownership seams, parsers/schema/composition/serialization, strict validation, and genuinely hot-path calculations such as pose or collision math. Expose typed facts, commands, and queries. | Model, Twin, tutorial, library-name, or product-specific policy that authored layers can express. |
| Rhai | High-level orchestration, use-case glue, tunable policy, sequencing, tutorial behavior, lints, verdicts, and selection among generic Rust capabilities. | Continuous dynamics, safety-critical admission, and hidden engine state. |

Math means domain equations in Modelica; Rust still owns numerical operations
that are part of a real-time engine invariant or a generic projection.

Use a policy hook when the decision is expected to change independently of the
engine: pass a small typed fact map in, require a closed result out, and keep
the Rust owner responsible for validation, safety limits, lifecycle, and
realization. A missing, faulting, or malformed hook result is a visible
diagnostic and an explicit no-op/hold as appropriate; it is never converted to
the first entity, a fabricated value, or an older behavior.

## Workflow

1. Discover before designing: search the current source, skills, registrations,
   maintained dependencies, and public query/command/event surfaces with
   `rg`. Identify the existing owner, reader, and test. Extend that surface
   when it is authoritative; do not add a second parser, registry, resolver,
   or fallback.
2. Separate facts from decisions. Put authored identity in USD, equations in
   Modelica, and changeable policy in Rhai. Keep Rust as the generic mechanism
   that validates facts and executes the decision. Prefer existing
   `lunco-hooks`/`lunco-hooks-rhai`, scripting `cmd`/`query`/`emit`/`on_event`,
   and existing asset tools before adding an API.
3. Minimize dependency fan-out. A new Rust edge must provide a reusable engine
   contract, not one convenience function. Split core/UI or API adapters at
   the actual dependency boundary; do not pull Bevy, rendering, physics, or a
   scripting host into a small contract crate.
4. Place tests at the observable owner. Keep Rust tests for pure lowering,
   math, parsing, schema/composition, serialization, and generic lifecycle or
   interpreter seams. Put behavior, policy, asset, long-USD, and model-backed
   tests in authored USD + Rhai production scene gates. A Rust test that loads
   a repository/Twin asset or embeds a long USDA string is a strong relocation
   signal.
5. Verify the smallest sufficient target, then review the diff for duplicate
   owners, hardcoded model/library names, `std::fs` asset access, unused
   dependencies, `#[allow(dead_code)]`, shims, aliases, and silent fallbacks.

## Existing seams to reuse

- `lunco-hooks` is the backend-neutral typed hook registry; `lunco-hooks-rhai`
  is the one Rhai adapter.
- `lunco-scripting-rhai-world` already registers authored policy files from
  `assets/scripting/policy/` and exposes the reusable world/policy surface;
  `lunco-scripting-rhai-runtime` composes it with application commands and
  persistence.
- `lunco-camera-core::DEFAULT_PRESENTATION_HOOK` is the camera example:
  Rust derives USD/ECS facts and realizes the closed decision, while the
  Rhai policy chooses `avatar`, `generated`, or `none`.
- `lunco-assets-core` and `lunco-storage` own asset resolution and storage;
  runtime/domain crates must not open asset bytes with raw filesystem paths.

## Handoff

Report: owner and why, existing capability reused, files changed, dependency
impact, focused checks, and any deliberate diagnostic/hold behavior.
