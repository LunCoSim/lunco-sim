# USD as source of truth → ECS projection

> Status: Design · Audience: contributors on the USD→ECS boundary
>
> **Partially implemented.** The principle holds throughout; the sections below
> marked as plan are not yet the shape of the code. For the as-built projection
> machinery see [`21-domain-usd.md`](21-domain-usd.md) and the
> [`usd-projection`](../../skills/usd-projection/SKILL.md) skill.

*Built:* the op-driven projection pipeline. An `ApplyUsdOp` edit lands in the
`UsdDocument` (base⊕runtime layers), `twin_projection::sync_twin_overlays` replays
the typed op onto the `CanonicalStage` (`lunco-usd-bevy/src/canonical.rs`), openusd's
change sink fires, and `live_consume::project_stage_changes` reconciles the ECS. See
[`21-domain-usd.md`](21-domain-usd.md) § "Op-driven
projection". Spawn / remove / reference are USD-first through this path.

*Also built — the gizmo authors USD.* Drag-end fires `TransformEntity`, whose
live leg seats the complete active-frame pose and whose persistence observer
authors translation plus rotation as one runtime-layer USD change set. A drag
therefore **survives a reload**, and Ctrl+Z goes through the Twin journal as one
compound edit.

> **Current boundary.** The gizmo and scene-edit commands author USD operations;
> they do not rely on a private in-memory undo history. If you add a new
> direct-manipulation tool, wire it to the owning USD operation — not directly to
> `Transform` — so the edit is journaled, projected, and reloadable.

*Remaining design work:* `SetObjectProperty` still mutates some ECS intent in place —
`Visibility`, `WheelRaycast`, and the appearance **intent** components `PbrLook` /
`ShaderLook` (see [`render-decoupling.md`](render-decoupling.md); the crate names
no material type, and `lunco-render-bevy` binds the intent to a real material).
The current observers persist the property classes that already have a USD reader;
the generic `UsdPrimIndex`, `UsdAttrProjection`, and
`project_usd_attrs_to_components` registry proposed below are not yet implemented.
Read §2–§5 as the design for the remaining consolidation, not as a list of
retired runtime paths.
**Scope:** make the remaining edits flow *into* USD and *project out* to ECS,
without adding another per-domain persistence or resolution mechanism.

---

## 0. Problem statement

An interactive edit (Inspector slider, `SetObjectProperty` API call) currently
updates **ECS intent in place** for immediate feedback. USD persistence is split
between the command's PBR authoring path and separate observers for shader,
visibility, and wheel properties; the split is the remaining consolidation target.
For properties without a reader, the result is:

- **Two stores that drift.** ECS holds the live truth; the `.usda` (and its runtime
  layer) holds a stale, partial copy.
- **Edits are lost on reload.** Reloading the scene re-reads USD and throws away every
  ECS-only edit (colors, PBR, visibility, wheel params, …).
- **No undo, no networking, no save** for those edits — because they never became
  authored USD operations.

We want the inverse: **USD is the single source of truth. An edit authors a USD
attribute into an edit-target layer; a projection system pushes the changed attribute
into the corresponding ECS component(s).** ECS becomes a pure, rebuildable projection
of composed USD.

---

## 1. The two projection modes (key finding)

The codebase has a document-backed projection mode and a direct asset-address mode.
They share the same scene lifecycle and `LoadScene` admission boundary; only the
document ownership available to the asset differs.

### Document world — workbench tabs, Twins (already correct)

- Editable **`UsdDocument`** held in **`UsdDocumentRegistry`**, with **base + runtime
  layers** (`LayerId::root()` / `LayerId::runtime()`), a **`generation`** counter, and a
  **journal** (undo/redo).
- `UsdOp::SetAttribute` (`lunco-usd/src/document.rs:735`) mutates the in-memory layer
  `sdf::Data`, `commit`s (bumps `generation`), and **returns an inverse op** → undo for
  free.
- Projected into ECS by `lunco-usd/src/twin_projection.rs` and
  `lunco-usd/src/live_consume.rs`: `sync_twin_overlays` publishes the composed
  `base ⊕ runtime` source and applies incremental authored changes; the live
  consumer drains the OpenUSD change sink and reconciles the ECS projection.

**This is already "USD is source of truth → project to ECS."**

### Direct asset-address mode — `LoadScene { path: "lunco://…" }`

- A direct `lunco://` load can mount a shipped asset without an editable document.
  The `UsdStageAsset` is an asset-server projection with no document generation,
  journal, or undo state.
- Startup `--scene` is different: it accepts a filesystem input, resolves and
  registers its owning root, and enters the document-first `twin://` path before
  mounting. This keeps CLI startup and `OpenFile` on the same loading sequence.
- A direct asset-address load is therefore suitable for read-only/demo content;
  edits that must persist belong to an opened Twin/document.

`SetObjectProperty` and its persistence observers live in
`lunco-scene-commands/src/commands.rs`; they author the properties for which a
document-backed prim and a reader already exist, while render-only/transient
properties remain live intent. There is no second scene loader or hidden
base-only redirect.

