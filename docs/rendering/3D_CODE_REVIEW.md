# 3D / Rendering Code Review — lunco

Bevy 0.18 + egui lunar simulator. Big-space floating origin, lunar-surface scale, native + WASM.
Scope: shaders (`assets/shaders/`), material system (`lunco-materials`), lighting / terrain / horizon
shadows (`lunco-usd-bevy`, `lunco-environment`, `lunco-celestial`), camera clip planes (`lunco-avatar`).

All findings below were adversarially verified; severities here are the **corrected** post-verification
values. Rejected findings have already been dropped. Where the original severity was lowered, the verifier
note explains why.

---

## 1. TL;DR

**Counts (corrected severity):**

| Severity | Count | IDs |
|---|---|---|
| High | 0 | — |
| Medium | 8 | SHD-1, CPU-1, SHA-2, LIG-2, LIG-4, LIG-5, WASM-1, ARC-1 |
| Low | 13 | SHA-3, SHA-4, MAT-1, MAT-2, MAT-3, MAT-5, MAT-6, CPU-2, CPU-3, CPU-4, ARC-2, ARC-3, LIG-3 |

*(21 findings kept of 27 raised; 6 rejected in verification.)*

*Note:* LIG-3 was lowered to **low** by the verifier (documented tuning knob, mitigated by the heightfield march
handoff). The medium tier is: **SHA-2, SHD-1, LIG-2, LIG-4, LIG-5, WASM-1, ARC-1, CPU-1**. No item survived
verification at "high" — the two original highs (SHD-1, LIG-2) were both corrected down to medium, and the original
high CPU-1 to medium.

**Top 5 must-fix (impact × likelihood × cheapness of fix):**

1. **SHD-1** — `pack()` panics (index OOB) on any `Material` struct >256 bytes. Latent crash on the
   documented hot-reload / shader-discovery path. *Trivial guard, big blast radius.*
2. **CPU-1 / CPU-2** — `shade_dynamic_entities` re-marches every entity against every terrain at uncapped
   render FPS while the sun moves, recomputing a per-terrain affine inverse inside the per-entity loop.
   *Main-thread frame-rate hit during the exact moment shadows change.*
3. **LIG-2** — far plane hard-pinned to `1e15` every frame, negating the adaptive-near intent and forcing a
   worst-case depth ratio even when the visible scene is metres deep. *One-line + a small scene-extent calc.*
4. **LIG-5** — fallback Sun light (no UsdLux authored) uses Bevy default cascades/biases/2048 map instead of
   the tuned 1500 m / 0.06 / 2.5 / 4096 USD setup → discontinuous, acne-prone shadows. *Factor a shared helper.*
5. **WASM-1 + ARC-2** — terrain ray-march shadows silently absent on web, and the native-only shader picker
   silently omits on-disk shaders on web, both with only a buried log. *Surface the degradation in the UI.*

---

## 2. Medium findings

### SHD-1 — `pack()` panics (index out of bounds) on a `Material` struct >256 bytes
- **File:** `crates/lunco-materials/src/dyn_params.rs:186-194` (pack), `227-232` (parse warn)
- **Category:** bug / latent crash · **Confidence:** high
- **Problem:** `ParamSchema::parse()` detects `size > BLOCK_BYTES` (256) and emits only
  `warn!("...extra fields will be clipped")` — it never actually clips: it returns `Some(ParamSchema{ fields, size })`
  with all fields intact. `pack()` then iterates every field and calls
  `v.write_flat(&mut flat, f.offset / 4)` into `flat: [f32; 64]`. A field at `offset >= 256` (so `i >= 64`),
  or a vec whose tail crosses index 64, indexes out of bounds and panics.
- **Impact:** Any authored/hot-reloaded/discovered `.wgsl` whose `struct Material` reflects past 256 bytes
  crashes the renderer/worker. `discover_shaders()` and `reflect_shader_schemas()` (on `AssetEvent<Shader>`,
  i.e. hot-reload) ingest *any* shader on disk, and `repack()` runs on the per-frame engine-field write path
  (`horizon.rs::write_engine`), so the panic would recur every frame.
- **Fix (pack-side guard, prefer this so a partially-fitting struct still renders):** `ParamType::components()`
  already exists at `dyn_params.rs:85`. Guard before `write_flat`:
  ```rust
  for f in &self.fields {
      let i = f.offset / 4;
      if i + f.ty.components() > BLOCK_F32S { continue; } // BLOCK_F32S == 64
      if let Some(v) = values.get(&f.name).copied().or(f.default) {
          v.write_flat(&mut flat, i);
      }
  }
  ```
  Alternatively make the warn truthful by truncating `fields` in `parse()` to those whose
  `offset + ty.size() <= BLOCK_BYTES`.
