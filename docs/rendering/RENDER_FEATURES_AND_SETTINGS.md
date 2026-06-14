# Render Features & Settings — Roadmap + Realtime Control + Settings Window Design

**Scope:** what rendering features `lunco` should add, how to store them durably, and how to drive every one of them in realtime via the existing command bus and a new egui settings window.

**Project facts that shape every decision:**

- Bevy **0.18.1**, egui, big-space floating origin, lunar-surface scale, **WASM web + native**.
- The web build enables the Bevy **`webgl2`** feature (`Cargo.toml:108`), **not `webgpu`**. WebGL2 has **no compute shaders** → SSAO/GTAO, AutoExposure, Atmosphere, SSR are hard-OFF on web. This is the single most important platform gate in this document.
- 0.18 moved post/AA into two new crates that the codebase already references (`bevy::post_process::bloom::Bloom` is used in `big_space_setup.rs`): **`bevy_anti_alias`** (`taa/`, `smaa/`, `fxaa/`, `contrast_adaptive_sharpening/`, `dlss/`) and **`bevy_post_process`** (`bloom/`, `auto_exposure/`, `dof/`, `motion_blur/`). The reorg is not a blocker.
- Already present: **Bloom** (`intensity 0.4`, lf_boost 0.5, prefilter threshold 2.0, EnergyConserving), **TonyMcMapface** tonemapping, **HDR**, CSM shadows (4 cascades, max 1500 m, first bound 40 m), `DirectionalLightShadowMap 4096`, depth bias 0.06 / normal bias 2.5, custom heightfield ray-march terrain shadows, `SetEnvironmentLight` command.
- Already absent: **any AA** beyond raw, **Exposure/EV control**, **Skybox/stars/Earth**, **EnvironmentMapLight (IBL)**, SSAO/GTAO, SSR, DoF, sun disc/glare.

---

## 1. TL;DR — what to add, ranked (airless, high-contrast scene)

A lunar scene is the *worst case* for aliasing: a razor-sharp terminator, a crawling bright limb against pure black, and high-frequency regolith shader noise. Nothing softens it (no atmosphere, no fog). So:

| Rank | Feature | Why it wins here | Effort | Web? |
|---|---|---|---|---|
| **1** | **Anti-aliasing (SMAA + FXAA baseline, MSAA fallback, TAA native-experimental)** | Kills terminator/limb shimmer — the single biggest visible defect. Cross-platform. | Low | Yes (SMAA/FXAA/MSAA) |
| **2** | **Manual Exposure (EV100)** | One physically-grounded knob to stop full-sun blow-out into the bloom prefilter. Free uniform, WebGL2-safe. | Trivial | Yes |
| **3** | **Skybox + star field (+ Earth disc)** | Black void reads as "missing", not "space". Cubemap → instantly space. | Low–Med | Yes |
| **4** | **EnvironmentMapLight (IBL earthshine fill)** | Lunar shadows are near-black; a low-intensity IBL gives shadow-fill + spec on props, far better than flat ambient. | Med | Yes |
| **5** | **SSAO/GTAO** | Contact darkening in crevices/under rover. **Native only** (compute). Pair with TAA (noisy alone). | Med | **No** |
| **6** | **CAS sharpen** | Cheap crispness after AA softening. | Trivial | Yes |
| **7** | **Sun disc + glare** | Small emissive billboard at sun dir; let existing Bloom make the glare. No built-in in 0.18. | Low | Yes |
| **8** | **Wireframe + debug overlays** | Dev aid. | Trivial | Yes |

