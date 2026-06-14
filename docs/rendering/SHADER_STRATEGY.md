# Shader Strategy — Great Shaders at Lunar Scale

> **Scope.** How `lunco` makes physically convincing shaders for an airless, sub-wavelength-grained, retroreflective world at human-to-orbital scale, on **both** the native and WASM web builds. Targets Bevy 0.18, big-space floating origin, the `lunco-materials` self-describing `ShaderMaterial` system, and the shaders in `assets/shaders/`.
>
> **Thesis.** The Moon looks wrong under naive Lambert + GGX PBR because that BRDF was built for atmospheres, microfacet specular, and energy-conserving diffuse — *none* of which describe a porous, airless, sub-wavelength-grained retroreflective powder. Three changes carry ~80% of the realism, and crucially **all three are pure per-pixel/per-light math with no bake dependency, so they fix the look on the shadow-less WASM build too**: (1) a Lommel-Seeliger + opposition-surge diffuse replacing Lambert, (2) physical lux/EV exposure with an earthshine fill light, and (3) jet-black ambient discipline.

---

## 1. TL;DR — the five highest-leverage moves, ranked

| # | Move | Why it matters | Effort | Bevy 0.18 |
|---|------|----------------|--------|-----------|
| **1** | **Lommel-Seeliger + opposition-surge diffuse** in a shared `lunco::lunar` WGSL module, used by `regolith.wgsl` and `terrain_shadow.wgsl` | The Moon back-scatters toward the *light*, brightens toward zero phase (heiligenschein), and has almost no limb darkening. GGX/Lambert models none of this. This is *the* lunar-look fix and works identically on web. | ½ day | ✅ pure WGSL |
| **2** | **Earthshine 2nd DirectionalLight** — cool blue (~0.6,0.75,1.0), **shadows OFF**, ~10–15 lx, direction = toward Earth | Turns dead-black "missing geometry" shadow cores into faintly readable blue relief — the real, *directional, colored* fill. A scalar ambient cannot be "blue in shadow + warm in sun." | 2 hr | ✅ multi-dir |
| **3** | **Physical exposure** — drive the sun in lux (~134,000 lx), set camera `Exposure` from EV100 ≈ 15, kill magic-number HDR scaling | The scene spans ~4 orders of magnitude sunlit→shadow-core. Only physical lux/EV holds that range without blowing ground or crushing shadows. | 2 hr | ✅ `Exposure` |
| **4** | **Fix the `tan_sun_r` divide-by-zero** (`horizon_march.wgsl:61`) + default solar `diameter_deg` to ~0.53° | **Hard latent bug**: a sun with no authored angular size → `tan(0)=0` → `occ=±inf` → the march returns garbage / all-black. Triggers whenever the USD sun omits angular diameter. | 1 hr | ✅ have it |
| **5** | **Make unrecognized `dyn_params` field types a hard error** (not a silent skip) | **Worst authoring footgun**: an unparsed field (`mat4`, `array`, `mat3`) vanishes from the schema but still occupies WGSL bytes → *every* later field packs at the wrong std140 offset → silent garbage uniforms, no error. | 2 hr | n/a (Rust) |

Moves 1–3 are the "day-one" trio that change everything; 4–5 are latent correctness landmines that should be defused alongside.

---

## 2. The lunar look: why naive PBR fails

### 2a. The regolith is retroreflective — it back-scatters toward the light, not the camera

Lunar soil is a porous powder of sub-wavelength grains. Two physical mechanisms make it brightest at **zero phase angle** (camera looking down the sun vector):

- **Shadow hiding** — at opposition every grain hides its own shadow, so no dark shadow area is visible. Broad peak, phase α < ~20°.
- **Coherent backscatter** — constructive interference of reciprocal light paths in the fine fraction, a razor-narrow spike at **α < 2–3°**.

Together these are the **opposition surge / heiligenschein**: a real brightness increase of tens of percent toward zero phase, parameterized in Hapke as amplitude `B₀` and angular width `h`. **GGX/Lambert has no term that brightens toward the light source** — if anything microfacet specular brightens toward the *mirror* direction. A naive PBR Moon is missing its single most distinctive photometric feature. This is also **why the full Moon looks flat**: at full Moon, Earth observers see it near zero phase, the surge flattens the disk to near-uniform brightness, and limb darkening nearly vanishes — a flat coin, not a lit sphere.

