//! Unit tests for lunco-materials crate.
//!
//! Integration tests run natively and read their WGSL fixtures off disk. The
//! workspace `disallowed_methods` ban on `std::fs` guards *wasm runtime* code
//! paths, not `tests/` — `clippy.toml`'s header already says so; cargo has no
//! path-scoped lint config, so the exemption has to be written out.
#![allow(clippy::disallowed_methods)]

//!
//! The `ShaderMaterial` packing test moved with the material itself, into
//! `lunco-render-bevy` — this crate is render-free and no longer names it.

use lunco_materials::{ParamSchema, ParamType};
use std::path::Path;

/// The blueprint grid is now the self-describing `blueprint.wgsl` driven by
/// `ShaderMaterial` (no bespoke `BlueprintExtension` type). Its `Material` struct
/// must reflect so its grid knobs pack at the right std140 offsets.
#[test]
fn test_blueprint_shader_schema_reflects() {
    let wgsl = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/shaders/blueprint.wgsl"
    ))
    .expect("blueprint.wgsl present");
    let schema = ParamSchema::parse(&wgsl).expect("blueprint Material struct reflects");
    // A few representative fields must be present with the right types.
    assert_eq!(
        schema.field("surface_color").map(|f| f.ty),
        Some(ParamType::Vec3)
    );
    assert_eq!(
        schema.field("transition").map(|f| f.ty),
        Some(ParamType::F32)
    );
    assert_eq!(
        schema.field("major_grid_spacing").map(|f| f.ty),
        Some(ParamType::F32)
    );
    assert_eq!(
        schema.field("blueprint_origin").map(|f| f.ty),
        Some(ParamType::Vec3)
    );
    assert_eq!(
        schema.field("blueprint_frame_origin").map(|f| f.ty),
        Some(ParamType::Vec3)
    );
    assert_eq!(
        schema.field("blueprint_frame_rotation").map(|f| f.ty),
        Some(ParamType::Vec4)
    );
    // Whole block stays within the 256-byte uniform budget.
    assert!(
        schema.size <= 256,
        "blueprint params overflow uniform block: {}",
        schema.size
    );
}

/// Globe imagery coordinates come from one tessellation-independent producer:
/// the body-fixed direction authored by the globe mesh and carried by its vertex
/// stage. Re-introducing vertex-projected equirectangular UV makes parent/child
/// LODs sample different lunar locations and recreates square/diagonal patches.
#[test]
fn blueprint_globe_mapping_is_derived_per_fragment_from_body_direction() {
    let shader_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders");
    let fragment =
        std::fs::read_to_string(shader_dir.join("blueprint.wgsl")).expect("blueprint shader");

    assert!(fragment.contains("@location(11) globe_direction: vec3<f32>"));
    assert!(fragment.contains("@location(5) globe_direction: vec3<f32>"));
    assert!(fragment.contains("equirectangular_uv(globe_direction_unit)"));
    assert!(fragment.contains("equirectangular_grad(globe_direction_unit"));
    assert!(fragment.contains("textureSampleGrad(albedo_tex"));
    assert!(
        !fragment.contains("let globe_uv = in.uv"),
        "projecting spherical UV at vertices makes imagery depend on LOD tessellation"
    );
    assert!(
        fragment.contains("stable_world = in.world_position.xyz + mat.blueprint_origin"),
        "flat blueprint coordinates must restore the stable WorldGrid origin"
    );
    assert!(
        fragment.contains("stable_world - mat.blueprint_frame_origin"),
        "flat blueprint coordinates must map into the authored terrain frame"
    );
    assert!(
        !fragment.contains("north_cap") && !fragment.contains("south_cap"),
        "a broad polar colour override is a view-dependent workaround, not projection math"
    );
}

