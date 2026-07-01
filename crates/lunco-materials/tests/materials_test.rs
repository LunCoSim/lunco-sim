//! Unit tests for lunco-materials crate.

use lunco_materials::{ParamSchema, ParamType, ParamValue, ShaderMaterial};
use std::sync::Arc;

/// A fresh `ShaderMaterial` carries an empty schema and packs all-zero; once
/// its shader's `Material` struct is reflected, named values pack into the
/// opaque block at the reflected std140 offsets. (The solar panel is now
/// driven by this material + `shaders/solar_panel.wgsl`, not a bespoke type.)
#[test]
fn test_shader_material_dynamic_packing() {
    let m = ShaderMaterial::default();
    assert_eq!(m.raw[0].x, 0.0, "empty default packs all-zero");
    assert_eq!(m.raw[5].x, 0.0);

    // Reflect a schema from a shader's `Material` struct and apply params by name.
    let schema = ParamSchema::parse(
        "//!@ui albedo color \"Albedo\"\n\
         struct Material { albedo: vec3<f32>, sun_vis: f32 }\n\
         @group(2) @binding(0) var<uniform> mat: Material;",
    )
    .expect("Material struct reflects");
    let mut m = ShaderMaterial::default();
    m.set_schema(Arc::new(schema));
    m.set("albedo", ParamValue::Vec3([0.4, 0.5, 0.6]));
    m.set_scalar("sun_vis", 1.0);
    // albedo vec3 @ byte 0 → lane 0 .xyz; sun_vis f32 @ byte 12 → lane 0 .w.
    assert_eq!(m.raw[0].x, 0.4);
    assert_eq!(m.raw[0].y, 0.5);
    assert_eq!(m.raw[0].z, 0.6);
    assert_eq!(m.raw[0].w, 1.0);
}

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
    assert_eq!(schema.field("surface_color").map(|f| f.ty), Some(ParamType::Vec3));
    assert_eq!(schema.field("transition").map(|f| f.ty), Some(ParamType::F32));
    assert_eq!(schema.field("major_grid_spacing").map(|f| f.ty), Some(ParamType::F32));
    // Whole block stays within the 256-byte uniform budget.
    assert!(schema.size <= 256, "blueprint params overflow uniform block: {}", schema.size);
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
    assert_eq!(schema.field("seamless_u").map(|f| f.ty), Some(ParamType::F32));
    assert_eq!(schema.field("v_scale").map(|f| f.ty), Some(ParamType::F32));
    assert!(schema.size <= 256, "solar_panel params overflow uniform block: {}", schema.size);
}
