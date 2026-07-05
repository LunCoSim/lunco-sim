# USD as source of truth → ECS projection

**Status:** **implemented**. Edits now flow *into*
USD (`ApplyUsdOp` → `UsdDocument` base⊕runtime layers) and *project out* to ECS via
the op-driven live-stage pipeline: `twin_projection::sync_twin_overlays` replays the
typed op onto the `CanonicalStage` (`lunco-usd-bevy/src/canonical.rs`), openusd's
change sink fires, and `live_consume::project_stage_changes` reconciles the ECS. See
[`architecture/21-domain-usd.md`](architecture/21-domain-usd.md) § "Op-driven
projection". The narrative below is retained as the design rationale.
**Scope:** make editing flow *into* USD and *project out* to ECS, instead of the
current ECS-first model where `SetObjectProperty` mutates components directly and
only a partial, lossy shadow-write reaches USD.

---

## 0. Problem statement

Today an interactive edit (Inspector slider, `SetObjectProperty` API call) mutates
**ECS components in place**. USD — the thing we treat as the authored scene — is
either not written at all, or written by a *separate* fire-and-forget observer that
only covers scalar shader params. The result:

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

## 1. The two worlds (key finding)

The codebase already contains *two* parallel scene pipelines. The good design exists
in one of them and is missing from the other.

### Document world — workbench tabs, Twins (already correct)

- Editable **`UsdDocument`** held in **`UsdDocumentRegistry`**, with **base + runtime
  layers** (`LayerId::root()` / `LayerId::runtime()`), a **`generation`** counter, and a
  **journal** (undo/redo).
- `UsdOp::SetAttribute` (`lunco-usd/src/document.rs:735`) mutates the in-memory layer
  `sdf::Data`, `commit`s (bumps `generation`), and **returns an inverse op** → undo for
  free.
- Projected into ECS by the **E1/E2 system** in `lunco-usd/src/live_projection.rs`:
  - `project_pending_live_imports` (first mount, creates `LiveDocScene { doc, generation }`),
  - `refresh_live_doc_scenes` (`live_projection.rs:217`) — on a `generation` bump it
    `classify_changes_since(...)`, applies cheap `InfoOnly` transforms in place
    (`apply_translates`, **no respawn**), and for structural changes refreshes
    `asset.reader = Arc::new(reader)` (which fires `AssetEvent::<UsdStageAsset>::Modified`)
    + `reconcile_structural` to spawn/despawn only changed subtrees.

**This is already "USD is source of truth → project to ECS."**

### Asset world — sandbox `--scene` (where editing actually happens)

- `--scene` is loaded via `LoadScene` → `asset_server` → `spawn_scene_root_with_stage`
  (`lunco-usd-sim/src/cosim.rs`). The scene becomes a **flattened `UsdStageAsset`**
  (`UsdStageAsset { reader: Arc<UsdData> }`, `lunco-usd-bevy/src/lib.rs:137`;
  `UsdData = openusd::sdf::Data`) with **no editable layers, no document, no generation,
  no undo.**
- `SetObjectProperty` lives here and mutates ECS directly.
- The only re-projection is **hand-written per domain** (`refresh_layered_terrain_layers`,
  `lunco-sandbox/src/lib.rs:621`) reacting to `AssetEvent::Modified`.

The two worlds never meet: `persist_property_to_runtime_layer`
(`lunco-sandbox-edit/src/commands.rs:1656`) tries to bridge by authoring
`UsdOp::SetAttribute` on `LayerId::runtime()`, **but** it only fires for scalar shader
params on prims an **open document owns** — in the pure sandbox there is no document,
so it is a no-op.

---

## 2. What `SetObjectProperty` does today (review)

`on_set_object_property` (`lunco-sandbox-edit/src/commands.rs:1933`) — resolves
`entity_id → Entity` via `ApiEntityRegistry`, then branches on `property`:

| Property | Mutation | Authors USD? |
|---|---|---|
| `drive_torque`, `brake_torque`, `friction_mu`, `wheel_radius`, `spring_k`, … | `WheelRaycast` fields (`wheel_param_setter`) | no |
| `shader` | swaps `MeshMaterial3d<ShaderMaterial>` asset | no |
| `visible` | sets `Visibility` (Hidden/Visible) | no |
| `base_color`, `emissive`, `metallic`, `roughness`, `reflectance`, `alpha`, `unlit`, … | live `StandardMaterial` via `apply_pbr_param` (`commands.rs:1891`) | no |
| *(fallback)* any shader-material param | live `ShaderMaterial` via `lunco_materials::apply_param` + `to_snake_case` | no |

Sibling observer `persist_property_to_runtime_layer` (`commands.rs:1656`):
- skips `shader`/`visible`; requires a **scalar float** value; requires an **active
  document** that **owns** the prim; requires `MeshMaterial3d<ShaderMaterial>` +
  `UsdPrimPath`.
- emits `UsdOp::SetAttribute { edit_target: LayerId::runtime(), path:
  <UsdPrimPath.path>, name: "primvars:<snake>", type: "float", value }`.

So: **ECS-first, with a partial, lossy, document-only shadow-write.** Colors, vectors,
PBR, visibility and wheel params never reach USD at all.

### Supporting facts

- **`UsdPrimPath { stage_handle: Handle<UsdStageAsset>, path: String }`**
  (`lunco-usd-bevy/src/lib.rs:244`) is the per-entity link back to its prim.
