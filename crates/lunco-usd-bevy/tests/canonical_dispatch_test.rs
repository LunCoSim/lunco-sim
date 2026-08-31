//! Integration test for the prepared USD projection plan and its live-stage
//! handoff. A real `UsdStageAsset` is projected end-to-end in a headless Bevy
//! app, and the resulting ECS components are checked across primitive, material,
//! lighting, and curve/surface paths.
//!
//! The appearance assertion is on the render-free `PbrLook` **intent** — this
//! crate no longer names `StandardMaterial` (see
//! `docs/architecture/render-decoupling.md`); `lunco-render-bevy` binds the look
//! to a material, and its own tests cover that half.
//!
//! What a headless test CANNOT cover is the final GPU pixel output; everything
//! up to and including the emitted ECS components is checked here.

use bevy::prelude::*;
use lunco_materials::ProceduralSkybox;
use lunco_render::PbrLook;
use lunco_usd_bevy::{
    CanonicalStage, CanonicalStages, StageRecipe, UsdPrimPath, UsdStageAsset, UsdVisualSyncFailed,
};

const SCENE: &str = r#"#usda 1.0
( defaultPrim = "World", metersPerUnit = 1 )
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
    def Xform "Sky" ( prepend apiSchemas = ["MaterialBindingAPI"] )
    {
        bool lunco:surface:skybox = true
        rel material:binding = </World/Mat>
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
fn recipe_asset_instantiates_from_prepared_projection_plan() {
    let mut app = app();

    // The asset carries the worker-produced prepared plan and the recipe used
    // by the live canonical stage for later authoring.
    let recipe = StageRecipe::from_source("inmemory://scene.usda", SCENE);
    let handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset::from_recipe(recipe).expect("prepare stage asset"))
    };
    let stage_id = handle.id();

    // Spawn the scene root exactly as the loader does. The observer only admits
    // it to the bounded projection queue; projection creates children from the
    // prepared hierarchy.
    app.world_mut().spawn((
        Name::new("World"),
        UsdPrimPath {
            stage_handle: handle,
            path: "/World".to_string(),
        },
    ));
    // Structural projection and CPU geometry have separate completion edges.
    // Drive the production app until the geometry workers publish all meshes;
    // a fixed two-update assertion would test an obsolete synchronous loader.
    for _ in 0..256 {
        app.update();
        let all_meshes_ready = {
            let world = app.world_mut();
            let mut q = world.query::<(&UsdPrimPath, Option<&Mesh3d>)>();
            [
                "/World/Box",
                "/World/Conduit",
                "/World/Elbow",
                "/World/ShellQuarter",
            ]
            .iter()
            .all(|path| {
                q.iter(world)
                    .any(|(prim, mesh)| prim.path == *path && mesh.is_some())
            })
        };
        if all_meshes_ready {
            break;
        }
        std::thread::yield_now();
    }
    // (a) Initial materialisation does not open the non-Send live stage. The
    // prepared plan is the complete composed reader for generation zero; the
    // live stage is opened only when an authored edit needs it.
    let has_canonical = app
        .world()
        .get_non_send::<CanonicalStages>()
        .expect("CanonicalStages resource")
        .get(stage_id)
        .is_some();
    assert!(
        !has_canonical,
        "initial materialisation must not open a live stage"
    );
    assert!(
        app.world()
            .resource::<Assets<UsdStageAsset>>()
            .get(stage_id)
            .map(|asset| asset.projection_plan.as_ref())
            .is_some(),
        "initial materialisation must retain its prepared composed projection"
    );

    // (b) The bound-shader appearance came from the prepared composed plan:
    // material:binding → outputs:surface → shader inputs.
    let box_e = entity_at(&mut app, "/World/Box").expect("Box prim entity");
    let look = app
        .world()
        .get::<PbrLook>(box_e)
        .expect("Box has a PbrLook")
        .clone();
    assert!(
        app.world().get::<Mesh3d>(box_e).is_some(),
        "Box has a Mesh3d"
    );

    // A procedural camera background is an Xform appearance intent. It is
    // projected once and consumed by the fullscreen background pass; the USD
    // projection must not create a mesh or leave mesh state from a prior
    // generation on the owner.
    let sky_e = entity_at(&mut app, "/World/Sky").expect("Sky prim entity");
    assert!(
        app.world().get::<ProceduralSkybox>(sky_e).is_some(),
        "the authored skybox intent must be projected onto the Xform"
    );
    assert!(
        app.world().get::<Mesh3d>(sky_e).is_none(),
        "a procedural skybox Xform must never receive mesh geometry"
    );
    let lin = look.base_color;
    assert!(
        (lin.red - 0.1).abs() < 1e-4,
        "diffuse R off prepared shader: {}",
        lin.red
    );
    assert!(
        (lin.green - 0.2).abs() < 1e-4,
        "diffuse G off prepared shader: {}",
        lin.green
    );
    assert!(
        (lin.blue - 0.8).abs() < 1e-4,
        "diffuse B off prepared shader: {}",
        lin.blue
    );
    assert!(
        (look.perceptual_roughness - 0.4).abs() < 1e-4,
        "roughness off prepared shader"
    );

    // (c) UsdLux light projection.
    let sun_e = entity_at(&mut app, "/World/Sun").expect("Sun prim entity");
    assert!(
        app.world().get::<DirectionalLight>(sun_e).is_some(),
        "DistantLight must project to a DirectionalLight"
    );

    // (d) `UsdLuxRectLight` → Bevy `RectLight`. Both put the rectangle in the
    // local XY plane emitting along -Z, so the mapping is 1:1 and `inputs:width`
    // / `inputs:height` carry straight through. Before this arm existed, AREA
    // lights hit the dispatcher's `_ => false` and vanished silently.
    let panel_e = entity_at(&mut app, "/World/CeilingPanel").expect("CeilingPanel prim entity");
    let panel = app
        .world()
        .get::<RectLight>(panel_e)
        .expect("RectLight must project to a Bevy RectLight");
    assert!((panel.width - 1.2).abs() < 1e-4, "width {}", panel.width);
    assert!((panel.height - 0.6).abs() < 1e-4, "height {}", panel.height);
    // `RectLight::intensity` is luminous POWER in lumens (unlike Point/Spot,
    // which are candela). With the schema-default `normalize = false`, the
    // authored power scales by the authored emitting area relative to the
    // schema's 1 m² fallback: 8000 × 1.2 × 0.6 = 5760 lumens. Exposure is
    // unauthored here, so it contributes its neutral 2^0 factor.
    assert!(
        (panel.intensity - 5760.0).abs() < 1e-2,
        "intensity {}",
        panel.intensity
    );

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
fn procedural_skybox_requires_an_xform_owner() {
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    def Sphere "Sky"
    {
        bool lunco:surface:skybox = true
        double radius = 1000
    }
}
"#;

    let mut app = app();
    let stage_handle = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
        UsdStageAsset::from_recipe(StageRecipe::from_source("invalid-sky.usda", SCENE))
            .expect("prepare stage asset"),
    );
    let sky_e = app
        .world_mut()
        .spawn((
            Name::new("Sky"),
            UsdPrimPath {
                stage_handle,
                path: "/World/Sky".into(),
            },
        ))
        .id();

    app.update();

    assert!(
        app.world().get::<UsdVisualSyncFailed>(sky_e).is_some(),
        "a skybox flag on a gprim must fail the USD projection visibly"
    );
    assert!(
        app.world().get::<ProceduralSkybox>(sky_e).is_none(),
        "an invalid skybox owner must not enter the background path"
    );
    assert!(
        app.world().get::<Mesh3d>(sky_e).is_none(),
        "an invalid skybox owner must not receive a mesh"
    );
}

