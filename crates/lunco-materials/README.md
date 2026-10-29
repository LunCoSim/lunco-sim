# lunco-materials

LunCoSim's custom Bevy render materials, kept **engine-agnostic** (nothing here is
USD-specific). There is **one** general material — [`ShaderMaterial`] — and new
looks are pure `.wesl` asset files, **no Rust per material**.

## Architecture

`ShaderMaterial` is a single self-describing material that runs **any** `.wesl`,
chosen per-instance via its `shader: Handle<Shader>`:

- **Parameters are declared in the shader**, not in Rust. Each `.wesl` declares a
  WESL `struct Material { … }` plus `//!@ui` / `//!@default` annotation comments.
  The engine reflects them (`reflect_shader_schemas`) into a `ParamSchema`, so
  every field becomes a free Inspector slider and `SetObjectProperty` target —
  and the layout **hot-reloads** on shader edit. When the look is backed by USD,
  edits are authored as typed `inputs:<field>` attributes on the bound
  `UsdShade.Shader` prim.
- **Lighting** is opt-in via the shared `import lunco::shaders::pbr_lit::lit;` module (full
  Bevy PBR — directional sun, shadows, tonemapping) — no `StandardMaterial`
  inheritance, no hand-copied `PbrInput` boilerplate.
- **Discovery is automatic**: drop a `.wesl` in `assets/shaders/` (or an open Twin)
  and the catalog scan (`maintain_catalogs` in `lunco-scene-catalog`) picks it up.

> There are **no bespoke per-effect material types**. The old `SolarPanelMaterial`
> and `BlueprintMaterial` (hand-rolled `ExtendedMaterial`s) are gone — they are now
> `assets/shaders/solar_panel.wesl` and `assets/shaders/blueprint.wesl` driven by
> `ShaderMaterial`. Reach for a new `Material` type only when you need a render
> feature `ShaderMaterial` structurally cannot express (e.g. a brand-new bind-group
> layout); extend `ShaderMaterial` first.

## Adding a new material (the whole process)

### 1. Write the shader — `assets/shaders/my_material.wesl`

```wgsl
import bevy_pbr::render::forward_io::VertexOutput;
import lunco::shaders::pbr_lit::lit;

//!@ui      base_color color "Base colour"
//!@default base_color 0.8,0.8,0.8
//!@ui      roughness  0 1   "Roughness"
//!@default roughness  0.6
struct Material {
    base_color: vec3<f32>,
    roughness:  f32,
}
@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, vec3(0.0));
}
```

That's it — no Rust. Optional texture slots are available at fixed bindings
(`albedo_map` 2/3, `mineral_map` 4/5, `surface_map` 6/7, `normal_map` 8/9,
`height_map` 1). Optional repeatable material detail uses `detail_albedo_map`
17/18, `detail_normal_map` 19/20, and `detail_surface_map` 21/22. Detail normals
are tangent-space; the normal raster at 8/9 is object-local. Detail samplers
repeat and use color/normal/linear mip preparation for their respective roles.
Declare only the bindings you sample.

### 2. Apply it from USD

```usda
def Xform "World"
{
    def Cube "MyObject" (
        prepend apiSchemas = ["MaterialBindingAPI"]
    )
    {
        rel material:binding = </World/Looks/MyObject_Mat>
    }
    def Scope "Looks"
    {
        def Material "MyObject_Mat"
        {
            rel outputs:surface.connect = </World/Looks/MyObject_Mat/Surface.outputs:surface>
            def Shader "Surface"
            {
                uniform token info:implementationSource = "sourceAsset"
                asset info:wesl:sourceAsset = @lunco://shaders/my_material.wesl@
                color3f inputs:diffuseColor = (0.2, 0.4, 0.9)
                float inputs:roughness = 0.3
            }
        }
    }
}
```

`apply_usd_shader_materials` (in `lunco-usd-sim`, deterministically ordered after
`sync_usd_visuals`) follows the composed `material:binding`, reads the bound
Shader's typed `inputs:*`, and packs values whose names match the WESL `Material`
fields. Geometry `primvars:displayColor` remains the standard shared colour
input for shaders that opt into `//!@engine display_color`.

### 3. Or apply it from Rust

```rust
let mut m = ShaderMaterial::default();
m.shader = asset_server.load("shaders/my_material.wesl");
m.set_many([
    ("base_color", ParamValue::Vec3([0.2, 0.4, 0.9])),
    ("roughness",  ParamValue::F32(0.3)),
]);
let handle = shader_materials.add(m);   // needs ShaderMaterialPlugin registered
```

## Registering the pipeline

A binary that renders `ShaderMaterial` adds `ShaderMaterialPlugin` (registers
`MaterialPlugin::<ShaderMaterial>` + the schema-reflection system). USD authoring
is separate — `lunco-usd-sim`'s `UsdSimPlugin` — so a binary with the pipeline but
not `UsdSimPlugin` does not project the USD `UsdShade` shader network into a
`ShaderLook`.

## Crate structure

```
crates/lunco-materials/
└── src/
    ├── lib.rs              # re-exports
    ├── dyn_params.rs       # ParamSchema/ParamValue: WESL `struct Material` reflection + std140 packing
    └── shader_material.rs  # the one general ShaderMaterial + ShaderMaterialPlugin
```

Shaders live in `assets/shaders/*.wesl` (e.g. blueprint.wesl, solar_panel.wesl,
regolith.wesl, terrain_layered.wesl, wheel.wesl) — pure assets, hot-reloaded.
Their shipped schema and composed USD/render contracts are exercised by the
authored `assets/scenes/tests/shader_asset_contracts.usda` scene and its Rhai
observer. Rust tests in this crate use inline WESL only for the generic
reflection and packing mechanism; they do not open repository shader files.

The 512-byte uniform block caps all params (named + engine-filled) at 128 f32 lanes;
supported types are `f32 / i32 / u32 / vec2 / vec3 / vec4` (std140).