### 2b. Lommel-Seeliger limb behavior, not Lambert

Lambert diffuse scales as `cos(incidence) = μ₀ = N·L`. Real regolith follows closer to **Lommel-Seeliger**, whose reflectance ∝ `μ₀ / (μ₀ + μ)`, where `μ = cos(emission) = N·V`. The `(μ₀+μ)` denominator **cancels most of the limb darkening** a Lambert sphere shows — brightness stays high toward the limb where Lambert falls to zero. (The textbook lunar-photometry correction multiplies Lambert data by `(μ+μ₀)/μ₀` to flatten it.) In our human-scale scene this manifests as **grazing-incidence terrain reading too dark** — Lambert's `cos(i)` crushes glancing slopes that the real surface keeps bright. This matters a lot under the low Shackleton-rim sun.

### 2c. Near-zero atmosphere → hard terminator, jet-black shadow cores, no aerial perspective

No air → no Rayleigh in-scatter to fill shadows, no aerial-perspective desaturation with distance, no atmospheric softening of the day/night line. Consequences:

- **Hard terminator**, softened *only* by the sun's finite disc (≈0.25° radius) and local relief.
- **Shadow cores to near-absolute black** — the only fill is earthshine + regolith bounce, both very dim.
- **Distant mountains stay as sharp and contrasty as near ones.**

Naive engines apply `DistanceFog` and a fat scalar ambient that **gray-fills the blacks** — the exact opposite of the lunar look. Our scene correctly has **`DistanceFog` ABSENT** — keep it that way, and keep `GlobalAmbientLight` near zero.

### 2d. Extreme dynamic range (~4 orders of magnitude)

Apollo's Hasselblad standard was **f/11 @ 1/250 s for sunlit ground**; open shadow needed **f/5.6** — a ~2-stop (4×) jump just to read open shadow. Shadow *cores* with only earthshine fill are thousands of times dimmer: earthshine irradiance at the Moon peaks near **150 mW/m² (~1/10,000 of sunlit solar flux)** near zero phase. The scene spans roughly **4 decades** sunlit→shadow. An 8-bit LDR pipeline at fixed exposure cannot hold this. We have HDR — good — but **no physical exposure yet** (§5).

### Quantify (use these numbers)

| Quantity | Value |
|----------|-------|
| Normal albedo | **0.07** maria / **0.13–0.16** highlands |
| Bond albedo | ~0.11 |
| Geometric albedo | ~0.12 |
| Sun angular radius | **≈ 0.25°** (diameter ~0.53°) → `tan(radius) ≈ 0.00436` |
| Sun illuminance at lunar surface | **≈ 134,000 lx** (no atmospheric loss) |
| Earthshine | **~150 mW/m² ≈ 1/10,000 sun**, cool blue, dominant at lunar night |
| Opposition surge | coherent-backscatter spike α<2–3°, shadow-hiding broad peak α<20° |
| Camera EV100 (sunlit) | **≈ 15** (sunny-f/11) |

---

## 3. A recommended lunar surface BRDF for `regolith.wgsl`

**Decision:** keep `apply_pbr_lighting` for the GGX **specular** path (regolith has a weak broad glossy lobe and you get shadow-map/light integration for free) and for **ambient/IBL**, but **replace the Lambert diffuse** for the dominant DirectionalLight with a Lommel-Seeliger + opposition diffuse term computed yourself in the shader from the engine `sun_dir`. Bevy's diffuse is hard-wired Lambert inside `apply_pbr_lighting`, so the clean approach is a **sun diffuse multiplier** you compute and feed as `base_color` (or add to the output) while letting the PBR path handle the rest.

Put it in a new shared module beside `pbr_lit.wgsl` so `regolith.wgsl` and `terrain_shadow.wgsl` both call one function (terrain_shadow inlines its own `PbrInput`, so the term must be a *function both call*, not a flag buried in one file).

