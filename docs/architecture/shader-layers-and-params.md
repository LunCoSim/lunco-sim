# Shader looks: dynamic parameters and texture layers

How to give something a custom shader look in LunCoSim — what a parameter is, what a
texture layer is, when to spend a **channel** instead of a **slot**, and how to get
higher quality in some places than others.

Companion to [`render-decoupling.md`](render-decoupling.md), which explains *why*
appearance is stated as data instead of as a material.

---

## The two halves

| | what it is | where it lives | how many |
|---|---|---|---|
| **Parameter** | a scalar/vector uniform the shader reads | one opaque 256-byte uniform block | ~64 `vec4`s — effectively unlimited |
| **Texture layer** | a raster the shader samples | a bind-group entry | **6 named slots** — a real budget |

Parameters are cheap and open-ended. **Texture slots are the scarce resource.** Most
"I need another layer" instincts should be spent on a *channel*, not a slot — see
[Spend a channel before a slot](#spend-a-channel-before-a-slot).

---

## Parameters are an open set — Rust hardcodes none of them

A shader declares its own parameters in its `struct Material` block, with ranges,
defaults and widget hints in `//!@` annotations. Rust reflects that into a
`ParamSchema` at load time. **Adding a parameter means editing a `.wgsl` file. It
never means editing Rust**, and the Inspector picks it up automatically, because it
derives its sliders from the schema instead of a hand-written list.

```wgsl
// assets/shaders/my_look.wgsl
struct Material {
    //!@ range(0.0, 1.0) default(0.35) name("Dust density")
    dust_density: f32,
    //!@ range(0.0, 64.0) default(8.0)
    clump_scale: f32,
    tint: vec4<f32>,
};
@group(2) @binding(0) var<uniform> material: Material;
```

Set them from Rust by **name**:

```rust
use lunco_materials::{ShaderLook, ParamValue, TextureLayer};

let look = ShaderLook::new("shaders/my_look.wgsl")
    .with("dust_density", ParamValue::F32(0.35))
    .with("clump_scale",  ParamValue::F32(8.0))
    .with("tint",         ParamValue::Color(Vec4::new(0.6, 0.6, 0.62, 1.0)));

commands.spawn((Mesh3d(mesh), look, Transform::from_translation(p)));
```

That is the whole API. You never touch `Assets<StandardMaterial>`, and the crate you
write this in does not link `bevy_pbr`. `lunco-render-bevy` binds it.

The uniform block is deliberately **opaque**: the same 256 bytes are reinterpreted by
each shader through its own `struct Material`. That is what makes the parameter set a
property of the *asset*, not of the engine. A name that is not in the schema is
dropped at pack time with a warning — it is never silently mis-packed into a
neighbouring field.

### Sharing, and how to not destroy it

Identical looks share **one** material and **one** bind group. The binder caches by
`ShaderLook::key()`, which quantises floats to 1e-4 so a rounding error cannot
silently mint a second material.

> **Vary a parameter per-instance and you get a material per instance.** That is a
> draw call and a bind group each. If instances must differ, **bucket** the value
> first — snap it to a small set — the way the terrain LOD path buckets its morph
> band and the rock scatter buckets its radius.

This is the single easiest way to accidentally destroy batching, and it is why the
key exists.

---

## Texture layers: six named slots

```rust
pub enum TextureLayer {
    Height,       // R32Float world heights (non-filterable) — ray-marched sun shadows
    Albedo,       // colour raster, e.g. the NASA lunar mosaic
    Mineral,      // class-id / composition raster, tinted through a palette LUT
    Surface,      // PACKED SCALARS: R=roughness G=ambient-occlusion B=rock-density A=hazard
    Normal,       // tangent/world-space normals — DEM relief the procedural FBM can't carry
    ShadowCache,  // pre-baked sun visibility (R8Unorm)
}
```

```rust
let moon = ShaderLook::new("shaders/terrain_geomorph.wgsl")
    .with_texture(TextureLayer::Albedo,  colour_mosaic)
    .with_texture(TextureLayer::Normal,  dem_normals)
    .with_texture(TextureLayer::Surface, packed_scalars)
    .with("dust_scale", ParamValue::F32(0.004));
```

The shader merges them. A shader that does not declare a binding is unaffected by
that layer being set (`None` binds Bevy's fallback image), so **one slot set serves
every shader** — you do not get a new material type per look.

---

## Spend a channel before a slot

**`Surface` is one texture carrying four independent scalar layers**:

| channel | layer |
|---|---|
| R | roughness |
| G | ambient occlusion |
| B | rock density |
| A | hazard |

That is the pattern. **A new *scalar* layer should take a spare channel, not a new
slot.** Four scalars for one binding is a 4× win on the scarce resource, and it costs
one texture fetch instead of four.

Reach for a new slot only when the layer is genuinely not a scalar — a colour, a
normal, something with its own format or filtering rules (`Height` is a slot precisely
because R32Float is non-filterable and cannot be packed with filterable data).

---

## Higher quality in some places than others

**This already works, and it needs no new machinery.**

Terrain is tiled (CDLOD), and **each tile carries its own `ShaderLook`, hence its own
texture handles**. So per-place quality is just: give the near tile a 2048² albedo and
the far tile a 256² one. Same six slots, different images, per tile. The `ShaderLook`
key includes the texture `AssetId`s, so the two tiles correctly get two materials —
and tiles at the same quality still share one.

That is the answer for "higher quality near the rover" and for "this landing site has
a real orbital raster, the rest of the Moon does not."

---

## When six slots really are not enough

In order of cost. Do not skip ahead.

1. **Channel-pack.** See above. Almost always the right answer for a scalar.

2. **Add a slot.** Six is a *conservative choice, not a hardware wall.* WebGL2/GLES3
   guarantees ≥16 sampled textures per fragment stage; Bevy's view and mesh bind
   groups consume some, leaving roughly 10–12 usable. Adding a named slot is a
   one-line change to `TextureLayer` plus a binding in `ShaderMaterial`, and every
   existing shader ignores it unless it declares the binding.

3. **`texture_2d_array`.** One binding, N layers, and `sampler2DArray` is core in
   GLES3 so it is WebGL2-safe. This is the right move for **many layers of the same
   kind** (a stack of mineral classes, a set of overlay masks): it converts the budget
   from *per-layer* into *per-kind*. Constraint: every layer must share format and
   resolution.

4. **Virtual texturing / clipmap.** A fixed-size physical texture cache plus an
   indirection table, streaming tiles at the resolution the view actually needs. This
   is what planetary renderers do and it is the principled answer to "arbitrary
   quality anywhere." It is also a serious piece of work with its own streaming,
   eviction and mip-seam problems. Do not reach for it until 1–3 are exhausted.

---

## Do NOT bake overlays into a texture

It is tempting to "just generate one texture with all the overlays merged in." **This
codebase has already decided against it, on purpose.**

`pack_surface_rgba8` writes `A = 255` and its doc says the hazard is *deliberately a
view, not baked data*. The reason is live re-tuning: `SetTerrainOverlay { cliff_deg }`
must change a **uniform**, not trigger a re-bake of every resident tile. Bake the
overlay and a slider drag becomes a multi-second stall.

The rule that falls out:

> **Bake the DATA. Derive the VIEW.**
>
> Normals, heights, DEM-derived slope: bake — they are expensive and they do not
> change when a user drags a slider.
> Hazard colours, traversability bands, science-zone tints: derive in the shader from
> a data layer, gated by uniforms.

The review's `D2` finding is the same lesson from the other direction: the overlay was
shading from the *LOD mesh normal*, so a 35° cliff read as safe on a coarse tile and
re-coloured as you approached. The fix was to derive it from the baked **normal map**
— the data layer — not to bake the **hazard** — the view.