- Reverse lookup (prim path → entity) exists only as an **ad-hoc, per-call HashMap**
  (`lunco-usd-sim/src/cosim.rs:483`), not a maintained index.
- Reading composed attrs: `UsdDataExt` (`lunco-usd-bevy/src/usd_data.rs`) —
  `prim_children`, `prim_attribute_value::<T>`, `field`, `prim_type_name`.
- Spawn-time projection precedent: `read_authored_params` (shader.rs) enumerates
  `primvars:*` once at instantiation — **one-way, instantiation-only**, no watcher.

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

### Step 0 (foundational): make the scene a document

Route the sandbox `--scene` through `UsdDocumentRegistry` and mount it as a
`LiveDocScene` (the `PendingLiveImports` path, registered in
`lunco-usd/src/commands.rs:115`) instead of the raw-asset `spawn_scene_root_with_stage`.
This gives the sandbox the editable base/runtime layers, `generation`, journal/undo, and
the entire E1/E2 reprojection loop the workbench already has. **Everything below then
works in both worlds.** This is the largest and riskiest change (it touches scene
loading); Steps 1–4 are mechanical once it lands.

### Step 1: `SetObjectProperty` authors USD

Rewrite `on_set_object_property` to:
1. resolve `entity_id → UsdPrimPath`;
2. look up `property` in the **attribute-mapping registry** (Step 2) → `(usd_attr_name,
   usd_type, usd_value_str, edit_target)`;
3. emit `ApplyUsdOp(UsdOp::SetAttribute { … })`;
4. **drop the direct ECS mutation.**

`edit_target = LayerId::runtime()` for live tuning (Save stays base-only, matching
today's intent); a separate explicit "bake to base" promotes runtime → root. Entities
with **no `UsdPrimPath`** (transient, editor-only objects) keep a direct-ECS fallback —
not everything belongs in USD.

`persist_property_to_runtime_layer` is then **subsumed** by Step 1 (no more separate
shadow-write) and can be deleted.

### Step 2: a bidirectional attribute ↔ component registry

Generalize the existing `TerrainLayerParserRegistry` / material `ROLES` table pattern
into one `UsdAttrProjection` registry keyed by property, each entry knowing **both**
directions:

- **author:** `(property, value_str) → (usd_attr_name, usd_type, usd_value_str)`
- **project:** `(usd_attr, composed_value) → set ECS component field`

Built-in projectors:

| Domain | USD attr | Type |
|---|---|---|
| ShaderMaterial params | `primvars:<snake>` (reuse `to_snake_case`/`apply_param`) | float / color3f / color4f |
| StandardMaterial / PBR | UsdPreviewSurface inputs, or `primvars:*` | float / color3f / bool |
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

## 5. Trade-offs / decisions

1. **Author-first vs optimistic.** Pure author-first adds ~1 tick of latency
   (edit → commit → project). Start author-first (simplest, truly single-source); if a
   slider feels laggy, also apply optimistically to ECS and let the projector reconcile
   (the projection is idempotent, so this is safe).
2. **Attributes with no natural USD home** (pure runtime markers, editor-only state) —
   keep ECS-only via the `UsdPrimPath`-absent fallback. Don't force everything into USD.
3. **Step 0 is the big one.** Unifying the sandbox onto the document path is where the
   real work and risk sit; the rest is mechanical.

---

## 6. Implementation sequence (as executed)

Built in order `0 → 4 → 2 → 1 → 3`, with **material params as the first end-to-end vertical slice**
(they already had the `primvars:<snake>` convention and the `persist_*` precedent to
fold in):

1. **Step 0** — scene-as-document for `--scene` (foundational; gates everything).
2. **Step 4** — `UsdPrimIndex` resource + maintenance observers (small, independent).
3. **Step 2** — `UsdAttrProjection` registry with the material-param projector only.
4. **Step 1** — `SetObjectProperty` (material props) authors `SetAttribute`; deleted the
   material branch's direct mutation + folded in `persist_property_to_runtime_layer`.
5. **Step 3** — `project_usd_attrs_to_components` fast path; verified a material slider
   round-trips USD→ECS with no respawn and survives reload.
6. Registry extended to PBR, visibility, wheels.

---

## 7. Key references

- `lunco-sandbox-edit/src/commands.rs` — `SetObjectProperty` struct
- `lunco-sandbox-edit/src/commands.rs` — `on_set_object_property`
- `lunco-sandbox-edit/src/commands.rs` — `persist_property_to_runtime_layer`
- `lunco-usd/src/document.rs` — `UsdOp::SetAttribute` apply (commit + inverse)
- `lunco-usd/src/live_projection.rs` — `refresh_live_doc_scenes` (E1/E2 template)
- `lunco-usd/src/commands.rs` — `PendingLiveImports` / projection registration
- `lunco-usd-bevy/src/lib.rs` — `UsdStageAsset`, `UsdPrimPath`
- `lunco-usd-bevy/src/usd_data.rs` — `UsdDataExt` (read composed attrs)
- `lunco-usd-sim/src/cosim.rs` — `LoadScene` / `spawn_scene_root_with_stage`; ad-hoc prim→entity index
- `lunco-sandbox/src/lib.rs:621` — `refresh_layered_terrain_layers` (per-domain
  projection-on-`Modified` precedent)