```wgsl
// assets/shaders/lunar_brdf.wgsl
#define_import_path lunco::lunar

// Lommel-Seeliger single-scattering core. Replaces Lambert mu0.
// Brightness stays high to the limb (denominator cancels limb darkening).
// The albedo/4 prefactor folds into base_color, so this returns the *shape*.
fn lommel_seeliger(mu0: f32, mu: f32) -> f32 {
    return mu0 / max(mu0 + mu, 1e-4);
}

// Shadow-hiding + coherent-backscatter opposition surge, phase alpha in RADIANS.
//   b0   ~ 0.6..1.0   amplitude
//   h_sh ~ 0.06 rad   (~3.5 deg) broad shadow-hiding term, <20 deg
//   h_cb ~ 0.02 rad   (~1.2 deg) narrow coherent-backscatter spike, <3 deg
fn opposition_surge(alpha: f32, b0: f32, h_sh: f32, h_cb: f32) -> f32 {
    let t = tan(alpha * 0.5);
    let shoe = b0       / (1.0 + t / h_sh);   // broad
    let cboe = (b0*0.5) / (1.0 + t / h_cb);   // narrow spike
    return 1.0 + shoe + cboe;
}

// Henyey-Greenstein single-particle phase. Backscattering => g_hg < 0 (~ -0.25).
fn hg_phase(cos_g: f32, g_hg: f32) -> f32 {
    let d = 1.0 + g_hg*g_hg - 2.0*g_hg*cos_g;
    return (1.0 - g_hg*g_hg) / max(pow(d, 1.5), 1e-4);
}

// The load-bearing replacement for Bevy's Lambert * mu0 on the SUN light.
//   N, L (to-sun), V (to-camera) all unit. albedo is LINEAR.
// Multiply the result by sun_irradiance * sun_vis at the call site; keep the
// earthshine / ambient fill OUTSIDE this so shadow cores stay black (sec 4b).
fn regolith_diffuse(
    N: vec3<f32>, L: vec3<f32>, V: vec3<f32>, albedo: vec3<f32>,
    b0: f32, h_sh: f32, h_cb: f32, g_hg: f32,
) -> vec3<f32> {
    let mu0 = max(dot(N, L), 0.0);
    let mu  = max(dot(N, V), 0.0);
    let cg  = clamp(dot(L, V), -1.0, 1.0);   // L and V both point AWAY from surf
    let alpha = acos(cg);                    // phase angle
    let ls    = lommel_seeliger(mu0, mu);
    let surge = opposition_surge(alpha, b0, h_sh, h_cb);
    let phase = hg_phase(cg, g_hg);          // backscatter lobe
    return albedo * ls * surge * phase;
}
```

**Sign sanity check (do this in-engine on a sphere):** `dot(L,V)` uses to-sun and to-camera, both pointing *away* from the surface, so `dot≈+1` at opposition — exactly where the surge fires. The sphere should **glow brightest where its Earth-observed shadow would be.** If it glows toward the mirror highlight instead, a vector is flipped.

