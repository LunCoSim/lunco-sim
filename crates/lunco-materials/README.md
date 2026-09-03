# lunco-materials

LunCoSim's custom Bevy render materials, kept **engine-agnostic** (nothing here is
USD-specific). There is **one** general material — [`ShaderMaterial`] — and new
looks are pure `.wgsl` asset files, **no Rust per material**.

## Architecture

`ShaderMaterial` is a single self-describing material that runs **any** `.wgsl`,
chosen per-instance via its `shader: Handle<Shader>`:

- **Parameters are declared in the shader**, not in Rust. Each `.wgsl` declares a
  WGSL `struct Material { … }` plus `//!@ui` / `//!@default` annotation comments.
  The engine reflects them (`reflect_shader_schemas`) into a `ParamSchema`, so
  every field becomes a free Inspector slider and `SetObjectProperty` target —
  and the layout **hot-reloads** on shader edit. When the look is backed by USD,
  edits are authored as typed `inputs:<field>` attributes on the bound
  `UsdShade.Shader` prim.
- **Lighting** is opt-in via the shared `#import lunco::pbr_lit::lit` module (full
  Bevy PBR — directional sun, shadows, tonemapping) — no `StandardMaterial`
  inheritance, no hand-copied `PbrInput` boilerplate.
- **Discovery is automatic**: drop a `.wgsl` in `assets/shaders/` (or an open Twin)
  and the catalog scan (`maintain_catalogs` in `lunco-luncosim-edit`) picks it up.

> There are **no bespoke per-effect material types**. The old `SolarPanelMaterial`
> and `BlueprintMaterial` (hand-rolled `ExtendedMaterial`s) are gone — they are now
> `assets/shaders/solar_panel.wgsl` and `assets/shaders/blueprint.wgsl` driven by
> `ShaderMaterial`. Reach for a new `Material` type only when you need a render
> feature `ShaderMaterial` structurally cannot express (e.g. a brand-new bind-group
> layout); extend `ShaderMaterial` first.

## Adding a new material (the whole process)

### 1. Write the shader — `assets/shaders/my_material.wgsl`

```wgsl
#import bevy_pbr::forward_io::VertexOutput
#import lunco::pbr_lit::lit

//!@ui      base_color color "Base colour"
//!@default base_color 0.8,0.8,0.8
//!@ui      roughness  0 1   "Roughness"
//!@default roughness  0.6
struct Material {
    base_color: vec3<f32>,
    roughness:  f32,
}
@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> mat: Material;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    return lit(in, is_front, mat.base_color, mat.roughness, 0.0, vec3(0.0));
}
```

That's it — no Rust. Optional texture slots are available at fixed bindings
(`albedo_map` 2/3, `mineral_map` 4/5, `surface_map` 6/7, `normal_map` 8/9,
`height_map` 1); declare only the ones you sample.

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
                uniform token info:id = "UsdPreviewSurface"
                asset info:wgsl:sourceAsset = @lunco://shaders/my_material.wgsl@
                color3f inputs:diffuseColor = (0.2, 0.4, 0.9)
                float inputs:roughness = 0.3
            }
        }
    }
}
```

`apply_usd_shader_materials` (in `lunco-usd-sim`, deterministically ordered after
`sync_usd_visuals`) follows the composed `material:binding`, reads the bound
Shader's typed `inputs:*`, and packs values whose names match the WGSL `Material`
fields. Geometry `primvars:displayColor` remains the standard shared colour
input for shaders that opt into `//!@engine display_color`.

### 3. Or apply it from Rust

```rust
let mut m = ShaderMaterial::default();
m.shader = asset_server.load("shaders/my_material.wgsl");
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
├── src/
│   ├── lib.rs              # re-exports
│   ├── dyn_params.rs       # ParamSchema/ParamValue: WGSL `struct Material` reflection + std140 packing
│   └── shader_material.rs  # the one general ShaderMaterial + ShaderMaterialPlugin
└── tests/
    └── materials_test.rs   # dynamic packing + blueprint.wgsl schema reflection

Shaders live in `assets/shaders/*.wgsl` (e.g. blueprint.wgsl, solar_panel.wgsl,
regolith.wgsl, terrain_layered.wgsl, wheel.wgsl) — pure assets, hot-reloaded.
```

The 256-byte uniform block caps all params (named + engine-filled) at 64 f32 lanes;
supported types are `f32 / i32 / u32 / vec2 / vec3 / vec4` (std140).
