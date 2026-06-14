//! Unit tests for lunco-materials crate.

use lunco_materials::{ParamSchema, ParamValue, ShaderMaterial};
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