/// Verifies that `solar_panel.wgsl`'s `Material` struct correctly reflects the
/// newly introduced `seamless_u` and `v_scale` parameters.
#[test]
fn test_solar_panel_shader_reflects_seamless_u_and_v_scale() {
    let wgsl = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/shaders/solar_panel.wgsl"
    ))
    .expect("solar_panel.wgsl present");
    let schema = ParamSchema::parse(&wgsl).expect("solar_panel Material struct reflects");
    assert_eq!(
        schema.field("seamless_u").map(|f| f.ty),
        Some(ParamType::F32)
    );
    assert_eq!(schema.field("v_scale").map(|f| f.ty), Some(ParamType::F32));
    assert!(
        schema.size <= 256,
        "solar_panel params overflow uniform block: {}",
        schema.size
    );
}

/// Every terrain shader that ray-marches the heightfield now declares the
/// `shadow_cache_on` engine field (the uniform flag that selects the pre-baked
/// shadow cache lookup vs. the live march) and must still reflect within the
/// 256-byte uniform budget after the cache wiring landed. This guards both the
/// schema reflection and the uniform-block overflow check for all five
/// terrain shaders that import `lunco::horizon`.
#[test]
fn test_terrain_shaders_reflect_shadow_cache_on() {
    // The `_web` twins are gone: one file per family now, with the native/web
    // difference behind `LUNCO_NOISE_2D` in `lunco::terrain`.
    let shaders = [
        "regolith.wgsl",
        "terrain_shadow.wgsl",
        "terrain_layered.wgsl",
    ];
    for name in shaders {
        let wgsl = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/shaders")
                .join(name),
        )
        .unwrap_or_else(|_| panic!("{name} present"));
        let schema =
            ParamSchema::parse(&wgsl).unwrap_or_else(|| panic!("{name} Material struct reflects"));
        assert_eq!(
            schema.field("shadow_cache_on").map(|f| f.ty),
            Some(ParamType::F32),
            "{name} reflects shadow_cache_on as f32"
        );
        assert!(
            schema.is_engine("shadow_cache_on"),
            "{name} marks shadow_cache_on as an engine field"
        );
        assert_eq!(
            schema.field("horizon_march_steps").map(|f| f.ty),
            Some(ParamType::F32),
            "{name} reflects horizon_march_steps as f32"
        );
        assert!(
            schema.is_engine("horizon_march_steps"),
            "{name} marks horizon_march_steps as an engine field"
        );
        assert!(
            schema.size <= 256,
            "{name} params overflow uniform block: {}",
            schema.size
        );
    }
}

/// BOTH terrain render paths must reflect the authored-layer weights under the
/// SAME names, because one authored scene feeds both: `terrain_layered.wgsl`
/// draws a static-mesh site and `terrain_geomorph.wgsl` draws a streamed
/// (`lodViz = true`) one.
///
/// This is a silent-failure guard, not a formality. The binder sets these by
/// name (`set_param(look, "weight_albedo", …)`); a name that does not exist in
/// the shader's `Material` struct is simply dropped, so a rename or a typo
/// would not fail to compile, would not warn, and would show up only as a site
/// rendering procedural grey while its real orthophoto sat loaded in memory —
/// which is exactly the bug step 4 existed to fix.
#[test]
fn both_terrain_paths_reflect_the_authored_layer_weights() {
    for name in ["terrain_layered.wgsl", "terrain_geomorph.wgsl"] {
        let wgsl = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/shaders")
                .join(name),
        )
        .unwrap_or_else(|_| panic!("{name} present"));
        let schema =
            ParamSchema::parse(&wgsl).unwrap_or_else(|| panic!("{name} Material struct reflects"));
        for field in ["weight_albedo", "weight_mineral"] {
            assert_eq!(
                schema.field(field).map(|f| f.ty),
                Some(ParamType::F32),
                "{name} must reflect {field} as f32 — the layer binder sets it by this name"
            );
        }
        assert!(
            schema.size <= 256,
            "{name} params overflow uniform block: {}",
            schema.size
        );
    }
}