- **Verifier note:** Confirmed; severity high→**medium** because no shipped shader triggers it today (largest,
  `regolith.wgsl`, is ~80 bytes / 14 fields; all six Material-bearing shaders are far under 256). It is a latent
  landmine in the documented hot-reload/discovery workflow, not an actively-firing crash.

### CPU-1 — `shade_dynamic_entities` re-marches every entity every render frame while the sun moves
- **File:** `crates/lunco-environment/src/horizon.rs:506-573` (gate 541-552, march 563-573); scheduled in
  `Update` at 644-653
- **Category:** inefficiency · **Confidence:** high
- **Problem:** Runs in `Update` (uncapped render FPS). The only throttle is the scene-wide boolean
  `if !sun_moved && !gt.is_changed() { continue; }`. The instant the sun direction changes by `>~0.1°`
  (`SUN_EPSILON_COS`), the gate opens for **every** mesh entity, each running `HeightField::sun_visibility`
  (a ≤96-step CPU ray-march) once per terrain. No per-entity timer, no work budget, no cross-frame amortization.
- **Impact:** `O(N_entities × M_terrains × march)` on the main thread every frame the sun animates (day/night
  cycle, `SetEnvironmentLight` slider drag), at whatever FPS the renderer delivers (notes say 120–175). The cost
  lands precisely when shadows are changing.
- **Fix:** Decouple from render frame and amortize. (a) Move to `FixedUpdate` or gate on a `Local<Timer>` at
  15–30 Hz; (b) when `sun_moved`, round-robin a slice of entities per frame via a `Local<usize>` cursor — the
  output is already quantized to 1/32 (`horizon.rs:576`) and deduped (`HorizonShade.last_vis`, 1e-3), so a few
  frames of lag is invisible; (c) keep the per-entity `gt.is_changed()` fast path for the static-sun case.
- **Verifier note:** Confirmed; high→**medium**. The march early-exits (out-of-bounds / `vis<=0` / `t>max_t`),
  so 96 is an upper bound, and a static sun costs ~0 via the fast path. Real but intermittent and bounded.

### SHA-2 — Object-space normal recovery is wrong under non-uniform scale (cap/barrel branch mis-picks)
- **File:** `assets/shaders/wheel.wgsl:69-71` (branch threshold at 74)
- **Category:** correctness · **Confidence:** medium
- **Problem:** `R = mat3(normalize(m[0].xyz), normalize(m[1].xyz), normalize(m[2].xyz))` then
  `n_local = normalize(transpose(R) * normalize(input.world_normal))`. Bevy transforms the vertex normal by the
  **inverse-transpose** of the model 3×3 (`bevy_pbr-0.18.1/.../mesh_functions.wgsl:68`), so for a model `R0·S` the
  world normal is `R0·S⁻¹·n_obj`. Column-normalizing `M` recovers `R0` exactly, so `Rᵀ·n_world = S⁻¹·n_obj`, **not**
  `n_obj`. Under uniform `S` the scalar cancels in `normalize()` (exact); under **non-uniform** `S` the recovered
  direction is skewed, and the `abs(n_local.y) > 0.5` cap-vs-barrel branch can flip (a true 45° bevel normal
  recovers to `y≈0.316` under a 3× axle stretch).
- **Impact:** Wheel cap/barrel split and the lat-long mapping distort on non-uniformly scaled wheels. Cosmetic
  but visible. Non-uniform scale is reachable: `lunco-usd-bevy/src/lib.rs:544-547` sets `transform.scale = v`
  directly from `xformOp:scale` (a `Vec3`), and non-uniform authoring is explicitly documented.
- **Fix:** For these centred-primitive props, derive the object-space surface direction from **position**, not the
  normal: `normalize((inverse(m) * vec4(world_position - m[3], 1.0)).xyz)` — exact under any affine model transform
  (no inverse-transpose mismatch). If non-uniform scale is genuinely never used on these props, document that
  constraint instead, since the current doc comment ("Rᵀ·n_world is the mesh-local normal") is only true under
  uniform scale.
- **Verifier note:** Confirmed (math + numerics + Bevy source). Same defect as SHA-3 (balloon) — fix both with
  the same position-based approach.