#[test]
fn procedural_skybox_projection_removes_existing_mesh_state() {
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "Sky"
)
def Xform "Sky"
{
    bool lunco:surface:skybox = true
}
"#;

    let mut app = app();
    let stage_handle = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
        UsdStageAsset::from_recipe(StageRecipe::from_source("sky.usda", SCENE))
            .expect("prepare stage asset"),
    );
    let stale_mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    let sky_e = app
        .world_mut()
        .spawn((
            Name::new("Sky"),
            Mesh3d(stale_mesh),
            UsdPrimPath {
                stage_handle,
                path: "/Sky".into(),
            },
        ))
        .id();

    app.update();

    assert!(
        app.world().get::<ProceduralSkybox>(sky_e).is_some(),
        "the authored skybox intent must own the projection"
    );
    assert!(
        app.world().get::<Mesh3d>(sky_e).is_none(),
        "re-projecting a procedural skybox must remove stale mesh state"
    );
}

#[test]
fn externally_composed_asset_has_prepared_projection_plan() {
    let recipe = StageRecipe::from_source("inmemory://scene.usda", SCENE);
    let canonical = CanonicalStage::from_recipe(&recipe).expect("compose external stage");
    let asset = UsdStageAsset::from_composed_stage(canonical.stage())
        .expect("snapshot externally composed stage");

    assert!(
        asset
            .projection_plan
            .prims
            .iter()
            .any(|prim| prim.path == "/World/Box"),
        "every asset construction path must provide the prepared composed plan"
    );
}

#[test]
fn identical_stage_paths_project_once_per_parent() {
    const SCENE: &str = r#"#usda 1.0
(
    defaultPrim = "World"
)
def Xform "World"
{
    def Xform "Child"
    {
    }
}
"#;

    let mut app = app();
    let handle = app.world_mut().resource_mut::<Assets<UsdStageAsset>>().add(
        UsdStageAsset::from_recipe(StageRecipe::from_source("scene.usda", SCENE))
            .expect("prepare stage asset"),
    );

    // Two mounted owners may legitimately project the same composed stage path.
    // Path identity is scoped by the USD parent/instance, not by a global scan.
    let parent_a = app
        .world_mut()
        .spawn((
            Name::new("World A"),
            UsdPrimPath {
                stage_handle: handle.clone(),
                path: "/World".into(),
            },
        ))
        .id();
    let parent_b = app
        .world_mut()
        .spawn((
            Name::new("World B"),
            UsdPrimPath {
                stage_handle: handle,
                path: "/World".into(),
            },
        ))
        .id();

    for _ in 0..4 {
        app.update();
    }

    let child_count = |app: &mut App, parent: Entity| {
        app.world()
            .get::<Children>(parent)
            .into_iter()
            .flat_map(|children| children.iter())
            .filter(|child| {
                app.world()
                    .get::<UsdPrimPath>(*child)
                    .is_some_and(|prim| prim.path == "/World/Child")
            })
            .count()
    };
    assert_eq!(child_count(&mut app, parent_a), 1);
    assert_eq!(child_count(&mut app, parent_b), 1);
}
