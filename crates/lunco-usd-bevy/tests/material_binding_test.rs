//! Integration test: writes a USDA fixture to disk and loads it. Native-only, so
//! the workspace `std::fs` ban (a wasm-runtime guard) does not apply — exactly the
//! `tests/` exemption `clippy.toml` describes but cargo cannot express as config.
//!
//! Asserts on the render-free appearance **intent** ([`PbrLook`]) rather than on a
//! `StandardMaterial`: `lunco-usd-bevy` no longer names `bevy_pbr` (see
//! `docs/architecture/render-decoupling.md`), and `lunco-render-bevy`'s own tests
//! cover the `PbrLook` → `StandardMaterial` binding. Every channel asserted here
//! is the same one the old material assertions checked.
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

    // The material resolves off the live canonical stage, built on demand from
    // this recipe (single in-memory layer, no external refs → `from_source`).
    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let stage_handle = stages.add(UsdStageAsset {
        recipe: Some(StageRecipe::from_source("scene.usda", usda_content)),
    });

    // Spawn the MeshWithMaterial entity representing the USD prim
    let test_entity = app
        .world_mut()
        .spawn((
            Name::new("MeshWithMaterial"),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
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

#[test]
fn test_usd_material_modification() {
    let mut app = App::new();

    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());

    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();

    app.add_plugins(UsdBevyPlugin);

    // Initial USDA content
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

    let stage_handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset {
            recipe: Some(StageRecipe::from_source("scene.usda", usda_content)),
        })
    };

    let test_entity = app
        .world_mut()
        .spawn((
            Name::new("MeshWithMaterial"),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: "/World/MeshWithMaterial".to_string(),
            },
        ))
        .id();

    app.update();

    // Verify initial values
    let look = app
        .world()
        .get::<PbrLook>(test_entity)
        .expect("Entity should have a PbrLook");
    assert!((look.base_color.red - 1.0).abs() < 1e-4);
    assert!((look.perceptual_roughness - 0.75).abs() < 1e-4);

    // Now simulate updating the USD document
    let updated_usda_content = r#"#usda 1.0
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
            color3f inputs:diffuseColor = (0.0, 0.0, 1.0)
            float inputs:roughness = 0.1
            float inputs:metallic = 0.9
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

    // Re-author the USD: rebuild the live canonical stage from a NEW recipe made
    // from the updated source (the canonical model's re-derive path — replaces
    // the old in-place `asset.reader = ...` swap). `rebuild` drops the previous
    // stage + sink and composes the post-edit one, so the re-instantiation below
    // reads the new material off the live stage.
    {
        let new_recipe = StageRecipe::from_source("scene.usda", updated_usda_content);
        let mut canonical = app
            .world_mut()
            .get_non_send_mut::<CanonicalStages>()
            .unwrap();
        assert!(
            canonical.rebuild(stage_handle.id(), &new_recipe),
            "rebuilding the canonical stage from the updated recipe must succeed"
        );
    }

    // Trigger visual sync again on the entity by removing UsdVisualSynced and triggering UsdPrimPath addition
    app.world_mut()
        .entity_mut(test_entity)
        .remove::<UsdVisualSynced>();
    let prim_path = app
        .world_mut()
        .entity_mut(test_entity)
        .take::<UsdPrimPath>()
        .unwrap();
    app.world_mut().entity_mut(test_entity).insert(prim_path);

    app.update();

    // Verify updated values
    let look2 = app
        .world()
        .get::<PbrLook>(test_entity)
        .expect("Entity should still have a PbrLook");

    // base color should now be blue (0.0, 0.0, 1.0)
    assert!((look2.base_color.red - 0.0).abs() < 1e-4);
    assert!((look2.base_color.green - 0.0).abs() < 1e-4);
    assert!((look2.base_color.blue - 1.0).abs() < 1e-4);

    assert!((look2.perceptual_roughness - 0.1).abs() < 1e-4);
    assert!((look2.metallic - 0.9).abs() < 1e-4);
}

/// Helper: parse a USDA stage, bind it to one prim, run the visual sync, and
/// return the resulting appearance intent.
fn material_for(usda: &str, prim_path: &str) -> PbrLook {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    app.add_plugins(UsdBevyPlugin);

    let stage_handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset {
            recipe: Some(StageRecipe::from_source("scene.usda", usda)),
        })
    };
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Bound"),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: prim_path.to_string(),
            },
        ))
        .id();
    app.update();

    app.world()
        .get::<PbrLook>(entity)
        .expect("entity should have a PbrLook")
        .clone()
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
