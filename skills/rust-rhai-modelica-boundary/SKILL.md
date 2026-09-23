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

Use Rhai's standard scalar math where it already provides the operation
(`PI`, `acos`, `sqrt`, trigonometry, and finite checks). Keep the runtime's
native engineering convention at `f64`; an explicit renderer/presentation
lowering is the only normal `f32` boundary. Put shared hot vector operations
in the Rust Rhai bridge over the existing native `DVec3`/`DQuat`/transform
values: finite/validity predicates, dot/cross, clamped cosine, angle, component
access, and explicit f64 conversion. Do not rebuild vectors as arrays inside a
per-tick or per-relation loop.

At the dynamic Rhai boundary, use typed overloads and explicit Rust predicates
such as `f64_from`, `f64_only`, `array_is`, `map_is`, `string_is`,
`vec3_is_native`, and `vec3_is_valid`. String comparisons remain appropriate
for actual semantic identifiers, qualified names, paths, and enum literals;
they are not a runtime type protocol. Opaque SysML handles/AST nodes may still
need their registered domain type identity until a typed predicate exists.

For reusable CAD/mechanical checks, prefer the authored
`assets/scripting/tools/mechanical_relations.rhai` policy. It accepts resolved
native values and caller-supplied tolerances, returns residual/evidence maps,
and can be extended without a Rust rebuild. Rust should provide only the
generic numerical mechanism; SysML supplies normative intent and Rhai selects
the policy.

Numerical settings use the existing active-Twin generic settings surface.
Keep separate f64 fields for scalar, length, angle, time, and solver policy;
resolve them once at a report/solve boundary, not inside a hot loop. A missing
or integer-valued engineering tolerance is a configuration error. Never add a
Rust global epsilon or let runtime solver policy silently override a normative
SysML tolerance.

For a typed language bridge such as SysML, keep parsing, name/type resolution,
source spans, and lossless native-value conversion in Rust. Expose those
resolved types to Rhai and keep changeable project parameter checks, selectors,
limits, and verdict policy in Rhai. Do not add Rust convenience booleans such
as `is_valid_for_griffin` or project-specific dimension/range checks when the
same rule can consume the typed source value in an authored policy. A generic
API may still accept table/name selectors solely to bound transport volume;
that is distinct from encoding domain acceptance policy in the Rust source
adapter.

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
   scripting host into a small contract crate. Native immutable preparation
   uses `lunco_core_runtime::AsyncWorkAdmission`; domain owners keep their
   typed result and deterministic commit boundary instead of adding a local
   priority scheduler. Superseded work may be withdrawn while queued. A running
   job is not preempted; reject its stale result at the owner and release only
   the exact progress operation it owns. Rhai scenario cache misses prepare
   root-source ASTs through shared admission before `TimeSpineSet`. Coalesce
   identical source, asset, and runtime-revision misses into one immutable AST
   result. Buffer all currently pending results and commit them in stable actor
   order; keep exact progress holds through dependency planning, initialization,
   and the first `on_start`. Cache hits skip worker dispatch but use the same
   activation boundary. If activation runs from paused `Update`, dependency
   planning, initialization, and `on_start` retain `Simulation` context at the
   current tick; discrete event hooks retain `Lifecycle` context. Imported
   modules remain on the synchronous resolver until their complete source/tool
   graph can be captured immutably. Native admission does
   not serve wasm; use an explicit Web Worker path instead of compiling on the
   browser main thread.
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