---

## 2. What `SetObjectProperty` does today (review)

`on_set_object_property` (`lunco-scene-commands/src/commands.rs:2228`) — resolves
`entity_id → Entity` via `ApiEntityRegistry`, then branches on `property`:

| Property | Mutation | Authors USD? |
|---|---|---|
| `brake_torque`, `friction_mu`, `wheel_radius`, `moi`, `spring_k`, … | `WheelRaycast` fields through the canonical wheel-parameter registry | yes — persisted through the USD runtime overlay |
| `shader` | swaps the `ShaderLook`'s shader | no |
| `visible` | sets `Visibility` (Hidden/Visible) | yes, when a document owns the prim |
| `base_color`, `emissive`, `metallic`, `roughness`, `ior`, `alpha`, `double_sided` | the entity's `PbrLook` via `apply_pbr_look` | **yes** — as `UsdPreviewSurface` `inputs:*` (`double_sided` → `doubleSided` on the Gprim) |
| `unlit` | the entity's `PbrLook` | no — render-only intent (overlay geometry); USD has no equivalent, by design |
| reflected shader param | the entity's `ShaderLook` values (`lunco_materials`) + the shared bound-Shader resolver | yes — typed `inputs:<snake_case>` on the bound `UsdShade.Shader` |

All five mutate **appearance intent**, never a material asset: `lunco-render-bevy`
watches `Changed<PbrLook>` / `Changed<ShaderLook>` and rebinds. `SetObjectProperty`'s
crate names no material type at all.

> **Why intent and not `Assets::get_mut(handle)`.** Materials are cached by *content*,
> so identical-looking prims **share one handle**. Reaching through the handle to
> recolour "this rock" would recolour **every rock that looked like it**.

`SetObjectProperty` owns the reflected shader-parameter case. It uses the same
WGSL schema parser as the live command, resolves the bound Shader through the
composed USD material network, preserves an existing composed declaration's
exact `typeName`, validates the literal, and emits one typed runtime-layer
`UsdOp::SetAttribute` on `inputs:<snake_case>`. The Inspector uses the same
resolver and groups multi-parameter edits into one `ApplyUsdOps` operation.
Geometry `primvars:` are not a generic parameter store; standard `UsdGeom`
primvars such as `primvars:displayColor` keep their own reader/writer contracts.

So: **ECS-immediate, with document-backed persistence for the reflected shader
parameter path.** The broader generic component projection registry below
remains the design for consolidating the other appearance and physics fields;
it must extend these existing owners rather than create another parameter
namespace.

### Supporting facts

- **`UsdPrimPath { stage_handle: Handle<UsdStageAsset>, path: String }`**
  (`lunco-usd-bevy/src/lib.rs:244`) is the per-entity link back to its prim.
- Reverse lookup (prim path → entity) exists only as an **ad-hoc, per-call HashMap**
  (`lunco-usd-sim/src/cosim.rs:483`), not a maintained index.
- Reading composed attrs: `UsdDataExt` (`lunco-usd-bevy/src/usd_data.rs`) —
  `prim_children`, `prim_attribute_value::<T>`, `field`, `prim_type_name`.
- Shader projection reads authored `inputs:*` from the composed bound Shader;
  connected inputs remain graph-owned and are not treated as local parameter
  values. The same composed declaration is used by the authoring resolver.

---

## 3. Target architecture

**Invariant:** an edit's only job is to author a USD attribute into an edit-target
layer. No system writes a component except the projector. ECS = projection of composed
USD.

```
  Inspector / API edit
          │
          ▼
   SetObjectProperty  ──(map property → usd attr via registry)──▶  ApplyUsdOp(SetAttribute{edit_target, path, name, type, value})
                                                                          │
                                                                          ▼
                                                        UsdDocument.apply  → mutate layer sdf::Data
                                                                          → commit (generation++ , journal inverse)
                                                                          │
                                                                          ▼
                                              refresh_live_doc_scenes (generation bump)
                                                 ├─ InfoOnly attr  → project to component (NO respawn)   ← fast path
                                                 └─ structural     → asset.reader=Arc::new(..) → AssetEvent::Modified → reconcile / per-domain rebuild
                                                                          │
                                                                          ▼
                                                                   ECS components
```

### Step 0 (foundational): make the scene a document — completed for startup/Twins

Route the luncosim `--scene` through the owning Twin/document and mount it as the
document-first `twin://` source. `OpenFile` uses the same root discovery and
mounting sequence. Direct `lunco://` loads remain intentionally read-only asset
mounts when no document owns them; they do not silently invent a document.

The remaining steps concern broadening USD round-trip coverage for every
`SetObjectProperty` field, not adding a second startup loader. Reflected shader
parameters already use the shared bound-Shader authoring path described below.

### Step 1: `SetObjectProperty` authors USD

For each property family that has a USD owner, `on_set_object_property`:
1. resolve `entity_id → UsdPrimPath`;
2. look up `property` in the **attribute-mapping registry** (Step 2) → `(usd_attr_name,
   usd_type, usd_value_str, edit_target)`;