### LIG-2 — Far plane unconditionally overwritten to `1e15`, negating adaptive-near and authored zoom ranges
- **File:** `crates/lunco-avatar/src/lib.rs:1814`
- **Category:** correctness · **Confidence:** high
- **Problem:** `update_avatar_clip_planes_system` computes `near` from the nearest body but assigns
  `perspective.far = 1.0e15` every frame on every `AdaptiveNearPlane` camera, with no dependence on scene extent
  or zoom. The chase camera clamps zoom to `1e6` (line 713) and orbit to `1e11` (line 825), both far below `1e15`,
  so the depth dynamic range is dominated by the static far and the adaptive-near work buys little depth precision.
- **Impact:** Worst-case depth ratio at all times, even when the visible scene is metres deep.
- **Fix:** Derive `far` from the scene the same way `near` is derived — track the current camera zoom distance (or
  farthest relevant rendered surface) + margin and clamp so `far/near` stays within ~1e5. Only widen `far` when a
  genuinely distant body is in frame, ideally via a dedicated background/far camera pass.
- **Verifier note:** Confirmed; high→**medium**. The component is named `AdaptiveNearPlane` (adapting only near
  was the explicit intent), near adaptivity still does its primary job (no near-clipping on approach), and Bevy
  0.18 reverse-Z F32 depth concentrates precision near the camera, softening the impact. The `1e15` far is
  plausibly intentional to keep distant celestial bodies in-frustum. Real quality issue, not a high-severity defect.

### LIG-4 — `shadow_normal_bias 2.5` is large — peter-panning (detached contact shadows) on small props
- **File:** `crates/lunco-usd-bevy/src/light.rs:142-145`
- **Category:** correctness · **Confidence:** medium
- **Problem:** Default `shadow_normal_bias=2.5` / `depth_bias=0.06`. Normal bias is scaled by the cascade's texel
  size (`bevy_pbr-0.18.1` `directional_light.rs:135`, `render/light.rs:577`), so the same scalar yields a larger
  world offset in the coarse far cascades (see LIG-3). At 2.5 it kills grazing-angle terrain acne, but as one
  global per-light value it also detaches contact shadows from small casters (rover parts). You cannot have tight
  contact shadows on rovers **and** acne-free terrain simultaneously.
- **Impact:** Floating/detached contact shadows under rovers and props at near-to-mid field.
- **Fix:** Lower `normal_bias` toward ~1.0 and lean on `depth_bias` + the mesh-accurate near cascade for acne
  control. Better: keep terrain self-shadow out of the directional CSM entirely (the heightfield march already
  handles it — `horizon_march.wgsl::sun_visibility`) and reserve a low-bias CSM for crisp object/contact shadows.
  Per-light authorability (`lunco:shadow:normalBias`) and live tuning (`SetEnvironmentLight`) already exist.
- **Verifier note:** Confirmed; Bevy-documented artifact. The project's own fallback sun (`sandbox.rs:618-619`)
  uses `0.8` with a comment that the bias "must stay small or it detaches/softens the contact edge" — independent
  corroboration of the trade-off. Deliberate/authorable visual trade-off, not a crash → stays medium.

### LIG-5 — Fallback Sun light uses default cascade config + biases + 2048 map (inconsistent with USD path)
- **File:** `crates/lunco-celestial/src/big_space_setup.rs:181-193`
- **Category:** correctness · **Confidence:** high
- **Problem:** The fallback `DirectionalLight { illuminance:10_000, shadows_enabled:true, ..default() }` has **no**
  `CascadeShadowConfig`, **no** bias overrides, and never inserts `DirectionalLightShadowMap` (only the USD path,
  `light.rs:147`, does). So a scene authoring no UsdLux sun inherits Bevy defaults — `maximum_distance 150 m`
  (not 1500), `first_cascade_far_bound 10 m`, biases `0.02 / 1.8`, shadow map `2048` — a materially different,
  untuned setup that shows the very acne the USD path raised biases to 0.06/2.5 to avoid.
- **Impact:** Shadow behaviour changes discontinuously depending on whether a scene happens to author a
  `DistantLight`; acne and missing 4096 map on no-UsdLux scenes.
- **Fix:** Factor cascade + bias + shadow-map config into a shared helper used by **both** `light.rs` and
  `big_space_setup.rs` (same `CascadeShadowConfigBuilder` defaults 1500/40/4/0.1, `shadow_depth_bias 0.06`,
  `shadow_normal_bias 2.5`, and `DirectionalLightShadowMap { size: 4096 }`).