**Reject:** **Atmosphere** (compute-gated AND models air the Moon doesn't have — wrong physics), **SSR** (compute, no web, matte regolith barely reflects), **AutoExposure** (compute histogram, no web — manual EV covers 90%), **DoF/MotionBlur** (cinematic, need prepass), **DistanceFog** (airless → no atmospheric fog).

---

## 2. Feature roadmap (Bevy 0.18.1 verified API)

All types/paths below were read from the installed Bevy 0.18.1 source. `Native` = Vulkan/DX12/Metal; `WebGL2` = this web build.

| Feature | Bevy 0.18 API (verified) | Native | WebGL2 | Pri | Effort | 1-line wiring |
|---|---|---|---|---|---|---|
| **MSAA** | `bevy_render::view::Msaa` (per-camera component), `Msaa::Sample4` | yes | yes | P0 | Trivial | Insert `Msaa::Sample4` on camera (default for RTT preview) |
| **FXAA** | `bevy_anti_alias::fxaa::Fxaa { edge_threshold: Sensitivity }`, `FxaaPlugin` | yes | **yes** | P0 | Low | Add `AntiAliasPlugin`; insert `Fxaa::default()` (cheapest cross-platform fallback) |
| **SMAA** | `bevy_anti_alias::smaa::Smaa { preset }`, `SmaaPreset::{Low,Medium,High,Ultra}`, `SmaaPlugin` (KTX2 LUTs) | yes | **yes** | P0 | Low | Insert `Smaa { preset: SmaaPreset::High }` — the cross-platform quality default |
| **TAA** | `bevy_anti_alias::taa::TemporalAntiAliasing { reset }`, `#[require(TemporalJitter, MipBias, DepthPrepass, MotionVectorPrepass)]` | yes | partial | P0* | Med | Native-only "High" opt-in. **Ghosts on custom shaders** (no motion vectors) — experimental |
| **CAS** | `bevy_anti_alias::contrast_adaptive_sharpening::ContrastAdaptiveSharpening` | yes | yes | P2 | Trivial | Insert after AA; toggle + strength |
| **DLSS** | `bevy_anti_alias::dlss::Dlss<F>` | NVIDIA + `dlss` feat | no | — | — | Out of scope |
| **Bloom** | `bevy::post_process::bloom::Bloom` (**already used**) | yes | **yes** (explicit WebGL2 fallback in `bloom/mod.rs`) | done | — | Surface `intensity / low_frequency_boost / prefilter / composite_mode` |
| **Tonemapping** | `bevy::core_pipeline::tonemapping::Tonemapping` (**already = TonyMcMapface**) | yes | yes | done | — | Dropdown: TonyMcMapface / AgX / Reinhard / None |
| **Exposure** | `bevy_camera::camera::Exposure { ev100 }`; consts `SUNLIGHT=15, OVERCAST=12, INDOOR=7, BLENDER=9.7`; `from_physical_camera()` | yes | **yes** | P0 | Trivial | Insert `Exposure { ev100: 15.0 }` on avatar camera; EV slider |
| **AutoExposure** | `bevy_post_process::auto_exposure::AutoExposure { range, filter, speed_*, metering_mask, compensation_curve }` | yes | **NO** (compute histogram) | P2 | Med | Native-only; grey on web |
| **EnvironmentMapLight (IBL)** | `bevy_light::probe::EnvironmentMapLight { diffuse_map, specular_map, intensity, rotation, affects_lightmapped_mesh_diffuse }` | yes | yes | P1 | Med | Pre-filtered KTX2 diffuse+specular cubemaps; drive `intensity` from earthshine knob |
| **Skybox** | `bevy_core_pipeline::skybox::Skybox { image, brightness, rotation }`, `SkyboxPlugin` | yes | yes | P1 | Low–Med | KTX2 star cubemap; low `brightness` so stars don't bloom |
| **SSAO/GTAO** | `bevy_pbr::ssao::ScreenSpaceAmbientOcclusion { quality_level, constant_object_thickness }`, `QualityLevel::{Low..Ultra,Custom}`, `#[require(DepthPrepass, NormalPrepass)]` | **yes** | **NO** (doc: "not supported on WebGL2") | P2 | Med | Native-only; custom terrain must join normal/depth prepass to be occluded |
| **SSR** | `bevy_pbr::ssr::ScreenSpaceReflections`, `ScreenSpaceReflectionsPlugin` | yes | no | reject | — | Matte regolith → near-zero payoff |
| **DepthOfField** | `bevy_post_process::dof::DepthOfField`, `DepthOfFieldMode::{Gaussian,Bokeh}` | yes | Gaussian | defer | — | Screenshot nicety only |
| **MotionBlur** | `bevy_post_process::motion_blur::MotionBlur` | yes | partial | defer | — | Needs motion prepass |
| **Atmosphere** | `bevy_pbr::atmosphere::{Atmosphere, AtmosphereSettings}`, `AtmospherePlugin` | yes | **NO** (`finish()` warns no compute) | **reject** | — | Wrong physics (Rayleigh/Mie air) for an airless Moon |
| **Wireframe** | `bevy_pbr::wireframe::{WireframePlugin, Wireframe, WireframeConfig { global, default_color }}` | yes | yes | P2 | Trivial | Global toggle + per-entity via `SelectEntity` |
| **Cascade viz** | none built-in in 0.18 | — | — | TODO | — | Custom debug shader; defer |
| **Sun disc/glare** | none built-in | yes | yes | P2 | Low | Emissive billboard at sun dir; existing Bloom → glare |

\* **TAA is P0 in importance but native-only + experimental in practice** — see gotchas (§4).

### Recommended quality tiers (the preset dropdown)

- **Web / Low (WebGL2):** MSAA×4 or FXAA · Bloom · TonyMcMapface · manual Exposure · Skybox · IBL. *(no SSAO/AutoExposure/TAA)*
- **Web / Medium:** SMAA High instead of MSAA.
- **Native / High:** SMAA High (or TAA experimental) · Bloom · Exposure · Skybox · IBL · CAS.
- **Native / Ultra:** TAA · SSAO · CAS · IBL · Skybox (full compute stack).

---

## 3. State model — `RenderSettings` resource (persisted)

A single resource implements the project's `SettingsSection` trait (`crates/lunco-settings/src/lib.rs`) with `const KEY = "render"`, so it auto-persists to `~/.lunco/settings.json` (native) / `localStorage` (wasm) via `settings_flush_system` on any mutation. The trait requires `Resource + Serialize + DeserializeOwned + Default + Clone + PartialEq`.

```rust
// crates/lunco-render-settings/src/lib.rs  (new crate, or fold into lunco-environment)
use bevy::prelude::*;
use serde::{Serialize, Deserialize};
use lunco_settings::SettingsSection;

#[derive(Resource, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]                 // unknown/missing keys -> field default (forward-compat)
pub struct RenderSettings {
    pub shadows:   ShadowSettings,
    pub aa:        AaSettings,
    pub exposure:  ExposureSettings,
    pub bloom:     BloomSettings,
    pub environ:   EnvironSettings,   // sun / earthshine / ambient / skybox
    pub ao:        AoSettings,         // native-only
    pub debug:     DebugSettings,
    pub preset:    QualityPreset,      // last applied top-level preset (or Custom)
}

impl SettingsSection for RenderSettings {
    const KEY: &'static str = "render";
}

impl Default for RenderSettings {
    fn default() -> Self {
        // Default = "Native / High" knobs that are also safe on web
        // (web-incompatible toggles default OFF so a fresh web user is valid).
        Self {
            shadows:  ShadowSettings::default(),
            aa:       AaSettings::default(),
            exposure: ExposureSettings::default(),
            bloom:    BloomSettings::default(),
            environ:  EnvironSettings::default(),
            ao:       AoSettings::default(),
            debug:    DebugSettings::default(),
            preset:   QualityPreset::Custom,
        }
    }
}
```

### 3.1 Shadows  — maps to `light.rs` CSM config + custom heightfield path

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ShadowSettings {
    pub enabled:            bool,   // default true   -> DirectionalLight.shadows_enabled
    pub map_size:           u32,    // default 4096   {1024,2048,4096} -> DirectionalLightShadowMap.size
    pub cascade_count:      u8,     // default 4      [1..=4]  -> CascadeShadowConfigBuilder.num_cascades
    pub max_distance_m:     f32,    // default 1500.0 [50..=5000] -> CascadeShadowConfig maximum_distance
    pub first_cascade_m:    f32,    // default 40.0   [5..=500]   -> first_cascade_far_bound
    pub overlap:            f32,    // default 0.1    [0..=0.5]   -> overlap_proportion
    pub depth_bias:         f32,    // default 0.06   [0..=1]     -> shadow_depth_bias (acne)
    pub normal_bias:        f32,    // default 2.5    [0..=5]      -> shadow_normal_bias (acne)
    pub terrain_march:      bool,   // default true (native)  custom heightfield ray-march on/off
    pub csm_march_blend_m:  f32,    // default = csm_far*0.5  -> regolith/terrain_shadow `csm_far` uniform
}
```

### 3.2 Anti-aliasing  — mutually-exclusive camera component

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum AaMode {
    Off,
    Msaa2, Msaa4,
    Fxaa,
    #[default] SmaaHigh,            // SmaaLow/Med/High/Ultra
    SmaaLow, SmaaMedium, SmaaUltra,
    Taa,                           // native-only; greyed on web
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct AaSettings {
    pub mode:          AaMode,   // default SmaaHigh
    pub cas_enabled:   bool,     // default false  -> ContrastAdaptiveSharpening
    pub cas_strength:  f32,      // default 0.6  [0..=1]
}
```

### 3.3 Exposure / Tone

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum Tonemapper { None, Reinhard, AgX, TonyMcMapface, AcesFitted }
// -> bevy::core_pipeline::tonemapping::Tonemapping variants

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct ExposureSettings {
    pub ev100:        f32,         // default 15.0 (Exposure::SUNLIGHT) [5..=18] -> Exposure.ev100
    pub auto:         bool,        // default false (native-only) -> AutoExposure present/absent
    pub auto_min_ev:  f32,         // default 9.0   AutoExposure.range.start
    pub auto_max_ev:  f32,         // default 16.0  AutoExposure.range.end
    pub auto_speed:   f32,         // default 3.0   AutoExposure.speed_brighten/darken
    pub tonemapper:   Tonemapper,  // default TonyMcMapface
}
```

### 3.4 Bloom  — surfaces the existing `big_space_setup.rs` Bloom

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum BloomComposite { EnergyConserving, Additive }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct BloomSettings {
    pub intensity:           f32,   // default 0.4  [0..=1]   -> Bloom.intensity
    pub low_frequency_boost: f32,   // default 0.5  [0..=1]   -> Bloom.low_frequency_boost
    pub prefilter_threshold: f32,   // default 2.0  [0..=10]  -> Bloom.prefilter.threshold
    pub prefilter_softness:  f32,   // default 0.0  [0..=1]   -> Bloom.prefilter.threshold_softness
    pub composite:           BloomComposite, // default EnergyConserving -> Bloom.composite_mode
}
```

### 3.5 Environment (sun / earthshine / ambient / skybox)  — extends `SetEnvironmentLight`

The sun fields below already exist on the live `SetEnvironmentLight` command (`crates/lunco-environment/src/lib.rs`): `sun_yaw`, `sun_pitch`, `illuminance`, `sun_color`, `shadows_enabled`, `ambient_brightness` (+ the shadow bias/cascade fields). `RenderSettings` mirrors them so the *window* is the source of truth and round-trips through the command.

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct EnvironSettings {
    // --- Sun (existing SetEnvironmentLight) ---
    pub sun_yaw_deg:      f32,   // default 35.0  [0..=360]  -> sun_yaw (rad at the boundary)
    pub sun_pitch_deg:    f32,   // default -8.5  [-90..=90] -> sun_pitch (terminator angle, the shimmer driver)
    pub illuminance_lux:  f32,   // default 100_000 [0..=130_000] -> DirectionalLight.illuminance
    pub sun_color:        [f32; 3], // default [1,1,1] linear -> sun_color
    pub sun_angular_radius_deg: f32, // default 0.27 [0..=2] -> sun_tan_radius (regolith/horizon penumbra)
    // --- Ambient / earthshine / IBL ---
    pub ambient_lux:      f32,   // default 200.0 [0..=5000] -> GlobalAmbientLight.brightness
    pub earthshine:       f32,   // default 0.0   [0..=2]    -> EnvironmentMapLight.intensity (P1)
    pub ibl_rotation_deg: f32,   // default 0.0   [0..=360]  -> EnvironmentMapLight.rotation
    // --- Sky ---
    pub skybox_enabled:   bool,  // default true  -> Skybox present/absent
    pub skybox_brightness:f32,   // default 30.0  [0..=500] (low so stars don't bloom) -> Skybox.brightness
    pub stars:            bool,  // default true  (carried by the skybox cubemap)
    pub earth_disc:       bool,  // default true  (billboard / faint disc)
}
```

### 3.6 Ambient occlusion (native-only)

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum AoQuality { Low, Medium, High, Ultra }

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct AoSettings {
    pub enabled:           bool,      // default false (native-only; force-false on web)
    pub quality:           AoQuality, // default Medium -> ScreenSpaceAmbientOcclusion.quality_level
    pub object_thickness:  f32,       // default 0.25 [0..=1] -> constant_object_thickness
}
```

### 3.7 Debug

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct DebugSettings {
    pub wireframe_global:   bool,     // default false -> WireframeConfig.global
    pub wireframe_color:    [f32; 4], // default white -> WireframeConfig.default_color
    pub show_cascades:      bool,     // default false (TODO: custom overlay, no built-in 0.18)
    pub show_normals:       bool,     // default false (prepass viz, native)
}
```

### 3.8 Quality preset

```rust
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub enum QualityPreset { WebLow, WebMedium, NativeHigh, NativeUltra, #[default] Custom }
```

`Custom` is recorded whenever the user hand-tweaks any field after a preset; the preset functions just stamp groups of the fields above (see §5.3).

---

## 4. Realtime control — one command, every surface

The settings must be drivable identically from **egui, HTTP `/api/commands`, MCP, and script** — that is exactly what the project's `#[Command]` + `#[on_command]` pattern already gives (HTTP `POST /api/commands {command, params}`, discovery via `GET /api/schema`, same observer path for UI/HTTP/MCP/script). We add one general command plus keep the existing `SetEnvironmentLight`.

### 4.1 The command

`SetRenderSetting` is a *patch* command: every field is `Option`, `None` = keep current. This mirrors `SetEnvironmentLight`'s shape so partial updates from a slider work, and it serializes cleanly for HTTP/MCP.

```rust
use lunco_core::{Command, on_command, register_commands};

/// Patch one or more render settings live. Any `None` keeps current.
/// Round-trips through persistence: mutates `RenderSettings` -> the
/// settings crate flushes it; a follow-up system applies it to the
/// live render-world components.
///
/// Example: {"type":"SetRenderSetting","aa_mode":"SmaaHigh","ev100":14.0}
#[Command(default)]
pub struct SetRenderSetting {
    // Anti-aliasing
    pub aa_mode:        Option<AaMode>,
    pub cas_enabled:    Option<bool>,
    pub cas_strength:   Option<f32>,
    // Exposure / tone
    pub ev100:          Option<f32>,
    pub auto_exposure:  Option<bool>,
    pub tonemapper:     Option<Tonemapper>,
    // Bloom
    pub bloom_intensity:    Option<f32>,
    pub bloom_lf_boost:     Option<f32>,
    pub bloom_threshold:    Option<f32>,
    pub bloom_composite:    Option<BloomComposite>,
    // Shadows
    pub shadow_map_size:    Option<u32>,
    pub shadow_cascades:    Option<u8>,
    pub shadow_max_dist:    Option<f32>,
    pub shadow_depth_bias:  Option<f32>,
    pub shadow_normal_bias: Option<f32>,
    pub terrain_march:      Option<bool>,
    // AO (native-gated; silently ignored on web)
    pub ssao_enabled:   Option<bool>,
    pub ssao_quality:   Option<AoQuality>,
    // Sky / IBL
    pub skybox_enabled: Option<bool>,
    pub skybox_brightness: Option<f32>,
    pub earthshine:     Option<f32>,
    // Debug
    pub wireframe_global: Option<bool>,
    // Top-level: apply a whole preset at once
    pub preset:         Option<QualityPreset>,
}
```

> The Sun/Ambient group keeps using the existing **`SetEnvironmentLight`** (already wired to `DirectionalLight` + `GlobalAmbientLight` + CSM/bias in `on_set_environment_light`). The window just calls *that* command for those fields — no duplication. `SetRenderSetting` covers everything `SetEnvironmentLight` doesn't.

### 4.2 The observer — mutate resource, then apply to render world

Two-step on purpose: the command writes the **persisted `RenderSettings` resource** (which the settings crate flushes to disk automatically), and a separate change-detection system pushes the resource onto the **live Bevy render components**. This keeps "what is saved" and "what is rendering" in one place and means *loading at startup runs the exact same apply path*.

```rust
#[on_command(SetRenderSetting)]
fn on_set_render_setting(cmd: SetRenderSetting, mut rs: ResMut<RenderSettings>) {
    if let Some(p) = cmd.preset { apply_preset_to(&mut rs, p); rs.preset = p; }
    let mut touched_custom = false;
    macro_rules! set { ($opt:expr, $dst:expr) => {
        if let Some(v) = $opt { $dst = v; touched_custom = true; }
    }}
    set!(cmd.aa_mode,            rs.aa.mode);
    set!(cmd.cas_enabled,        rs.aa.cas_enabled);
    set!(cmd.cas_strength,       rs.aa.cas_strength);
    set!(cmd.ev100,              rs.exposure.ev100);
    set!(cmd.auto_exposure,      rs.exposure.auto);
    set!(cmd.tonemapper,         rs.exposure.tonemapper);
    set!(cmd.bloom_intensity,    rs.bloom.intensity);
    set!(cmd.bloom_lf_boost,     rs.bloom.low_frequency_boost);
    set!(cmd.bloom_threshold,    rs.bloom.prefilter_threshold);
    set!(cmd.bloom_composite,    rs.bloom.composite);
    set!(cmd.shadow_map_size,    rs.shadows.map_size);
    set!(cmd.shadow_cascades,    rs.shadows.cascade_count);
    set!(cmd.shadow_max_dist,    rs.shadows.max_distance_m);
    set!(cmd.shadow_depth_bias,  rs.shadows.depth_bias);
    set!(cmd.shadow_normal_bias, rs.shadows.normal_bias);
    set!(cmd.terrain_march,      rs.shadows.terrain_march);
    set!(cmd.ssao_enabled,       rs.ao.enabled);
    set!(cmd.ssao_quality,       rs.ao.quality);
    set!(cmd.skybox_enabled,     rs.environ.skybox_enabled);
    set!(cmd.skybox_brightness,  rs.environ.skybox_brightness);
    set!(cmd.earthshine,         rs.environ.earthshine);
    set!(cmd.wireframe_global,   rs.debug.wireframe_global);
    if touched_custom && cmd.preset.is_none() { rs.preset = QualityPreset::Custom; }
    // No direct component writes here. `apply_render_settings` (below) reacts
    // to the resource change next frame and applies to the render world.
    // Mutating `rs` also marks the SettingsSection dirty -> auto-persisted.
}
register_commands!(on_set_render_setting);
```

```rust
/// The single apply path. Runs on `Changed`/startup. Native-gated arms
/// are no-ops under WebGL2 so a saved native preset never panics on web.
fn apply_render_settings(
    rs: Res<RenderSettings>,
    cams: Query<Entity, With<lunco_core::Avatar>>,    // main camera only (NOT loose Camera3d — multi-cam scene)
    mut commands: Commands,
    mut bloom_q: Query<&mut Bloom>,
    mut exposure_q: Query<&mut Exposure>,
    mut tonemap_q: Query<&mut Tonemapping>,
    mut sun_q: Query<&mut DirectionalLight>,
    mut shadow_map: Option<ResMut<DirectionalLightShadowMap>>,
    mut wire_cfg: Option<ResMut<WireframeConfig>>,
    is_web: Res<WebGl2Gate>,                            // runtime adapter check, set at startup
) {
    if !rs.is_changed() { return; }
    let cam = cams.iter().next();

    // --- Bloom (web-safe) ---
    if let Ok(mut b) = bloom_q.single_mut() {
        b.intensity = rs.bloom.intensity;
        b.low_frequency_boost = rs.bloom.low_frequency_boost;
        b.prefilter.threshold = rs.bloom.prefilter_threshold;
        b.composite_mode = match rs.bloom.composite { /* ... */ };
    }
    // --- Exposure (web-safe) ---
    if let Ok(mut e) = exposure_q.single_mut() { e.ev100 = rs.exposure.ev100; }
    if let Ok(mut t) = tonemap_q.single_mut() { *t = rs.exposure.tonemapper.into(); }

    // --- Anti-aliasing: mutually-exclusive component swap on the avatar cam ---
    if let Some(cam) = cam {
        let mut ec = commands.entity(cam);
        ec.remove::<(Fxaa, Smaa, TemporalAntiAliasing)>();   // clear all AA
        match rs.aa.mode {
            AaMode::Off        => { ec.insert(Msaa::Off); }
            AaMode::Msaa2      => { ec.insert(Msaa::Sample2); }
            AaMode::Msaa4      => { ec.insert(Msaa::Sample4); }
            AaMode::Fxaa       => { ec.insert((Msaa::Off, Fxaa::default())); }
            AaMode::SmaaLow    => { ec.insert((Msaa::Off, Smaa { preset: SmaaPreset::Low })); }
            AaMode::SmaaMedium => { ec.insert((Msaa::Off, Smaa { preset: SmaaPreset::Medium })); }
            AaMode::SmaaHigh   => { ec.insert((Msaa::Off, Smaa { preset: SmaaPreset::High })); }
            AaMode::SmaaUltra  => { ec.insert((Msaa::Off, Smaa { preset: SmaaPreset::Ultra })); }
            AaMode::Taa if !is_web.0 => { ec.insert((Msaa::Off, TemporalAntiAliasing::default())); }
            AaMode::Taa        => { ec.insert((Msaa::Off, Smaa { preset: SmaaPreset::High })); } // web fallback
        }
        // CAS
        ec.remove::<ContrastAdaptiveSharpening>();
        if rs.aa.cas_enabled {
            ec.insert(ContrastAdaptiveSharpening { sharpening_strength: rs.aa.cas_strength, ..default() });
        }
    }

    // --- Shadows ---
    if let Some(sm) = shadow_map.as_mut() { sm.size = rs.shadows.map_size as usize; }
    if let Ok(mut sun) = sun_q.single_mut() { sun.shadows_enabled = rs.shadows.enabled; /* bias via CSM rebuild */ }

    // --- SSAO (NATIVE ONLY) ---
    if let Some(cam) = cam {
        let mut ec = commands.entity(cam);
        ec.remove::<ScreenSpaceAmbientOcclusion>();
        if rs.ao.enabled && !is_web.0 {
            ec.insert(ScreenSpaceAmbientOcclusion {
                quality_level: rs.ao.quality.into(),
                constant_object_thickness: rs.ao.object_thickness,
            });
        }
    }

    // --- Wireframe ---
    if let Some(cfg) = wire_cfg.as_mut() { cfg.global = rs.debug.wireframe_global; }

    // --- Skybox / IBL: spawn/despawn/patch components on the camera (web-safe) ---
    // (omitted: insert Skybox{image,brightness} & EnvironmentMapLight{..,intensity:earthshine})
}
```

### 4.3 Round-trip with persistence

```
egui slider / HTTP / MCP / script
        │  (all four hit the same observer)
        ▼
  SetRenderSetting  ──on_command──►  mutate ResMut<RenderSettings>
        │                                   │
        │                                   ├──► settings_flush_system (dirty)  → ~/.lunco/settings.json / localStorage
        │                                   │
        │                                   └──► apply_render_settings (Changed) → live Bevy components (Bloom/Exposure/Msaa/SSAO/…)
        ▼
  HTTP 200 / UI reflects new resource value
```

Because **mutating the resource is the only write**, the same `apply_render_settings` runs at startup-load (§6), so disk → render is identical to command → render. No drift between "saved" and "showing".

---

## 5. Settings window design — `RenderSettingsPanel`

A new egui panel implementing the project `Panel` trait (`crates/lunco-workbench/src/panel.rs`): `id() / title() / default_slot() -> PanelSlot::RightInspector` (or `Floating`), `render(&mut self, ui, world)`. Immediate-mode, reads/writes `ResMut<RenderSettings>` and dispatches `SetRenderSetting` (or `SetEnvironmentLight` for the sun group) on change. WebGL2-unsupported controls are greyed via `ui.add_enabled(!is_web, …)`.

### 5.1 ASCII mockup

```
┌─ Render Settings ──────────────────────────────[×]┐
│ Quality preset:  [ Native / High      ▼ ]         │
│   Web-Low · Web-Med · Native-High · Native-Ultra  │
│                                  [ Reset to defaults ]
│                                                     │
│ ▼ Anti-Aliasing                                     │
│   Mode  [ SMAA High            ▼ ]                  │
│         Off·MSAA×2·MSAA×4·FXAA·SMAA L/M/H/U·TAA*    │
│   ( * TAA native-only — disabled on web )          │
│   [✓] CAS sharpen     Strength [====o----] 0.60    │
│                                                     │
│ ▼ Exposure & Tone                                   │
│   EV100        [=========o--]  15.0  (Sunlight)    │
│   [ ] Auto-exposure  (native only — greyed on web) │
│        min EV [9.0]  max EV [16.0]  speed [3.0]    │
│   Tonemapper [ TonyMcMapface   ▼ ]                  │
│                                                     │
│ ▼ Bloom                                             │
│   Intensity     [===o------]  0.40                 │
│   LF boost      [=====o----]  0.50                 │
│   Threshold     [==o-------]  2.00                 │
│   Composite [ EnergyConserving ▼ ]                 │
│                                                     │
│ ▼ Sun & Lighting        (→ SetEnvironmentLight)     │
│   Azimuth   [=====o----]  35°                      │
│   Elevation [===o------]  -8.5°  (terminator)      │
│   Illuminance [========o-] 100000 lx               │
│   Sun color  [■]  Angular radius [0.27°]           │
│   [✓] Sun casts shadows                             │
│                                                     │
│ ▼ Ambient / Earthshine / Sky                        │
│   Ambient    [=o--------]  200 lx                  │
│   Earthshine [==o-------]  0.30   (IBL fill)       │
│   IBL rotation [o---------] 0°                      │
│   [✓] Skybox   Brightness [=o-------] 30           │
│   [✓] Star field   [✓] Earth disc                  │
│                                                     │
│ ▼ Shadows                                           │
│   Map size  ( )1024 ( )2048 (•)4096                │
│   Cascades  [===o------]  4                        │
│   Max dist  [=====o----]  1500 m                   │
│   1st bound [=o--------]  40 m                     │
│   Depth bias  [o---------] 0.060                   │
│   Normal bias [==o-------] 2.50                    │
│   [✓] Terrain ray-march shadows   (native)         │
│   CSM↔march blend [===o------] 750 m               │
│                                                     │
│ ▼ Ambient Occlusion   (native only — greyed on web)│
│   [ ] SSAO    Quality [ Medium ▼ ]                 │
│   Object thickness [==o-------] 0.25               │
│                                                     │
│ ▼ Debug                                             │
│   [ ] Wireframe (global)   color [■]               │
│   [ ] Wireframe selected entity                    │
│   [ ] Show cascade splits  (TODO: custom overlay)  │
│   [ ] Show normals (prepass, native)               │
└─────────────────────────────────────────────────────┘
```

### 5.2 `render()` skeleton

```rust
pub struct RenderSettingsPanel;

impl Panel for RenderSettingsPanel {
    fn id(&self) -> PanelId { PanelId("render_settings") }
    fn title(&self) -> String { "Render Settings".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::RightInspector }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let is_web = world.resource::<WebGl2Gate>().0;
        // Take a working copy so we can diff -> dispatch only on real change.
        let before = world.resource::<RenderSettings>().clone();
        let mut rs = before.clone();

        // ── Quality preset ──────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Quality preset:");
            egui::ComboBox::from_id_salt("preset")
                .selected_text(format!("{:?}", rs.preset))
                .show_ui(ui, |ui| {
                    for p in [QualityPreset::WebLow, QualityPreset::WebMedium,
                              QualityPreset::NativeHigh, QualityPreset::NativeUltra] {
                        if ui.selectable_value(&mut rs.preset, p, format!("{p:?}")).clicked() {
                            apply_preset_to(&mut rs, p);   // stamp all groups
                        }
                    }
                });
            if ui.button("Reset to defaults").clicked() { rs = RenderSettings::default(); }
        });

        // ── Anti-Aliasing ───────────────────────────────────────────
        ui.collapsing("Anti-Aliasing", |ui| {
            egui::ComboBox::from_label("Mode")
                .selected_text(format!("{:?}", rs.aa.mode))
                .show_ui(ui, |ui| {
                    use AaMode::*;
                    for m in [Off, Msaa2, Msaa4, Fxaa, SmaaLow, SmaaMedium, SmaaHigh, SmaaUltra] {
                        ui.selectable_value(&mut rs.aa.mode, m, format!("{m:?}"));
                    }
                    ui.add_enabled_ui(!is_web, |ui| {            // TAA greyed on web
                        ui.selectable_value(&mut rs.aa.mode, Taa, "Taa (native)");
                    });
                });
            ui.checkbox(&mut rs.aa.cas_enabled, "CAS sharpen");
            ui.add_enabled(rs.aa.cas_enabled,
                egui::Slider::new(&mut rs.aa.cas_strength, 0.0..=1.0).text("Strength"));
        });

        // ── Exposure & Tone ─────────────────────────────────────────
        ui.collapsing("Exposure & Tone", |ui| {
            ui.add(egui::Slider::new(&mut rs.exposure.ev100, 5.0..=18.0).text("EV100"));
            ui.add_enabled(!is_web,                                  // AutoExposure native-only
                egui::Checkbox::new(&mut rs.exposure.auto, "Auto-exposure (native)"));
            // tonemapper combo ...
        });

        // ── Bloom / Sun / Ambient / Shadows / AO / Debug: same shape ──
        // (Sun group writes via SetEnvironmentLight instead of SetRenderSetting)
        // ...

        // ── Dispatch ONLY the changed fields ─────────────────────────
        if rs != before {
            let patch = diff_to_command(&before, &rs);     // build SetRenderSetting{ Some(..) only for changed }
            world.send_event(patch);                       // same bus as HTTP/MCP/script
            // Sun group fields route to SetEnvironmentLight similarly.
        }
    }
}
```

The panel never writes Bevy render components itself — it only emits the command, so the egui path is byte-identical to the HTTP/MCP path and persistence happens automatically.

### 5.3 Quality presets → field stamps

`apply_preset_to(rs, preset)` writes whole groups (the rest keep their values):

| Preset | AA | SSAO | AutoExp | Shadow map | TAA | Skybox/IBL |
|---|---|---|---|---|---|---|
| **Web-Low** | `Fxaa` (or `Msaa4`) | off | off | 2048 | off | on |
| **Web-Medium** | `SmaaHigh` | off | off | 4096 | off | on |
| **Native-High** | `SmaaHigh` | off | off | 4096 | off | on |
| **Native-Ultra** | `Taa` | on (High) | optional | 4096 | on | on |

On web, selecting a Native preset auto-downshifts the web-incompatible bits (`apply_preset_to` clamps `ssao=false`, `auto=false`, `Taa→SmaaHigh` when `is_web`), so a shared settings file or a copied HTTP command never produces an invalid web state.

---

## 6. Wiring checklist

```rust
// In the render-settings plugin's build():
app.add_plugins(lunco_settings::SettingsPlugin);          // (already added app-wide)
app.register_settings_section::<RenderSettings>();         // (1) persistence + load-on-startup

app.add_plugins(bevy_anti_alias::AntiAliasPlugin);         // (2) AA crate (FXAA/SMAA/TAA/CAS)
app.add_plugins(bevy_pbr::wireframe::WireframePlugin::default());
// Skybox/EnvironmentMapLight/Exposure/Bloom plugins ride DefaultPlugins.
// SsaoPlugin only added when !is_web (native).
#[cfg(not(target_arch = "wasm32"))]
app.add_plugins(bevy_pbr::ssao::ScreenSpaceAmbientOcclusionPlugin);

app.insert_resource(WebGl2Gate(detect_webgl2()));          // runtime adapter check

register_commands!(on_set_render_setting);                 // (3) command on UI/HTTP/MCP/script bus
//   discover_commands (lunco-api) auto-exposes it on GET /api/schema.

register_workbench_panel::<RenderSettingsPanel>(app);      // (4) the egui window

// (5) startup-load + apply: register_settings_section already loads the
// resource from disk at boot. Add the apply system so the loaded values
// reach the render world on the first frame and on every change:
app.add_systems(Update, apply_render_settings);
//   Runs once at startup (resource is "changed" after load) AND on every
//   command — single apply path, no native/web divergence in the call site.
```

**Camera-insert seam (where the components actually live):**
- Avatar/main camera — `crates/lunco-avatar/src/lib.rs` (camera spawn): insert `Exposure`, initial `Smaa`/`Msaa`, later `Ssao`/`Taa`/`Skybox`/`EnvironmentMapLight` via `apply_render_settings`. **Resolve the camera via `With<lunco_core::Avatar>`, never loose `Camera3d`** — the scene has 2 cameras (avatar + RTT preview) and writing to the wrong one is a known past bug.
- Bloom/Tonemapping already on the camera in `crates/lunco-celestial/src/big_space_setup.rs` (~lines 443–463) — `apply_render_settings` just mutates them.
- Shadows/CSM/bias — `crates/lunco-usd-bevy/src/light.rs` + the existing `SetEnvironmentLight` handler in `crates/lunco-environment/src/lib.rs`.
- RTT preview camera — `crates/lunco-usd/src/ui/viewport.rs` (order 1, layer 31): give it its own cheap `Msaa::Sample4` (+ matching `Exposure`) so it doesn't look different from the main view.
- Platform gate — `Cargo.toml:108` (`webgl2`); also keep a **runtime** adapter check (`WebGl2Gate`) so a native-saved preset opened on web greys/no-ops instead of panicking.

### Gotchas to honour (from the verification pass)

- **Web = WebGL2:** SSAO / AutoExposure / Atmosphere / SSR are unavailable on web — `#[cfg]` plugin registration + runtime `WebGl2Gate` greying. Never insert those components on a WebGL2 adapter.
- **Prepass cost is the hidden tax on TAA and SSAO:** TAA `#[require]`s `MotionVectorPrepass`; SSAO `#[require]`s `DepthPrepass + NormalPrepass`. The custom self-describing shaders (`regolith.wgsl`, `terrain_shadow.wgsl`, prop shaders) **do not write prepass/motion outputs today** → TAA ghosts on terrain/props and SSAO won't occlude them until those materials gain prepass support. Treat both as "needs prepass work first", not drop-in.
- **AA is mutually exclusive:** always `remove::<(Fxaa, Smaa, TemporalAntiAliasing)>()` before inserting the chosen one (and toggle `Msaa::Off` vs sample count). Leaving two on is undefined.
- **Bloom prefilter & EV interact:** raising `illuminance`/lowering `ev100` pushes more of the surface past the bloom prefilter `threshold` (2.0) — tune EV and threshold together, and keep `Skybox.brightness` low so stars stay below threshold.
- **Reject Atmosphere outright:** even where compute exists it models an atmosphere the Moon lacks. Sun disc = emissive billboard + existing Bloom instead.
- **Cross-platform "looks great" core** (all WebGL2-safe): Bloom · Tonemapping · Exposure · FXAA/SMAA/MSAA · CAS · Skybox · EnvironmentMapLight · Wireframe. Ship these everywhere; reserve SSAO/AutoExposure/TAA for the Native tiers.

---

## Appendix — relevant files

- `crates/lunco-celestial/src/big_space_setup.rs` — main camera; Bloom/Tonemapping already here (~443–463).
- `crates/lunco-avatar/src/lib.rs` — avatar `Camera3d` spawn (~466–480); add Exposure/AA here.
- `crates/lunco-environment/src/lib.rs` — `SetEnvironmentLight` command host (sun/ambient/CSM/bias).
- `crates/lunco-usd-bevy/src/light.rs` — `DistantLight→DirectionalLight`, CSM config, shadow biases, `DirectionalLightShadowMap`.
- `crates/lunco-usd/src/ui/viewport.rs` — RTT preview camera (order 1, layer 31).
- `crates/lunco-sandbox-edit/src/ui/inspector.rs` — existing Environment section (alternative host).
- `crates/lunco-settings/src/lib.rs` — `SettingsSection` trait + `register_settings_section`.
- `crates/lunco-workbench/src/panel.rs` — `Panel` trait + `PanelSlot` + `register_workbench_panel`.
- `crates/lunco-command-macro` / `lunco-api/src/discovery.rs` — `#[Command]`, `/api/commands`, `/api/schema`.
- `Cargo.toml:108` — the `webgl2` feature (the platform gate).
- `assets/shaders/{regolith,terrain_shadow,horizon_march}.wgsl` — custom shaders that need prepass support before TAA/SSAO are clean.
