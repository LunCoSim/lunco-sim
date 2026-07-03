//! Ph0′ integration test: a `UsdStageAsset` that carries a [`StageRecipe`] is
//! instantiated **off the live canonical stage**, end-to-end, in a headless
//! Bevy `App` — the visual extractor's runtime cutover.
//!
//! This is the test that would have caught the "runtime no-op" the boot
//! surfaced: the earlier design built the `CanonicalStage` in an `Update` system
//! that ran AFTER the synchronous `on_usd_prim_added` observer cascade, so the
//! extractors always missed the live stage and silently fell back to the
//! flatten. Here we assert (a) the `CanonicalStages` resource actually holds the
//! stage after instantiation — i.e. the on-demand build fired, so the LIVE
//! branch was taken — and (b) the resulting Bevy components (mesh, bound-shader
//! material, light) are correct, proving the live extraction produced them.
//!
//! What a headless test CANNOT cover is the final GPU pixel output; everything
//! up to and including the emitted ECS components is checked here.

use bevy::prelude::*;
use lunco_usd_bevy::{CanonicalStages, StageRecipe, UsdPrimPath, UsdStageAsset};

const SCENE: &str = r#"#usda 1.0
( defaultPrim = "World" )
def Xform "World"
{
    def Material "Mat"
    {
        token outputs:surface.connect = </World/Mat/S.outputs:surface>
        def Shader "S"
        {
            uniform token info:id = "UsdPreviewSurface"
            color3f inputs:diffuseColor = (0.1, 0.2, 0.8)
            float inputs:roughness = 0.4
            token outputs:surface
        }
    }
    def Cube "Box" ( prepend apiSchemas = ["MaterialBindingAPI"] )
    {
        rel material:binding = </World/Mat>
        double size = 2
    }
    def DistantLight "Sun"
    {
        float inputs:intensity = 5000
    }
}
"#;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();
    app.add_plugins(lunco_usd_bevy::UsdBevyPlugin);
    app
}

/// Find the entity whose `UsdPrimPath.path` equals `path`.
fn entity_at(app: &mut App, path: &str) -> Option<Entity> {
    let mut q = app.world_mut().query::<(Entity, &UsdPrimPath)>();
    q.iter(app.world())
        .find(|(_, p)| p.path == path)
        .map(|(e, _)| e)
}

#[test]
fn recipe_asset_instantiates_off_live_canonical_stage() {
    let mut app = app();

    // A recipe-carrying asset is the Ph0′ construction: the canonical stage is
    // buildable from it, so the dispatcher must take the LIVE branch.
    let recipe = StageRecipe::from_source("inmemory://scene.usda", SCENE);
    let handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset { recipe: Some(recipe) })
    };
    let stage_id = handle.id();

    // Spawn the scene root exactly as the loader does: a `UsdPrimPath` entity.
    // The `on_usd_prim_added` observer instantiates it (and recursively its
    // children) in a synchronous cascade — the exact path that used to miss the
    // canonical stage.
    app.world_mut().spawn((
        Name::new("World"),
        UsdPrimPath { stage_handle: handle.clone(), path: "/World".to_string() },
    ));
    // A couple of updates drain any deferred spawns / asset events.
    app.update();
    app.update();

    // (a) THE cutover assertion: the canonical stage was built and cached, which
    // only happens on the LIVE branch (`get_or_build`). If instantiation had
    // fallen back to the flatten, this map would be empty — exactly the runtime
    // no-op the boot caught.
    let has_canonical = app
        .world()
        .get_non_send_resource::<CanonicalStages>()
        .expect("CanonicalStages resource")
        .get(stage_id)
        .is_some();
    assert!(
        has_canonical,
        "instantiation must build + use the live canonical stage (LIVE branch), \
         not fall back to the flattened reader"
    );

    // (b) The bound-shader material resolved off the live stage: base color is the
    // shader's diffuseColor (0.1, 0.2, 0.8), which only resolves if the
    // material:binding → outputs:surface(.connect) → shader walk works on the
    // live `StageView` (the attribute-connection fix).
    let box_e = entity_at(&mut app, "/World/Box").expect("Box prim entity");
    let mat_h = app
        .world()
        .get::<MeshMaterial3d<StandardMaterial>>(box_e)
        .expect("Box has a StandardMaterial")
        .0
        .clone();
    assert!(app.world().get::<Mesh3d>(box_e).is_some(), "Box has a Mesh3d");
    let materials = app.world().resource::<Assets<StandardMaterial>>();
    let mat = materials.get(&mat_h).expect("material registered");
    let lin = mat.base_color.to_linear();
    assert!((lin.red - 0.1).abs() < 1e-4, "diffuse R off live shader: {}", lin.red);
    assert!((lin.green - 0.2).abs() < 1e-4, "diffuse G off live shader: {}", lin.green);
    assert!((lin.blue - 0.8).abs() < 1e-4, "diffuse B off live shader: {}", lin.blue);

    // (c) The UsdLux light extracted off the live stage → a Bevy DirectionalLight.
    let sun_e = entity_at(&mut app, "/World/Sun").expect("Sun prim entity");
    assert!(
        app.world().get::<DirectionalLight>(sun_e).is_some(),
        "DistantLight must project to a DirectionalLight off the live stage"
    );
}

#[test]
fn recipeless_asset_builds_no_canonical_and_is_skipped() {
    // Post-collapse invariant: the flatten fallback is GONE — the canonical stage
    // is the single source. An asset with no recipe builds no canonical stage, so
    // the visual dispatcher SKIPS it (no stage → no instantiation), rather than
    // falling back to the flattened reader. Every runtime scene now loads through
    // the recipe-building async loader, so recipe-less assets don't occur in
    // production; this pins the skip-not-crash behavior.
    let mut app = app();
    let handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset { recipe: None })
    };
    app.world_mut().spawn((
        Name::new("World"),
        UsdPrimPath { stage_handle: handle.clone(), path: "/World".to_string() },
    ));
    app.update();
    app.update();

    // No recipe ⇒ no canonical stage ⇒ the dispatcher skips (no children spawned).
    assert!(
        app.world().get_non_send_resource::<CanonicalStages>().unwrap().get(handle.id()).is_none(),
        "a recipe-less asset builds no canonical stage"
    );
    assert!(
        entity_at(&mut app, "/World/Box").is_none(),
        "with the flatten fallback removed, a recipe-less asset is skipped — no children instantiated"
    );
}