- **Verifier note:** Confirmed; the finding's prose said Bevy default `maximum_distance ≈1000 m` but it is
  actually 150 m — which *understates* the divergence (10× rather than ~1.5×). Stays medium (visual consistency,
  not a crash; bites only when no UsdLux sun is authored over shadow-receiving terrain).

### WASM-1 — USD regolith/horizon terrain silently loses all ray-march shadows on web
- **File:** `crates/lunco-environment/src/horizon.rs:198-206, 415-485`
- **Category:** wasm / observability · **Confidence:** high
- **Problem:** On wasm, `start_horizon_bakes` removes `HorizonShadowTerrain` and emits only a per-entity `warn!`;
  no bake task runs. `HorizonMap` is inserted only by `finish_horizon_bakes` (never on wasm), so
  `wire_terrain_materials` (queries `&HorizonMap`) never wires `sun_dir/sun_tan_radius/hf_size/hf_res/csm_far` —
  they stay zero. The march WGSL is gated `#ifdef VERTEX_UVS_A`, and the planar UVs are inserted only in
  `finish_horizon_bakes`; the Shackleton DEM glb ships POSITION/NORMAL only, so the whole march block compiles out.
  Meanwhile `apply_usd_shader_materials` *does* run on wasm and builds the regolith material with zeroed engine
  fields.
- **Impact:** Web renders opted-in terrain lit-but-flat (no far-field march, no near/far split), signalled only by
  a buried log. (Near-field CSM terrain shadows still work via `apply_pbr_lighting` — what is lost is the
  far-field march + the split, exactly as the module docs intend on web.)
- **Fix:** (a) Bake the heightfield off-thread on wasm via a real web worker; or (b) make the degradation explicit:
  on skipped bake, set a resource/flag and surface it once in the Inspector Environment section ("terrain
  ray-march shadows unavailable on web") instead of a per-entity log; document the regolith-on-wasm=flat outcome
  next to the shader, not just at the bake site.
- **Verifier note:** Confirmed end-to-end. Graceful, documented degradation but a genuine observability/UX gap
  split across two crates with no surface-level indicator → medium. *(See ARC-2 for the parallel picker gap.)*

### ARC-1 — Horizon bake/wire/shade systems have no preview-layer guard → preview terrain leaks into main scene
- **File:** `crates/lunco-environment/src/horizon.rs:189-196, 407-414, 518`
- **Category:** correctness · **Confidence:** medium
- **Problem:** `instantiate_usd_prim` (`lunco-usd-bevy/src/lib.rs:323-338`) stamps `HorizonShadowTerrain` on any
  prim with `lunco:terrain:horizonShadows=true`, **including** prims spawned under the RTT preview `scene_root`
  (preview runs the same `sync_usd_visuals` pipeline). The three horizon queries filter on
  `HorizonShadowTerrain`/`HorizonMap` but **not** `RenderLayers`/`UsdPreviewOnly`. So a previewed terrain gets a
  full heightfield bake + R32Float texture, its StandardMaterial is swapped to ShaderMaterial, and
  `shade_dynamic_entities` marches **main-scene** entities against the preview terrain. `pick_sun` already uses
  `Without<RenderLayers>` — the pattern exists but was omitted here. `RenderLayers` is also applied by a *separate
  later* system (`propagate_preview_render_layer`), leaving a window where the preview terrain has no layer at all.
- **Impact:** Cross-scene shadow contamination (main-scene entities visibly mis-darkened) + wasted bake and GPU
  memory. Native-only (bake is skipped on wasm) and gated on opening, in preview, a USD asset carrying the custom
  terrain attribute.
- **Fix:** Best — don't stamp `HorizonShadowTerrain` on preview-stage prims at all (gate `instantiate_usd_prim`
  on `UsdPreviewOnly` ancestry). Simplest — add `Without<RenderLayers>` to the `terrains` queries in
  `start_horizon_bakes`, `wire_terrain_materials`, and `shade_dynamic_entities` to mirror `pick_sun`.
- **Verifier note:** Confirmed; one-directional preview→main contamination, real wrong shading, no crash → medium.

---

## 3. Low findings (condensed)