3. emit `ApplyUsdOp(UsdOp::SetAttribute { … })`;
4. lets the normal USD projection update the ECS intent.

`edit_target = LayerId::runtime()` for live tuning (Save stays base-only, matching
today's intent); a separate explicit "bake to base" promotes runtime → root.
Transient editor-only objects with no `UsdPrimPath` have no USD owner and are not
addressable by this document-edit command.

The reflected shader-parameter path is owned by `SetObjectProperty` itself and
shares its destination resolver with the Inspector. Other property families
remain separate until their own authoritative USD readers and projectors are
consolidated.

### Step 2: a bidirectional attribute ↔ component registry

Generalize the existing `TerrainLayerParserRegistry` / material `ROLES` table pattern
into one `UsdAttrProjection` registry keyed by property, each entry knowing **both**
directions:

- **author:** `(property, value_str) → (usd_attr_name, usd_type, usd_value_str)`
- **project:** `(usd_attr, composed_value) → set ECS component field`

Built-in projectors:

| Domain | USD attr | Type |
|---|---|---|
| `ShaderLook.dyn_params` | bound `UsdShade.Shader` `inputs:<snake>` | reflected scalar/vector type, preserving an existing USD role and array shape |
| `PbrLook` | UsdPreviewSurface inputs, or `primvars:*` | float / color3f / bool |
| Visibility | `visibility` (USD-native) | token (`inherited`/`invisible`) |
| WheelRaycast | `lunco:wheel:<field>` (matches existing `lunco-usd-sim` convention) | float |
| Transform | `xformOp:translate` / `:orient` / `:scale` (already via `apply_translates`) | — |

Adding a new editable domain = register one entry; no edits to the projection system
(mirrors `App::add_terrain_layer`).

### Step 3: generic projection on change (the fast path)

Add `project_usd_attrs_to_components` — the generic analog of
`refresh_layered_terrain_layers`. From the change batch's **`InfoOnly`** attr paths, for
each `(prim, attr)`:
1. find the entity via the reverse index (Step 4);
2. look up the projector in the registry;
3. read the new value from the cheap `composed()` base⊕runtime merge and **set the
   component field — no reflatten, no respawn** (mirrors `apply_translates`).

This is what keeps a slider drag at frame rate. **Heavy/structural attributes** whose
bridges genuinely rebuild (e.g. terrain `density`, which re-bakes the height grid) stay
on the coarse `Modified`-driven rebuild path that already exists — the registry entry
marks an attr `structural` to opt into that path instead of the fast one.

### Step 4: a maintained prim → entity index

Promote the ad-hoc `by_path` HashMap (`lunco-usd-sim/src/cosim.rs:483`) to a resource:

```rust
#[derive(Resource, Default)]
pub struct UsdPrimIndex { pub by_path: HashMap<String, Entity> }
```

kept current by observers on `UsdPrimPath` add/remove. The projector needs O(1)
prim→entity.

---

## 4. What falls out for free

- **Undo / redo** — every edit is a journaled `UsdOp` with an inverse (already produced
  by `UsdDocument::apply`).
- **Networking & determinism** — `ApplyUsdOp` is a command; tuning replicates exactly
  like spawns/moves do today.
- **Save semantics** — runtime-layer edits persist to `<twin>/.lunco/runtime/<scene>.usda`;
  the base `.usda` changes only on an explicit promote.
- **No drift** — one store; reload can never lose an edit.

---

## 5. Current decisions

1. **Document authority.** A typed edit commits to `UsdDocument` synchronously.
   The canonical stage is a derived projection and may settle on the following
   update; document queries therefore read the document composition for paths it
   owns and use the live stage for paths supplied by external composition arcs.
2. **Attributes with no natural USD home** (pure runtime markers, editor-only state)
   remain ECS-only when no `UsdPrimPath` exists; the document edit command does not
   address state without an authored USD owner.
3. **New mappings.** A property family may become document-backed only when its
   authoritative USD owner, typed reader, projector, and production-level
   acceptance path are implemented together.

---

## 6. Key references

- `lunco-scene-commands/src/commands.rs` — `SetObjectProperty` struct and observers
- `lunco-scene-commands/src/commands.rs` — `on_set_object_property`
- `lunco-usd/src/document.rs` — `UsdOp::SetAttribute` apply (commit + inverse)
- `lunco-usd/src/twin_projection.rs` — `sync_twin_overlays` and document-backed mounts
- `lunco-usd/src/live_consume.rs` — `project_stage_changes` (E1/E2 consumer)
- `lunco-usd/src/commands.rs` — scene command admission and document registration
- `lunco-usd-bevy/src/lib.rs` — `UsdStageAsset`, `UsdPrimPath`
- `lunco-usd-bevy/src/usd_data.rs` — `UsdDataExt` (read composed attrs)
- `lunco-usd-sim/src/cosim.rs` — `LoadScene` / `spawn_scene_root_with_stage`; ad-hoc prim→entity index
- `lunco-luncosim/src/lib.rs:621` — `refresh_layered_terrain_layers` (per-domain
  projection-on-`Modified` precedent)
