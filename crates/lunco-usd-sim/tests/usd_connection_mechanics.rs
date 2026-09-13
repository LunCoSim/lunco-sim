//! Mechanism tests for USD-native co-simulation wiring.
//!
//! Production-authored scenes own asset-specific wiring assertions. These
//! tests retain only the generic derived-cache contract:
//! `connectionPaths` becomes `SimConnection` at load, edits rebuild it, and
//! authored factor/offset values survive both supported scalar types.

use bevy::asset::AssetApp;
use bevy::prelude::*;
use lunco_cosim::SimConnection;
use lunco_usd_bevy_core::{canonical::CanonicalStages, UsdStageAsset};
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_core::StageRecipe;
use lunco_usd_sim::cosim::{install_wiring_system, WiringDirty};
use openusd::sdf::Path as SdfPath;

/// Build an app with a live canonical stage for `SCENE`, initial changes drained.
fn setup() -> (App, AssetId<UsdStageAsset>, Handle<UsdStageAsset>) {
    let mut app = App::new();
    app.add_plugins(bevy::asset::AssetPlugin::default())
        .init_asset::<UsdStageAsset>()
        .init_non_send::<CanonicalStages>()
        .init_resource::<WiringDirty>();

    // Keep this mechanism fixture structural rather than embedding a long USDA
    // document in Rust. Authored asset/policy fixtures belong in USD + Rhai;
    // this test only needs two endpoint prims for the derived-cache seam.
    let recipe = StageRecipe::from_source("scene.usda", "#usda 1.0\n");
    let handle = app
        .world_mut()
        .resource_mut::<Assets<UsdStageAsset>>()
        .add(UsdStageAsset::from_recipe(recipe.clone()).expect("prepare stage asset"));
    let id = handle.id();

    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .get_or_build(id, &recipe)
        .expect("canonical stage builds from the recipe");
    {
        let stage = app
            .world()
            .non_send::<CanonicalStages>()
            .get(id)
            .expect("canonical stage exists")
            .stage();
        stage
            .define_prim("/World")
            .expect("define wiring root")
            .set_type_name("Xform")
            .expect("type wiring root");
        for (path, type_name) in [("/World/Src", "Cube"), ("/World/Sink", "Cube")] {
            stage
                .define_prim(path)
                .expect("define wiring endpoint")
                .set_type_name(type_name)
                .expect("type wiring endpoint");
        }
    }
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .drain_all_changes();
    (app, id, handle)
}

fn spawn_endpoints(app: &mut App, handle: Handle<UsdStageAsset>) {
    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle.clone(),
            path: "/World/Src".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
    app.world_mut().spawn((
        UsdPrimPath {
            stage_handle: handle,
            path: "/World/Sink".into(),
        },
        lunco_core::PortSurfaceReady,
    ));
}

fn edges(app: &mut App) -> Vec<SimConnection> {
    let mut q = app.world_mut().query::<&SimConnection>();
    q.iter(app.world()).cloned().collect()
}

/// Structural endpoint projection derives the edge during the load-time
/// rebuild. Clearing a live connection and marking the derived cache dirty
/// removes it again.
#[test]
fn rewire_derives_at_load_and_clears() {
    let (mut app, id, handle) = setup();
    install_wiring_system(&mut app);

    app.world()
        .non_send::<CanonicalStages>()
        .get(id)
        .unwrap()
        .stage()
        .create_attribute("/World/Sink.inputs:force_y", "float")
        .unwrap()
        .set_connections([SdfPath::new("/World/Src.outputs:netForce").unwrap()])
        .unwrap();
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .drain_all_changes();

    spawn_endpoints(&mut app, handle);
    app.update();

    let projected = edges(&mut app);
    assert_eq!(
        projected.len(),
        1,
        "one SimConnection derived, got {projected:?}"
    );
    let edge = &projected[0];
    let endpoint_entities: Vec<Entity> = {
        let mut q = app.world_mut().query::<(Entity, &UsdPrimPath)>();
        q.iter(app.world())
            .filter(|(_, path)| path.path == "/World/Src" || path.path == "/World/Sink")
            .map(|(entity, _)| entity)
            .collect()
    };
    assert_eq!(endpoint_entities.len(), 2);
    assert_eq!(edge.start_connector, "netForce");
    assert_eq!(edge.end_connector, "force_y");
    assert!(endpoint_entities.contains(&edge.start_element));
    assert!(endpoint_entities.contains(&edge.end_element));
    assert_ne!(edge.start_element, edge.end_element);

    app.world()
        .non_send::<CanonicalStages>()
        .get(id)
        .unwrap()
        .stage()
        .prim(SdfPath::new("/World/Sink").unwrap())
        .attribute("inputs:force_y")
        .set_connections(Vec::<SdfPath>::new())
        .unwrap();
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .drain_all_changes();
    app.world_mut().resource_mut::<WiringDirty>().0 = true;
    app.update();
    assert!(
        edges(&mut app).is_empty(),
        "clearing connectionPaths removes the edge"
    );
}

fn add_connection_and_transform(
    app: &mut App,
    id: AssetId<UsdStageAsset>,
    factor_type: &str,
    factor: openusd::sdf::Value,
    offset: openusd::sdf::Value,
) {
    {
        let stages = app.world().non_send::<CanonicalStages>();
        let stage = stages.get(id).unwrap().stage();
        stage
            .create_attribute("/World/Sink.inputs:force_y", "float")
            .unwrap()
            .set_connections([SdfPath::new("/World/Src.outputs:netForce").unwrap()])
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:factor:force_y", factor_type)
            .unwrap()
            .set(factor)
            .unwrap();
        stage
            .create_attribute("/World/Sink.lunco:offset:force_y", factor_type)
            .unwrap()
            .set(offset)
            .unwrap();
    }
    app.world_mut()
        .non_send_mut::<CanonicalStages>()
        .drain_all_changes();
}

fn assert_transform(factor_type: &str, factor: openusd::sdf::Value, offset: openusd::sdf::Value) {
    let (mut app, id, handle) = setup();
    install_wiring_system(&mut app);
    add_connection_and_transform(&mut app, id, factor_type, factor, offset);
    spawn_endpoints(&mut app, handle);
    app.update();

    let projected = edges(&mut app);
    assert_eq!(
        projected.len(),
        1,
        "one transformed edge derived, got {projected:?}"
    );
    assert_eq!(projected[0].scale, 2.5);
    assert_eq!(projected[0].offset, 0.5);
}

#[test]
fn rewire_applies_factor_and_offset() {
    assert_transform(
        "double",
        openusd::sdf::Value::Double(2.5),
        openusd::sdf::Value::Double(0.5),
    );
}

#[test]
fn rewire_reads_float_authored_transform() {
    assert_transform(
        "float",
        openusd::sdf::Value::Float(2.5),
        openusd::sdf::Value::Float(0.5),
    );
}
