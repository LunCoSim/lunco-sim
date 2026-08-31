//! Integration test: writes a USDA fixture to disk and loads it. Native-only, so
//! the workspace `std::fs` ban (a wasm-runtime guard) does not apply — exactly the
//! `tests/` exemption `clippy.toml` describes but cargo cannot express as config.
//!
//! Asserts on the render-free appearance **intent** ([`PbrLook`]) rather than on a
//! `StandardMaterial`: `lunco-usd-bevy` no longer names `bevy_pbr` (see
//! `docs/architecture/render-decoupling.md`), and `lunco-render-bevy`'s own tests
//! cover the `PbrLook` → `StandardMaterial` binding. Every channel asserted here
//! belongs to the current appearance contract.
#![allow(clippy::disallowed_methods)]

use bevy::prelude::*;
use lunco_render::{PbrLook, SurfaceAlpha};
use lunco_usd_bevy::*;

#[test]
fn test_usd_material_binding_parsing() {
    let mut app = App::new();

    // Core Bevy plugins
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());

    // Register assets manually to avoid full render plugin dependencies
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();

    app.add_plugins(UsdBevyPlugin);

    // Setup a mock USD stage with a Material, Shader and a bound Cube Mesh
    let usda_content = r#"#usda 1.0
(
    defaultPrim = "World"
)

def Xform "World"
{
    def Material "MyMaterial"
    {
        token outputs:surface.connect = </World/MyMaterial/PbrShader.outputs:surface>

        def Shader "PbrShader"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (1.0, 0.5, 0.25)
            float inputs:roughness = 0.75
            float inputs:metallic = 0.25
            color3f inputs:emissiveColor = (0.1, 0.2, 0.3)
            token outputs:surface
        }
    }

    def Cube "MeshWithMaterial" (
        apiSchemas = ["MaterialBindingAPI"]
    )
    {
        rel material:binding = </World/MyMaterial>
        double size = 2.0
    }
}
"#;

    // The material resolves from the asset's prepared composed projection. The
    // same recipe remains available to the live canonical stage for edits.
    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let stage_handle = stages.add(
        UsdStageAsset::from_recipe(StageRecipe::from_source("scene.usda", usda_content))
            .expect("prepare stage asset"),
    );

    // Spawn the MeshWithMaterial entity representing the USD prim
    let test_entity = app
        .world_mut()
        .spawn((
            Name::new("MeshWithMaterial"),
            UsdPrimPath {
                stage_handle,
                path: "/World/MeshWithMaterial".to_string(),
            },
        ))
        .id();

    // Run the systems to trigger visual synchronization
    app.update();

    // Check if the entity was processed and has visual sync
    assert!(app.world().get::<UsdVisualSynced>(test_entity).is_some());

    // Verify the appearance intent exists
    let look = app
        .world()
        .get::<PbrLook>(test_entity)
        .expect("Entity should have a PbrLook component");

    // Assert PBR properties parsed from shader network matches expectation
    assert!((look.base_color.red - 1.0).abs() < 1e-4);
    assert!((look.base_color.green - 0.5).abs() < 1e-4);
    assert!((look.base_color.blue - 0.25).abs() < 1e-4);

    assert!((look.perceptual_roughness - 0.75).abs() < 1e-4);
    assert!((look.metallic - 0.25).abs() < 1e-4);

    let emissive = look.emissive;
    assert!((emissive.red - 0.1).abs() < 1e-4);
    assert!((emissive.green - 0.2).abs() < 1e-4);
    assert!((emissive.blue - 0.3).abs() < 1e-4);

    // A static material shares its (content-keyed) material with every identical
    // look — only an ANIMATED one opts out.
    assert!(!look.unshared, "a static material must stay shareable");
}

/// Helper: parse a USDA stage, bind it to one prim, run the visual sync, and
/// return the resulting appearance intent.
fn material_for(usda: &str, prim_path: &str) -> PbrLook {
    material_for_optional(usda, prim_path).expect("entity should have a PbrLook")
}

/// Run the production USD visual projection and preserve the distinction
/// between a valid look and a rejected authored material.  A missing look is
/// the important assertion for malformed USD: a type error must not produce a
/// plausible appearance intent at the consumer boundary.
fn material_for_optional(usda: &str, prim_path: &str) -> Option<PbrLook> {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.add_plugins(UsdBevyPlugin);

    let stage_handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(
            UsdStageAsset::from_recipe(StageRecipe::from_source("scene.usda", usda))
                .expect("prepare stage asset"),
        )
    };
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Bound"),
            UsdPrimPath {
                stage_handle,
                path: prim_path.to_string(),
            },
        ))
        .id();
    app.update();

    app.world().get::<PbrLook>(entity).cloned()
}

