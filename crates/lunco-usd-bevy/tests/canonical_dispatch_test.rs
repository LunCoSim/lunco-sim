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
//! appearance intent, light) are correct, proving the live extraction produced
//! them.
//!
//! The appearance assertion is on the render-free `PbrLook` **intent** — this
//! crate no longer names `StandardMaterial` (see
//! `docs/architecture/render-decoupling.md`); `lunco-render-bevy` binds the look
//! to a material, and its own tests cover that half.
//!
//! What a headless test CANNOT cover is the final GPU pixel output; everything
//! up to and including the emitted ECS components is checked here.

use bevy::prelude::*;
use lunco_render::PbrLook;
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
    def RectLight "CeilingPanel"
    {
        float inputs:intensity = 8000
        float inputs:width = 1.2
        float inputs:height = 0.6
    }
    def BasisCurves "Conduit"
    {
        uniform token type = "linear"
        int[] curveVertexCounts = [4]
        point3f[] points = [(0, 0, 0), (0, 0, 1), (1, 0, 1), (1, 0, 2)]
        float[] widths = [0.08]
    }
    def BasisCurves "CameraRail"
    {
        uniform token type = "cubic"
        uniform token basis = "catmullRom"
        int[] curveVertexCounts = [4]
        point3f[] points = [(0, 5, 0), (2, 5, 0), (2, 5, 2), (0, 5, 2)]
    }
    def Xform "Cutters" ( )
    {
        uniform token purpose = "guide"
        def Cube "PortholeCutter"
        {
            double size = 0.42
        }
    }
    def NurbsPatch "ShellQuarter"
    {
        int uVertexCount = 3
        int vVertexCount = 2
        int uOrder = 3
        int vOrder = 2
        double[] uKnots = [0, 0, 0, 1, 1, 1]
        double[] vKnots = [0, 0, 1, 1]
        double[] pointWeights = [1, 0.70710678118, 1, 1, 0.70710678118, 1]
        point3f[] points = [
            (1, 0, 0), (1, 0, 1), (0, 0, 1),
            (1, 2, 0), (1, 2, 1), (0, 2, 1)
        ]
    }
    def NurbsCurves "Elbow"
    {
        int[] curveVertexCounts = [3]
        int[] order = [3]
        double[] knots = [0, 0, 0, 1, 1, 1]
        double[] pointWeights = [1, 0.70710678118, 1]
        point3f[] points = [(1, 0, 0), (1, 1, 0), (0, 1, 0)]
        float[] widths = [0.05]
    }
}
"#;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
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
        .get_non_send::<CanonicalStages>()
        .expect("CanonicalStages resource")
        .get(stage_id)
        .is_some();
    assert!(
        has_canonical,
        "instantiation must build + use the live canonical stage (LIVE branch), \
         not fall back to the flattened reader"
    );

    // (b) The bound-shader appearance resolved off the live stage: base color is the
    // shader's diffuseColor (0.1, 0.2, 0.8), which only resolves if the
    // material:binding → outputs:surface(.connect) → shader walk works on the
    // live `StageView` (the attribute-connection fix).
    let box_e = entity_at(&mut app, "/World/Box").expect("Box prim entity");
    let look = app
        .world()
        .get::<PbrLook>(box_e)
        .expect("Box has a PbrLook")
        .clone();
    assert!(app.world().get::<Mesh3d>(box_e).is_some(), "Box has a Mesh3d");
    let lin = look.base_color;
    assert!((lin.red - 0.1).abs() < 1e-4, "diffuse R off live shader: {}", lin.red);
    assert!((lin.green - 0.2).abs() < 1e-4, "diffuse G off live shader: {}", lin.green);
    assert!((lin.blue - 0.8).abs() < 1e-4, "diffuse B off live shader: {}", lin.blue);
    assert!((look.perceptual_roughness - 0.4).abs() < 1e-4, "roughness off live shader");

    // (c) The UsdLux light extracted off the live stage → a Bevy DirectionalLight.
    let sun_e = entity_at(&mut app, "/World/Sun").expect("Sun prim entity");
    assert!(
        app.world().get::<DirectionalLight>(sun_e).is_some(),
        "DistantLight must project to a DirectionalLight off the live stage"
    );

    // (d) `UsdLuxRectLight` → Bevy `RectLight`. Both put the rectangle in the
    // local XY plane emitting along -Z, so the mapping is 1:1 and `inputs:width`
    // / `inputs:height` carry straight through. Before this arm existed, AREA
    // lights hit the dispatcher's `_ => false` and vanished silently.
    let panel_e = entity_at(&mut app, "/World/CeilingPanel").expect("CeilingPanel prim entity");
    let panel = app
        .world()
        .get::<RectLight>(panel_e)
        .expect("RectLight must project to a Bevy RectLight off the live stage");
    assert!((panel.width - 1.2).abs() < 1e-4, "width {}", panel.width);
    assert!((panel.height - 0.6).abs() < 1e-4, "height {}", panel.height);
    // `RectLight::intensity` is luminous POWER in lumens (unlike Point/Spot,
    // which are candela) — the authored 8000 is taken as lumens, unscaled
    // because `inputs:exposure` is unauthored (2^0 = 1).
    assert!((panel.intensity - 8000.0).abs() < 1e-2, "intensity {}", panel.intensity);

    // (e) `UsdGeomBasisCurves` + `widths` → swept-tube geometry. A curve prim
    // carrying a width is a TUBE, not a line, so it must produce a mesh.
    let conduit_e = entity_at(&mut app, "/World/Conduit").expect("Conduit prim entity");
    assert!(
        app.world().get::<Mesh3d>(conduit_e).is_some(),
        "a BasisCurves with `widths` must sweep to a Mesh3d"
    );

    // (f) …and `widths` is exactly what discriminates geometry from a pure PATH.
    // A camera rail authors no `widths` — it is infinitely thin, has no surface,
    // and must NOT silently become a visible pipe. This is the USD-native
    // distinction, which is why the curve reader needs no `lunco:` gate to tell
    // the two apart.
    let rail_e = entity_at(&mut app, "/World/CameraRail").expect("CameraRail prim entity");
    assert!(
        app.world().get::<Mesh3d>(rail_e).is_none(),
        "a BasisCurves WITHOUT `widths` has no surface and must not become geometry"
    );

    // (g) `UsdGeomNurbsCurves` sweeps through the same path — a rational quadratic
    // quarter-arc (middle weight √2/2), i.e. the pipe-elbow case. It shares the
    // sweep with BasisCurves; only the centerline evaluator differs, so this pins
    // that the NURBS branch is reached and produces geometry at all.
    let elbow_e = entity_at(&mut app, "/World/Elbow").expect("Elbow prim entity");
    assert!(
        app.world().get::<Mesh3d>(elbow_e).is_some(),
        "a NurbsCurves with `widths` must sweep to a Mesh3d"
    );

    // (h) `UsdGeomNurbsPatch` → a tessellated surface. Unlike the curves, a patch
    // needs NO `widths` — it is already a surface. This one is a rational
    // cylindrical quarter, the shape HAB-1's shell and every lathe part are made
    // of, and the only way USD can express a PARTIAL revolution (the gprims are
    // all complete ones).
    let patch_e = entity_at(&mut app, "/World/ShellQuarter").expect("ShellQuarter prim entity");
    assert!(
        app.world().get::<Mesh3d>(patch_e).is_some(),
        "a NurbsPatch must tessellate to a Mesh3d"
    );

    // (i) `purpose = "guide"` is INHERITED. The cutter Cube authors no purpose of
    // its own — only its parent `Cutters` Xform does — so this pins the ancestor
    // walk. Reading the prim alone would render every child of a guide group,
    // which for HAB-1 means nine boolean cutters appearing as solid boxes
    // floating through the shell.
    let cutter_e =
        entity_at(&mut app, "/World/Cutters/PortholeCutter").expect("cutter prim entity");
    assert!(
        app.world().get::<Mesh3d>(cutter_e).is_none(),
        "a prim under a `purpose = \"guide\"` ancestor must not render"
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
        app.world().get_non_send::<CanonicalStages>().unwrap().get(handle.id()).is_none(),
        "a recipe-less asset builds no canonical stage"
    );
    assert!(
        entity_at(&mut app, "/World/Box").is_none(),
        "with the flatten fallback removed, a recipe-less asset is skipped — no children instantiated"
    );
}
