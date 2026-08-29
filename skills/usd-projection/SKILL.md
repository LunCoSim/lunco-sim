---
name: usd-projection
description: >
  Extend or diagnose LunCoSim's USD-to-ECS projection: add a supported prim or
  attribute, trace an ignored field, or fix edits that fail to persist, undo,
  replicate, or render. Use for `lunco-usd*` machinery and document-owned ECS
  state. Prefer this skill for projection internals; use build-usd-scene for
  scene authoring and luncosim-architecture for cross-domain ownership.
---

# USD → ECS projection

Before adding a field or reader branch, use
[`luncosim-architecture`](../luncosim-architecture/SKILL.md) and the
[standard-schema boundary](../../docs/architecture/clean-architecture-and-usd-standards.md).
Prefer the OpenUSD schema that owns the concept. A migration is a clean
cutover: update the authored source and all readers, delete the superseded spelling
and compatibility branch, regenerate schema artifacts, and add a negative
test. Never make the ECS projection a second source of truth.

**USD is the source of truth. The ECS is a projection of it.** Every entity you
see is a rendering of a prim. Nothing is authoritative because it is in the
world; it is in the world because it is in the document.

That single sentence generates every rule below.

## The pipeline, end to end

```
UsdOp  ──►  UsdDocumentRegistry::apply   (journals + inverts)
              │
              ▼
          openusd Stage (the live CanonicalStage, NonSend)
              │  StageSink fires → RawStageChange { resynced, info_only }
              ▼
          project_stage_changes            (lunco-usd/src/live_consume.rs)
              ├── resynced   → structural: spawn / despawn prims
              └── info_only  → attribute-only: translate, rotate, domes …
              │
              ▼
          on_usd_prim_added  (observer on `Add, UsdPrimPath`)
              │
              ▼
          instantiate_usd_prim_from_stage  (lunco-usd-bevy/src/lib.rs)
              └── match reader.type_name(path) → components
```

Two entry points feed `instantiate_usd_prim_from_stage`: the observer above (live
edits, runtime spawns) and `sync_usd_visuals` (a stage asset finishing its
load). Both go through the *same* extractor, which is why a scene loaded from
disk and a prim authored at runtime produce identical entities.

## Law 1 — every edit goes through `ApplyUsdOp`

An edit that does not lower to a `UsdOp` is absent from **save, journal, undo,
and network replication**. Route every editor and runtime mutation through the
same projection boundary.

```rust
commands.trigger(ApplyUsdOp { doc, op });                        // one op
apply_ops_as_change_set(world, doc, "Edit material", ops);       // N ops, ONE undo unit
```

Prefer `apply_ops_as_change_set` whenever an intent lowers to more than one op —
a loop of `ApplyUsdOp` journals N independent entries, and undo then peels off
one and leaves the object half-edited.