#[test]
fn marker_assets_are_emissive_and_shadowless() {
    const WAYPOINT: &str = include_str!("../../../assets/vessels/markers/waypoint.usda");
    const LANDING_LOCATION: &str =
        include_str!("../../../assets/vessels/markers/landing_location.usda");
    const PREDICTED_LANDING: &str =
        include_str!("../../../assets/vessels/markers/predicted_landing.usda");
    let markers = [
        ("waypoint", WAYPOINT, "/WaypointMarker/Dome", Some(0.45)),
        (
            "landing location",
            LANDING_LOCATION,
            "/LandingLocationMarker/Dome",
            None,
        ),
        (
            "predicted landing PX",
            PREDICTED_LANDING,
            "/PredictedLandingMarker/Brackets/PX",
            None,
        ),
        (
            "predicted landing NX",
            PREDICTED_LANDING,
            "/PredictedLandingMarker/Brackets/NX",
            None,
        ),
        (
            "predicted landing PZ",
            PREDICTED_LANDING,
            "/PredictedLandingMarker/Brackets/PZ",
            None,
        ),
        (
            "predicted landing NZ",
            PREDICTED_LANDING,
            "/PredictedLandingMarker/Brackets/NZ",
            None,
        ),
        (
            "predicted landing center",
            PREDICTED_LANDING,
            "/PredictedLandingMarker/Brackets/Center",
            None,
        ),
    ];

    for (name, usda, prim_path, expected_opacity) in markers {
        let look = material_for(usda, prim_path);
        assert!(look.no_shadow_cast, "{name} must not cast shadows");
        assert_eq!(look.base_color.red, 0.0, "{name} must have no diffuse red");
        assert_eq!(
            look.base_color.green, 0.0,
            "{name} must have no diffuse green"
        );
        assert_eq!(
            look.base_color.blue, 0.0,
            "{name} must have no diffuse blue"
        );
        assert_eq!(
            look.specular_tint.red, 0.0,
            "{name} must have no specular red"
        );
        assert_eq!(
            look.specular_tint.green, 0.0,
            "{name} must have no specular green"
        );
        assert_eq!(
            look.specular_tint.blue, 0.0,
            "{name} must have no specular blue"
        );
        assert!(look.emissive != LinearRgba::BLACK, "{name} must emit light");
        match expected_opacity {
            Some(opacity) => {
                assert!(
                    matches!(look.alpha, SurfaceAlpha::Blend),
                    "{name} must blend"
                );
                assert!(
                    (look.base_color.alpha - opacity).abs() < 1e-4,
                    "{name} must use authored opacity {opacity}, got {}",
                    look.base_color.alpha
                );
            }
            None => assert!(
                matches!(look.alpha, SurfaceAlpha::Opaque),
                "{name} must remain opaque"
            ),
        }
    }
}

const OPACITY_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Glass"
    {
        token outputs:surface.connect = </World/Glass/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.2, 0.4, 0.8)
            float inputs:opacity = 0.4
            float inputs:ior = 1.45
            token outputs:surface
        }
    }
    def Cube "Pane" ( apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Glass>
        double size = 2.0
    }
}
"#;

/// `inputs:opacity < 1` → base-color alpha + alpha-blended; `inputs:ior` binds.
#[test]
fn opacity_drives_alpha_blend_and_ior() {
    let look = material_for(OPACITY_STAGE, "/World/Pane");
    assert!(
        (look.base_color.alpha - 0.4).abs() < 1e-4,
        "alpha from inputs:opacity"
    );
    assert!(
        matches!(look.alpha, SurfaceAlpha::Blend),
        "sub-1 opacity → Blend"
    );
    assert!((look.ior - 1.45).abs() < 1e-4, "ior bound");
}

const ADDITIVE_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Glow"
    {
        token outputs:surface.connect = </World/Glow/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (1.0, 0.2, 0.02)
            float inputs:opacity = 0.4
            token outputs:surface
        }
    }
    def Cube "Plume" ( apiSchemas = ["MaterialBindingAPI", "LunCoSurfaceAPI"] )
    {
        rel material:binding = </World/Glow>
        bool lunco:surface:additive = true
        double size = 2.0
    }
}
"#;