| ID | Title | File:line | Fix in one line | Note |
|---|---|---|---|---|
| **SHA-3** | balloon.wgsl: same non-uniform-scale normal recovery → lat-long checker warps | `assets/shaders/balloon.wgsl:69-71` | Use position-based object-space direction (as SHA-2); needs `m[3]` origin | Confirmed; med→**low**, cosmetic + non-uniform-scale-only |
| **SHA-4** | regolith `bump_layer` perturbation unbounded by `strength` at high fine_bump | `assets/shaders/regolith.wgsl:189` | Cosmetic over-tilt only; the asserted NaN/flip are *provably impossible* (perturb ⟂ n, length≥1) | **Uncertain**; recategorize correctness→cosmetic |
| **MAT-1** | every `set*()` does a full `repack()`; `write_engine` repacks 5× | `crates/lunco-materials/src/shader_material.rs:118-160`; `horizon.rs:438-445` | Add `set_many`/`with_values` guard that repacks once at end | Confirmed; high→**low**, micro-scale (no per-pixel amplification, change-gated) |
| **MAT-2** | `set()` heap-allocates a `String` key even when key exists | `shader_material.rs:127-130` | `if let Some(s)=values.get_mut(name){*s=v}else{insert(name.to_string(),v)}` | Confirmed; med→**low**, change-gated, alloc dwarfed by repack |
| **MAT-3** | `wire_terrain_materials` rewrites static `hf_size/hf_res` + re-clones `height_map` every sun move | `horizon.rs:438-461` | Write static heightfield params once (gate on `height_map != Some`); per-frame write only `sun_dir`+`csm_far` | Confirmed; med→**low**, GPU re-upload already 1× per fire |
| **MAT-5** | `discover_shaders` reads+parses every `.wgsl` then discards the schema; re-parsed by reflect cache | `shader_material.rs:328-353` | Startup-only; cheap annotation scan or feed schemas into a path-keyed cache | Confirmed; **low** |
| **MAT-6** | `reflect_shader_schemas` scans all materials every frame even with no shader events | `shader_material.rs:453-492` | Skip `mats.iter()` scan unless a relevant `AssetEvent` fired; or assign schema at creation | Confirmed; **low** (empty `Vec` does *not* allocate — that sub-claim is wrong) |
| **CPU-2** | per-entity loop recomputes terrain affine inverse + sun-local vector (loop-invariant) | `horizon.rs:564-567` | Precompute `(inv, sun_local)` per terrain once before the entity loop; keep `transform_point3` inside | Confirmed; med→**low**, M (terrains) ≈ 1 so payoff ≈ O(N) saved |
| **CPU-3** | `update_avatar_clip_planes_system` iterates all bodies per camera every PostUpdate, no change gating | `lunco-avatar/src/lib.rs:1805-1825` | Gate on camera/body `Changed`/`Ref`; cache last near | Confirmed; **low**, body count is a fixed handful |
| **CPU-4** | StandardMaterial shadow path mints a permanent unique material on first sub-full visibility, never freed | `horizon.rs:589-606` | Prefer routing shadow-darkenable props through `ShaderMaterial` (sun_vis scalar, clone-free); or revert to shared handle at `vis>0.99` | Confirmed; **low**, one extra asset per shadowed entity, bounded |
| **ARC-2** | `discover_shaders` native-only; web picker silently omits on-disk shaders, no log | `shader_material.rs:242-243, 315-353` | Build-time static manifest (`build.rs`/`include_dir!`) so native==web; else one-time `info!` on wasm | Confirmed; **low**, affects picker offerings not usability |
| **ARC-3** | `force_hard_shadow_filtering` applies `Hardware2x2` to every `Camera3d` incl. RTT preview | `lunco-client/src/bin/sandbox.rs:732-741` | Scope to window-targeting cameras (filter `RenderTarget::Window`; `WorkbenchViewportCamera` alone won't exclude preview) | Confirmed; **low**, inert today (preview sun shadows off), latent for future secondary cams |

---

## 4. Cross-cutting themes

1. **Per-material repack/alloc churn (MAT-1, MAT-2, MAT-3).** Every `set*()` on `ShaderMaterial` does a full
   256-byte `repack()` and an unconditional `String` key alloc, and the terrain wiring fires 5 setters per sun
   move while re-writing post-bake constants. Each is individually low, but they compound on the same code path.
   A single batched mutator (`with_values(|v| …)` that repacks once on drop + `get_mut`-else-insert keys) fixes
   all three at once and is the cheapest high-leverage cleanup.

2. **Main-thread per-frame CPU scaling with scene/body count, ungated (CPU-1, CPU-2, CPU-3, MAT-6).** Several
   systems run in `Update`/`PostUpdate` with full O(N) or O(N×M) work and only coarse or no change gating. The
   common remedy is the same: a timer/`FixedUpdate` cadence, round-robin cursors, hoisting loop-invariants, and
   `Changed`/event gating. CPU-1 is the one that actually bites (during sun motion); the rest are cheap insurance.

3. **WASM feature gaps signalled only by logs (WASM-1, ARC-2).** Two distinct web degradations — no terrain
   ray-march shadows, and a curated-only shader picker — are both correct-but-silent, each flagged only by a
   buried `warn!`/nothing. Pattern fix: a single platform-capability surface (resource + one Inspector line)
   instead of per-site logs, and a build-time shader manifest so native/web cannot drift.

4. **Depth precision & shadow tuning at lunar scale (LIG-2, LIG-3, LIG-4).** A hard `1e15` far, a 1500 m / 4-cascade
   CSM split, and a high 2.5 normal bias are all in tension: the static far wastes the depth budget, the coarse
   far cascade has low texel density, and the large bias compensating for that detaches contact shadows. They
   should be tuned together (adaptive far + per-scene cascade/bias), and terrain self-shadow should lean on the
   heightfield march so the directional CSM can run a low bias for crisp object shadows.

5. **Authored-vs-fallback / main-vs-preview consistency (LIG-5, ARC-1, ARC-3).** Three places where a second code
   path (fallback sun, RTT preview camera, preview terrain) diverges from or contaminates the primary path because
   config/markers weren't factored or scoped. Fixes: a shared light-config helper, preview-layer guards on the
   horizon queries (or no stamping on preview prims), and `RenderTarget::Window`-scoped camera mutations.

6. **Unbounded/untruthful buffer handling (SHD-1).** The one true crash-class item: `pack()` trusts a "will be
   clipped" promise that `parse()` never keeps. Make the bound real at pack time.

7. **Procedural-shader normal correctness under affine transforms (SHA-2, SHA-3).** Both object-space prop shaders
   reconstruct the mesh-local normal in a way that is only exact under uniform scale. Use the object-space
   *position* direction for centred primitives — exact under any affine transform.

---

## 5. Prioritized fix order (rough effort)

| # | Item(s) | Why first | Effort |
|---|---|---|---|
| 1 | **SHD-1** pack-side bounds guard | Removes a latent every-frame crash on the hot-reload/discovery path; `components()` helper already exists | **XS** (~5 lines) |
| 2 | **CPU-1 + CPU-2** throttle + hoist `shade_dynamic_entities` | Biggest live frame-rate win, during sun motion; CPU-2 hoist is free alongside | **S** (timer/cursor + move 2 lines out of inner loop) |
| 3 | **LIG-2** adaptive far plane | One-line pin → small scene-extent/zoom-based calc; unlocks the depth budget | **S** |
| 4 | **LIG-5** shared light-config helper | Factor cascade+bias+4096-map once; fixes authored-vs-fallback discontinuity (and reusable for LIG-4) | **S–M** |
| 5 | **MAT-1/2/3** batched `with_values` mutator | One change collapses repack/alloc churn across all three; tidy the terrain wiring to write statics once | **S** |
| 6 | **ARC-1** preview-layer guard | Either gate stamping on `UsdPreviewOnly` ancestry, or `Without<RenderLayers>` on 3 queries | **S** |
| 7 | **SHA-2 + SHA-3** position-based object-space direction in wheel/balloon | Shared one-shader-pattern fix; correctness under non-uniform scale | **S** |
| 8 | **WASM-1 + ARC-2** platform-capability surface + build-time shader manifest | Replace buried logs with one Inspector indicator; `build.rs`/`include_dir!` manifest stops native/web drift | **M** |
| 9 | **LIG-4** lower normal bias / move terrain self-shadow off CSM | Pairs with LIG-5; depends on confirming march-only terrain shadows | **M** (needs visual tuning) |
| 10 | **LIG-3** cascade split tuning | Tuning knob already authorable; verify texel density per scene | **S** (per-scene tuning) |
| 11 | **CPU-3, CPU-4, MAT-5, MAT-6, ARC-3, SHA-4** | Low-impact cleanups; batch opportunistically | **XS each** |

**Effort key:** XS ≈ minutes, S ≈ <½ day, M ≈ ½–1 day.

---

*Methodology: every finding above was independently re-verified against the cited source (Bevy 0.18.1 internals
checked where the claim depended on engine behaviour). Severities are the corrected post-verification values;
items lowered from their original rating carry a verifier note. No findings were invented beyond the verified set.*