Writing an ECS component directly is legitimate **only** for state that is
genuinely not part of the document (a camera's current yaw, a hover highlight).
If a user would expect it to survive save-and-reload, it belongs in USD.

`AttachProgram { doc, spec }` is the canonical multi-op authoring intent for a
source-backed Modelica, Python, Rhai, or behaviour-tree program. It lowers the
complete `LunCoProgramAPI` child, source asset, scalar ports, defaults, and
connections through this same change-set path. A palette or script must call
that command; it must not create an ECS marker or write a parallel registry.
An empty contract is source-only and remains visibly distinct from a running
cosim participant.

## Law 2 — ask the scene root, never guess

To author a new top-level prim you need the target document *and* the parent
path. Both come from the scene root:

```rust
roots: Query<&UsdPrimPath, With<lunco_usd_bevy::UsdSceneRoot>>
let doc = scene_document_for(&backed, &asset_server, root.stage_handle.id())?;
let parent = &root.path;            // "/SandboxScene", "/World", …
```

Two failure modes this exists to prevent:

- **Counting the registry** ("there's only one document") — false. The registry
  also holds terrain and script documents.
- **Hardcoding `/World`** — the luncosim scene is rooted at `/SandboxScene`. A
  prim authored outside the mounted `defaultPrim` subtree *composes into the
  layer and is then never mounted*: it saves, it journals, and it is invisible.
  This failure is completely silent.

## Law 3 — spell it the way USD spells it

Use the real schema. `UsdLuxDomeLight` for an HDRI, `UsdPreviewSurface` for a
material, `UsdPhysics*` for physics. Before inventing anything, check whether
USD already defines it — a scene that leaves this app must still mean what it
said.

- `inputs:*` is the **UsdShade** namespace: it lives on a `Shader` prim, reached
  by `material:binding` → `outputs:surface`. A `float inputs:metallic` on a
  Sphere is not valid USD, and no DCC will read it back. Use
  `lunco_usd::material::ensure_preview_surface_ops()` — it builds the
  Material+Shader+binding for you, and it is deliberately in `lunco-usd` so
  every crate authors materials the same way.
- `primvars:displayColor` / `displayOpacity` are the *only* Gprim display
  attributes. There is no "display emissive" — **emission requires a material**.
- Genuinely new concepts get the `lunco:` vendor namespace (`lunco:dome:skybox`,
  `lunco:terrain:*`). That is the correct, spec-sanctioned way to extend USD. What
  is *not* correct is inventing a second spelling for something USD already has.

**Never add an alias to make a file load.** A tolerant reader (`inputs:roughness`
*or* `perceptual_roughness` *or* bare `roughness`) is not robustness — it is a
trap. It teaches callers the invalid spelling and hides the bug: the writer
authors garbage, the reader accepts it, and the two conceal each other until the
file opens in Houdini and the material is gone. If the wrong form is authored,
the right behaviour is for it to visibly do nothing.

## Adding support for a new prim type or attribute

1. **Read it.** Extractors use the `UsdRead` trait
   (`lunco-usd-bevy/src/read.rs`), implemented by `StageView` (the live
   composed stage). Authoring-layer reads use `UsdDataExt` separately; runtime
   extractors never switch to that source.
   - Floats: use `real` / `real_f32`, **never** `scalar::<f64>` — a `float`-
     authored value silently reads `None` through the f64 path.
   - Asset paths: `read_token` (it coerces `String`/`Token`/`AssetPath`), then
     `resolve_texture_path` to make it relative to the stage layer. Downloaded
     assets are `lunco://textures/…` (declared in a crate's `Assets.toml`).
2. **Dispatch it.** Prim types are a `match` on `reader.type_name(&path)` inside
   `instantiate_usd_prim_from_stage`. There is no registry to add to.
3. **Project it.** Insert components. Keep render-bound types out of
   `lunco-usd-bevy` — it is render-free by contract (`cargo tree -p lunco-usd-bevy
   -i wgpu` must be empty). `bevy_light` / `bevy_image` / `bevy_camera` are fine;
   `bevy_pbr` / `bevy_render` are not, and belong in `lunco-render-bevy`.
4. **Re-project it on edit.** *This is the step people forget.* A structural
   change (new prim) reconciles automatically. An **attribute-only** edit arrives
   as `info_only`, and `project_stage_changes` only handles the cases it knows —
   translate, rotate, dome lights. If you add an editable attribute and skip
   this, `SetFoo` will journal and save correctly and **nothing will move on
   screen** until reload. Add a handler in `live_consume.rs`.
5. **Author it.** Add a command that lowers to `UsdOp`s (Law 1) and register it
   with `register_commands!` — a command is only reachable from the HTTP API /
   MCP / rhai if its *type* is in the reflect registry.
6. **Test it.** Because extractors are generic, unit-test off a flattened
   `sdf::Data` parsed from a `&str` of USDA — no App, no renderer.

## Worked example

`crates/lunco-usd-bevy/src/dome.rs` (HDRI environment) is the whole checklist in
one file: standard schema (`UsdLuxDomeLight`), `lunco:` only for the two knobs
UsdLux genuinely lacks, a shared reader used by both the load path and the live-
edit path, an `info_only` refresh so runtime edits appear, a `SetDomeLight`
command that lowers to ops, and pure-function tests.

## Gotchas

- `bevy::init_asset::<A>()` is **destructive**, not idempotent — it wipes
  `Assets<A>` and swaps the allocator. Guard with `contains_resource`.
- The `CanonicalStage` is `NonSend` (openusd `Stage` is `!Send`). Read it under a
  short borrow and release it *before* mutating the world.
- `reconcile_structural_live` does nothing for a prim that exists **and** already
  has an entity — it spawns and despawns only. Refreshing an existing entity is
  your job.