/// The authored surface policy wins over preview-surface opacity: an emissive
/// volume must add radiance without darkening the surface behind it.
#[test]
fn authored_additive_surface_uses_additive_blending() {
    let look = material_for(ADDITIVE_STAGE, "/World/Plume");
    assert!(matches!(look.alpha, SurfaceAlpha::Add));
}

const CUTOUT_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Foliage"
    {
        token outputs:surface.connect = </World/Foliage/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.1, 0.6, 0.1)
            float inputs:opacityThreshold = 0.5
            token outputs:surface
        }
    }
    def Cube "Leaf" ( apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Foliage>
        double size = 2.0
    }
}
"#;

/// A non-zero `inputs:opacityThreshold` → cutout (`SurfaceAlpha::Mask`).
#[test]
fn opacity_threshold_is_alpha_mask() {
    let look = material_for(CUTOUT_STAGE, "/World/Leaf");
    match look.alpha {
        SurfaceAlpha::Mask(t) => assert!((t - 0.5).abs() < 1e-4),
        other => panic!("expected Mask(0.5), got {other:?}"),
    }
}

/// An opaque material (no opacity authored) stays `Opaque` — no needless
/// transparent pass.
#[test]
fn opaque_material_stays_opaque() {
    let look = material_for(
        OPACITY_STAGE
            .replace("float inputs:opacity = 0.4\n", "")
            .as_str(),
        "/World/Pane",
    );
    assert!(
        matches!(look.alpha, SurfaceAlpha::Opaque),
        "no opacity → Opaque"
    );
    assert!((look.base_color.alpha - 1.0).abs() < 1e-4);
}

const MALFORMED_ROUGHNESS_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Look"
    {
        token outputs:surface.connect = </World/Look/Surface.outputs:surface>
        def Shader "Surface"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.2, 0.3, 0.4)
            string inputs:roughness = "not-a-number"
            token outputs:surface
        }
    }
    def Cube "Body" ( apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Look>
        double size = 2.0
    }
}
"#;

/// An authored wrong-type shader input must not silently select the
/// `UsdPreviewSurface` roughness default.
#[test]
fn malformed_shader_scalar_does_not_use_preview_default() {
    assert!(
        material_for_optional(MALFORMED_ROUGHNESS_STAGE, "/World/Body").is_none(),
        "wrong USD roughness type must reject the PBR intent"
    );
}

const OUT_OF_RANGE_ROUGHNESS_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Look"
    {
        token outputs:surface.connect = </World/Look/Surface.outputs:surface>
        def Shader "Surface"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.2, 0.3, 0.4)
            float inputs:roughness = 2.0
            token outputs:surface
        }
    }
    def Cube "Body" ( apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Look>
        double size = 2.0
    }
}
"#;

/// Unit-interval Preview Surface inputs are rejected, never clamped into a
/// different authored look.
#[test]
fn out_of_range_shader_scalar_is_rejected() {
    assert!(
        material_for_optional(OUT_OF_RANGE_ROUGHNESS_STAGE, "/World/Body").is_none(),
        "out-of-range roughness must not be clamped or defaulted"
    );
}

const SPECULAR_STAGE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Spec"
    {
        token outputs:surface.connect = </World/Spec/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.8, 0.8, 0.8)
            int inputs:useSpecularWorkflow = 1
            color3f inputs:specularColor = (0.9, 0.1, 0.1)
            float inputs:metallic = 0.7
            float inputs:clearcoat = 1.0
            float inputs:clearcoatRoughness = 0.2
            token outputs:surface
        }
    }
    def Cube "Body" ( apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Spec>
        double size = 2.0
    }
}
"#;

/// Specular workflow forces `metallic = 0`; clearcoat + clearcoatRoughness map 1:1.
///
/// KNOWN GAP: the `specularColor` TINT is not carried — `PbrLook` has no
/// `specular_tint` channel, so a specular-workflow prim renders with an untinted
/// (white) specular highlight. Closing it means adding one field to `PbrLook` and
/// to `lunco-render-bevy`'s `standard_material()`; no scene in the repo authors one.
#[test]
fn specular_workflow_and_clearcoat_bind() {
    let look = material_for(SPECULAR_STAGE, "/World/Body");
    assert!(
        (look.metallic - 0.0).abs() < 1e-4,
        "specular workflow → metallic 0"
    );
    assert!((look.clearcoat - 1.0).abs() < 1e-4, "clearcoat bound");
    assert!(
        (look.clearcoat_perceptual_roughness - 0.2).abs() < 1e-4,
        "clearcoatRoughness bound"
    );
}