**Composition with `apply_pbr_lighting`:**
1. Compute `regolith_diffuse(...)` for the sun, multiply by sun irradiance and `sun_vis` (the horizon-march result).
2. Let `apply_pbr_lighting` contribute **specular only** (high roughness → negligible lobe, that's correct) plus **ambient/IBL** for the warm-bounce fill.
3. Add the **earthshine** DirectionalLight contribution *outside* the `sun_vis` gate (it's a separate light).
4. Run `main_pass_post_lighting_processing` as today.

**Recommendations:**
- **P0** — `lunco::lunar::regolith_diffuse` in a shared module; call from both terrain shaders. Biggest single realism win, ~½ day, pure WGSL, no bake — works on web.
- **P1** — Expose `b0 / h_sh / h_cb / g_hg` as `//!@ui` params so the Inspector tunes the surge live, with **maria vs highlands presets** (highlands more backscattering per the LOLA Hapke study). ~1 hr; `dyn_params` already does this.
- **P1** — **Lower default albedos**: `regolith` `0.17 → ~0.13` (mare), `terrain_shadow` `0.5 → ~0.13` (0.5 reads as concrete). 0.17 is defensible for fresh Shackleton-rim highland but is the bright end.
- **P2** — Full Hapke multiple-scattering H-functions. Not worth it real-time; LS + surge + HG is what SurRender / Cycles lunar work converges to visually. **Skip.**

---

## 4. Lighting model: earthshine fill + warm bounce ambient

A single DirectionalLight gives a perfectly hard key with **zero fill** — *almost* right, but it loses the two real fill sources, and a flat scalar `GlobalAmbientLight` fills *uniformly and grayly*, destroying directional shadow shaping and graying the blacks. The real Moon has two distinct, **directional and colored** fills.

### 4a. Earthshine — a dim, blue-ish 2nd DirectionalLight

From the Moon, Earth is ~50× brighter than the Moon is from Earth and earthshine **dominates** illumination during lunar night. It comes from Earth's direction (roughly opposite the sun at "full Earth"), high color temperature (clouds + ocean).

- Model as a **2nd DirectionalLight** in `crates/lunco-usd-bevy/src/light.rs` (you already spawn one from `DistantLight`).
- Illuminance **~10–15 lx** (≈ 1/10,000 of the ~134,000 lx sun).
- Color ≈ `(0.6, 0.75, 1.0)` cool blue.
- **Shadows OFF.**
- Direction = toward Earth in the sky frame.
- Wire its yaw/pitch/illuminance/color into `SetEnvironmentLight`.

**P0**, ~2 hr, fully supported (multiple directional lights). Huge shadow-readability win: dead-black "missing geometry" becomes faintly readable blue relief.

### 4b. Warm bounce ambient — a tiny pre-baked gradient EnvironmentMapLight

Sunlit regolith bounces a **warm** (~5800 K, slightly reddened by the albedo's spectral slope) low hemispherical fill in sunlit areas only. A single scalar ambient cannot be both "earthshine blue in shadow" and "warm bounce in sun." Use a small **`EnvironmentMapLight` (IBL probe)**: a cheap 2-color gradient cubemap (warm-up from ground bounce, cool/black from space) gives directional, colored ambient that respects geometry.

**Bevy 0.18 reality:** `EnvironmentMapLight` and `IrradianceVolume` exist and are supported, **but there is no built-in procedural IBL baker** (auto-skybox→IBL is a known Bevy gap; the glTF-IBL-Sampler path is offline-only and WASM-hostile). **Practical move:** ship a **tiny pre-baked 2×2 or 16×16 cubemap** (warm-ground / dark-space gradient) as an asset and assign it as `EnvironmentMapLight` — no runtime baker, works on WASM. Keep `GlobalAmbientLight` near **zero** once this is in.

**P1**, ~1 day.

**Discipline rule (P0, mostly there):** the sun diffuse term is what `sun_vis` gates; the earthshine DirectionalLight and the IBL bounce are **outside** that gate and add *additively*. Never lift shadow cores with a scalar ambient on the sun term — keep them black, let the fill lights color them.

- **P2** — CPU-computed L1/L2 spherical-harmonics ambient from sun direction + ground albedo, pushed as a uniform, for sun-tracking warm bounce without a cubemap. Nice-to-have; the static gradient cubemap is ~90% of it.

---

## 5. Exposure, tonemap, bloom

The ~4-decade range (§2d) demands **physical exposure**, not a fixed HDR scale. Apollo's f/11 @ 1/250 s is the calibration anchor.

- **P0 — Physical exposure (manual EV).** Drive the DirectionalLight in **lux** (sun ≈ **134,000 lx**), set per-camera **`Exposure`** from **EV100 ≈ 15** (sunny-f/11). Sunlit ground then sits mid-gray and shadows fall off naturally — no magic-number scaling. Expose a `SetEnvironmentLight`-adjacent EV control. ~2 hr; `Exposure` is supported in 0.18.
- **P1 — Tonemap: keep `TonyMcMapface` default, add `AgX` toggle, never `ACES`.** TonyMcMapface is intentionally neutral and **the right call** — it preserves the input stimulus and holds near-black shadows without crushing. **ACES is wrong here**: its dramatic contrast S-curve and hue-shift (greens/reds → orange) fights the desaturated gray Moon and crushes shadow detail you want faintly visible. **AgX** is a fine alternate (gentle desaturating shoulder) — offer it as a toggle. Trivial (enum on the camera).
- **P1 — Bloom discipline.** Airless → no atmospheric glow. Current `Bloom { intensity: 0.4, prefilter threshold: 2.0 }`: keep threshold **high** so only genuine specular glints / the sun disc bloom, not the whole sunlit ground; consider **dropping intensity to ~0.15** for the lunar preset.
- **P2 — AutoExposure: skip.** A sim wants *deterministic, reproducible* exposure (screenshots). Ship **EV presets** (sunlit / shadow-detail / orbital) instead.

---

## 6. Current shaders — what to fix

Grounded in the per-file critique. Priority: **P0** = correctness landmine or the lunar-look fix; **P1** = visible quality/robustness; **P2** = polish.

| Shader / file | Issue | Fix | Pri |
|---------------|-------|-----|-----|
| `horizon_march.wgsl:61` | `occ = (…)/(t * tan_sun_r)`; `tan_sun_r` can be **0** (USD sun omits angular size → `unwrap_or_default()` → `diameter_deg=0` at `horizon.rs:381`) → `occ=±inf` → garbage / all-black march. **Hard bug.** | Clamp `tan_sun_r = max(tan_sun_r, 1e-4)` in WGSL **and** default `diameter_deg` to ~0.53° (tan radius **0.00436**) in Rust. | **P0** |
| `regolith.wgsl:189` | `normalize(n - strength*grad/eps)` can flip below the surface or hit a near-zero vector → **NaN/garbage** on night-facing micro-slopes at high `mid_bump`. | Clamp the perturbation magnitude and/or `faceforward` the result against the geometric normal. | **P0** |
| `dyn_params.rs` `parse_struct_fields` | Unrecognized field type (`mat4`, `array`, `mat3`) is **silently skipped** but still occupies WGSL bytes → every later field packs at the wrong std140 offset → silent garbage uniforms. **Worst footgun.** | **Hard error** on unknown type, or insert a "skip N bytes" opaque placeholder so downstream offsets stay correct. | **P0** |
| `regolith.wgsl` + `terrain_shadow.wgsl` | Plain Lambert/GGX, **no backscatter / opposition / Lommel-Seeliger** — the core lunar-look miss. terrain_shadow **inlines** its own `PbrInput`, so a flag in one file won't reach it. | Add `lunco::lunar::regolith_diffuse` (§3) as a shared function **both** call. | **P0** |
| `regolith.wgsl:183` (`bump_layer`) | `eps = 0.5/scale` is half a *base-octave* period, but `fbm` runs 3–5 octaves → the bump **aliases away** the high octaves it should capture. Forward-only difference (`ht-h0`) also biases direction. | Move to **analytic-gradient noise** (`vnoise` returning value + gradient) — fixes the aliasing **and** roughly halves the ~300 `hash13`/pixel close-up cost (kills the 3-tap bump). Fall back: central differences + sample at the finest octave's scale. | **P1** |
| `regolith.wgsl:178-180` | `if abs(n.y)>0.99` up-vector swap is a **discontinuity** — tangent frame snaps 90° on near-vertical crater walls → bump direction visibly jumps. | Smooth tangent selection (blend two candidate ups) or derive tangent from `fwidth(world_position)`. | **P1** |
| `regolith.wgsl` + `terrain_shadow.wgsl` (`#ifdef VERTEX_UVS_A`) | The horizon march **compiles out** on a POSITION/NORMAL-only mesh (the Shackleton DEM glb) → "ray-marched horizon shadow" silently does nothing. | Add planar UVs to the DEM mesh, or the headline feature stays dormant. | **P1** |
| `terrain_shadow.wgsl:28` | albedo default **0.5** reads as concrete. | Default to **~0.13**. | **P1** |
| `wheel.wgsl` / `balloon.wgsl` (object-space) | `mat3(normalize(c0),normalize(c1),normalize(c2))` then `transpose` is **not** the inverse rotation under non-uniform scale/shear → cap/barrel test and checker **skew** on squashed meshes. | Use the proper **inverse-transpose** normal matrix (Bevy `mesh_functions` helpers). (`solar_panel.wgsl` is pure-UV → immune.) | **P1** |
| `solar_panel.wgsl` | **No `fwidth` line AA** on any feature (gap/bus/border binary) → moiré/shimmer at distance/grazing; `bus_width 0.004` lines flicker sub-pixel. | Port `blueprint_grid.wgsl`'s `grid_mask` (`fwidth` + `smoothstep`). | **P1** |
| `horizon_march.wgsl:52` | `h0 + 0.35` is a fixed world-metre acne lift — at fine resolution 0.35 m can hide real small shadows. | Make it texel-relative (fraction of `texel·slope`). | **P2** |
| `horizon_march.wgsl:44` | `sun_local.y <= 0 → return 0.0` is a hard cutoff across `y=0` → visible hard line at sunset, no soft terminator. | Soft fade near the horizon. | **P2** |
| `wheel.wgsl:74` / `balloon.wgsl` | Hard cap/barrel & checker edges, no AA; balloon lat-long cells crowd/shimmer at poles. | `fwidth` cell-edge softening; pole-area fade. | **P2** |
| `pbr_lit.wgsl` | `reflectance = vec3(0.5)` hardcoded; **no tangent** passed (isotropic-only — any future normal-map/clearcoat path silently won't work); base/emissive alpha forced 1.0. | Document as isotropic-opaque-only. Consider param-driving reflectance for metal rims. | **P2** |

**Color convention (P0, cross-cutting).** Whether `//!@default albedo 0.17,0.17,0.17` is sRGB or linear is **undefined** between `dyn_params`, the Inspector, and USD import. The shaders treat `mat.albedo` as **linear** (fed straight to `base_color`). If the egui swatch round-trips sRGB, the 0.17 is misinterpreted → "colors look slightly off." **Define and enforce:** *all `color` params are linear in the buffer; the UI does sRGB↔linear at the widget.* This is the most likely subtle color bug.

---

## 7. Authoring at scale

### 7a. Procedural detail, orbit to bootprint — keep what works

`regolith.wgsl` already does the hard part well and this architecture is **correct**:

- **World-space multi-octave FBM** across four scale bands (5.5 mm grain → 12.5 cm clumps → 7 m hummocks → hectometre albedo) — sidesteps UV tiling entirely, crisp at every range.
- **Per-layer analytic AA** (`pw = length(fwidth(p))` computed **before** any branch — derivatives need uniform control flow) feeding `aa_fade`, gating each layer on `*_fade > 0.0`. This is both anti-aliasing **and** a perf/LOD strategy: far pixels skip the expensive octaves.
- Amplitude-normalized `fbm` so ramps stay valid across octave counts.
- The CSM-near / horizon-march-far split (`csm_far·0.5..0.9` smoothstep).

**Refinements:**

- **P1 — Domain warping** to kill FBM lattice streaks at grazing light: warp the sample point by a low-octave noise before the detail FBM:
  ```wgsl
  p += warp_amp * fbm(p * warp_scale);   // ~6 instructions, removes the axis-aligned tell
  ```
- **P1 — Detail normal map** (single tiling high-freq normal, **triplanar**, blended below the FBM's fine band) for the characteristic dusty micro-glint the analytic surge implies. ~½ day.
- **P1 — Triplanar projection** for any real **DEM/detail normal map** on steep crater walls (the mesh ships POSITION/NORMAL only, no UVs). World-space 3D *noise* is already triplanar-equivalent, so procedural detail is fine; a *sampled texture* needs hand-WGSL triplanar (~30 lines, 3 samples blended by `normal²`) — no built-in in 0.18.
- **P1 — SSAO/GTAO** for **macro contact darkening** (crater walls, rocks, away-from-sun creases) — the macroscopic cousin of the grain-scale self-shadowing that *is* the opposition effect. Bevy 0.18 supports `ScreenSpaceAmbientOcclusion`. ~½ day. **Don't double-count:** the §3 surge term *is* the analytic micro-shadow model — keep micro-AO subtle where the surge is strong.
- **P2 — Procedural crater field** (Voronoi-seeded rim+bowl) — FBM gives hummocks, not rimmed bowls, and craters dominate real lunar silhouette. Big win but ~2–3 days and independent of the BRDF work.

### 7b. `dyn_params` packing footguns — document these for authors

The std140 packer (`offset = round_up(cursor, align); cursor = offset + size`) is correct for all scalar/vec2/vec3/vec4 *sequences*, but authors must know:

1. **No `mat3`/`mat4`, no arrays, no nested structs.** Unrecognized fields are **silently dropped** from the schema but still occupy real WGSL bytes → all later fields misalign → garbage uniforms, no error. (Being upgraded to a hard error — §6 P0.) Until then: **stick to scalars and `vec2/3/4`.**
2. **256-byte block is a hard cap.** Over-budget `Material` logs a *warning* and **silently truncates** — engine fields (`csm_far` etc.) fall off and "shadows stop working." Surface it in the Inspector, not just the log.
3. **Engine fields eat ~48 bytes you don't see.** `sun_dir` (vec3), `sun_tan_radius` (f32), `hf_size` (vec2), `hf_res` (f32), `csm_far` (f32) are reserved in the same 256-byte block — leave room.
4. **Color = linear in the buffer** (see §6). UI converts at the widget.
5. **Integer params need the explicit `int` UI keyword.** A `u32` field with a plain `//!@ui count 1 16` gets a continuous **float Slider** that writes bit-reinterpreted garbage. Author integer sliders as `//!@ui count int 1 16` (or keep the field `f32`).
6. **Place a scalar right after a vec3** to use the vec3's 16-byte tail (the packer does this correctly, e.g. regolith `albedo: vec3; macro_clump_scale: f32` → offsets 0/12) — but your WGSL struct field order must match what the packer computes. Keep the WGSL `struct Material` field order identical to the `//!@` declaration order.

---

## 8. Phased plan

### P0 — day-one (the look + the landmines) — ~1.5 days total

| Change | Effort | Bevy 0.18 | No-bake (works on WASM) |
|--------|--------|-----------|--------------------------|
| `lunco::lunar::regolith_diffuse` (LS + surge + HG), shared by regolith & terrain_shadow | ½ day | ✅ pure WGSL | ✅ |
| Earthshine 2nd DirectionalLight (blue, shadowless, ~10–15 lx) | 2 hr | ✅ multi-dir | ✅ |
| Physical exposure: sun ≈134k lx, camera `Exposure` EV100≈15, drop magic scaling | 2 hr | ✅ `Exposure` | ✅ |
| Clamp `tan_sun_r = max(.,1e-4)` + default `diameter_deg` ~0.53° | 1 hr | ✅ have it | ✅ |
| `regolith.wgsl:189` clamp/`faceforward` the bumped normal vs NaN | 1 hr | ✅ | ✅ |
| `dyn_params` unknown-type → hard error (or opaque pad) | 2 hr | n/a | ✅ |
| Define + enforce color linear-in-buffer convention | 2 hr | n/a | ✅ |
| Keep shadow cores black: earthshine/IBL outside `sun_vis` gate; lower albedos (regolith 0.13, terrain_shadow 0.13) | 1 hr | ✅ | ✅ |

### P1 — quality + robustness — ~3–4 days total

- Surge params (`b0/h_sh/h_cb/g_hg`) as `//!@ui` + maria/highlands presets — 1 hr.
- Pre-baked gradient `EnvironmentMapLight` (warm-ground/dark-space); `GlobalAmbientLight → ~0` — 1 day. ⚠️ no procedural baker; ship a static 16×16 cubemap asset (WASM-safe).
- SSAO/GTAO macro contact darkening — ½ day.
- Tonemap: keep TonyMcMapface default, add AgX toggle, never ACES; bloom intensity → ~0.15 lunar preset — trivial.
- Analytic-gradient noise (fixes bump aliasing **and** ~halves close-up cost) — ½–1 day.
- Domain warp + detail normal map (triplanar) — ½ day each. ⚠️ triplanar = hand-WGSL.
- Smooth tangent selection in `bump_layer` — ½ day.
- Fix object-space prop normal matrix (inverse-transpose) in `wheel`/`balloon` — ½ day.
- `fwidth` line AA in `solar_panel.wgsl` (port `grid_mask`) — 1 hr.
- Add planar UVs to the DEM mesh so the horizon march actually runs — ½ day (mesh-side).
- Verify shadow bias (`depth 0.06` / `normal 2.5`) doesn't Peter-pan contact shadows at grazing sun; consider lower `normal_bias` for the lunar preset — ½ day.

### P2 — polish / skip

- Procedural crater field (Voronoi rim+bowl) — 2–3 days.
- Full Hapke multiple-scattering — **skip** (LS+surge+HG is the visual converged point).
- AutoExposure — **skip**; ship EV presets instead.
- CPU SH L1/L2 ambient — nice-to-have; static cubemap is ~90% of it.
- Texel-relative march acne lift; soft horizon-terminator fade; prop-edge AA; document `pbr_lit` isotropic-opaque-only.

---

## 9. Project-level absences that *cap* shader realism

Not shader bugs, but they bound the ceiling — track them:

- **No IBL/`EnvironmentMapLight`** → prop glints are sun-only, no sky reflection (addressed by §4b).
- **No AA beyond raw** (no TAA/SMAA/MSAA) → every hard-edged procedural pattern aliases; lean hard on per-shader `fwidth` AA.
- **No per-camera `Exposure`/AutoExposure today** → bloom threshold 2.0 is tuned to a fixed exposure (addressed by §5).
- **No sun disc / Earth-in-sky / star field** → empty black sky; the earthshine light fakes the *fill* but not the *backdrop*.
- **WASM: heightfield bake is skipped** (`horizon.rs`, async pool = main thread) → **no terrain-march shadows on web**. This is exactly why the §3 BRDF, §4a earthshine, and §5 exposure are prioritized: they are pure per-pixel/per-light math with **no bake dependency** and fix the look on web *and* native.

---

## Sources

- **Hapke opposition / coherent backscatter / shadow-hiding (α<2° vs α<20°), B₀/h, grain size:** Helfenstein 1997 *Icarus*; Hapke 1998 *Icarus*; Hapke 1993 *Science* (coherent backscatter); JGR 2013JE004580 (B₀/h amplitude & width, fines→larger B₀); LOLA NIR Hapke (highlands more backscattering).
- **Lommel-Seeliger limb darkening, (μ+μ₀)/μ₀ correction:** Wu et al. 2013 *Science Bulletin* (Chang'E-1 IIM); "Planetary Photometry: The Lommel-Seeliger Law."
- **Albedo (maria 0.07 / highlands 0.16, Bond 0.11, geometric 0.12):** Diviner-derived A&A 650 A38; the-moon.us Albedo; Bond albedo (Wikipedia).
- **Earthshine (blue, ~150 mW/m² ≈ 1/10,000 sun, Earth ~50× brighter, dominant at lunar night):** Glenar et al. 2019 *Icarus* (arXiv:1904.00236).
- **Apollo Hasselblad exposure (f/11 sunlit, f/5.6 shadow, 1/250 s):** moonhoaxdebunked Apollo still-photography primer; FlatEarth.ws Apollo exposure.
- **Real-time lunar / Hapke rendering practice (Cycles Hapke BRDF, SurRender):** SurRender (arXiv:2106.11322); Liang et al. Monte Carlo + Hapke, Chang'E-1 (PMC3913513); physics-based lunar sensor sim (arXiv:2410.04371).
- **Tonemapping (TonyMcMapface neutral, ACES contrast/hue-shift, AgX):** Bevy tonemapping docs; Bevy Cheatbook HDR & Tonemapping.
- **Bevy 0.18 IBL/probes (EnvironmentMapLight + IrradianceVolume exist, no procedural baker):** EnvironmentMapLight docs.rs; bevy#9380 (auto skybox→IBL gap); Lighting & Shadows (DeepWiki).
