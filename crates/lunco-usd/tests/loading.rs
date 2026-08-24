use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_usd_avian::*;
use lunco_usd_bevy::*;
use lunco_usd_sim::*;

#[test]
fn test_rover_loading_physics() {
    let mut app = App::new();

    // Core Bevy functionality for testing mapping
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());

    // Register types and assets manually to avoid RenderPlugin dependency
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    // UsdSimPlugin's production schedule expects these shared projection
    // resources. The minimal harness does not add the full app plugin stack,
    // so initialize the same resources explicitly instead of allowing a
    // missing-resource panic to hide the wheel contract this test exercises.
    app.init_resource::<UsdStageRevision>();
    app.init_resource::<lunco_modelica::state::ModelicaDocumentRegistry>();
    app.init_resource::<lunco_modelica::state::GeneratedModelicaSources>();
    app.init_resource::<lunco_cosim::BindingRevision>();
    // The avian/sim extractors read the LIVE canonical stage; without
    // `UsdBevyPlugin` (which normally inits it) this minimal harness must
    // provide the resource itself so `get_or_build` can compose off the recipe.
    app.init_non_send::<CanonicalStages>();

    // Add our mapping plugins
    // Note: UsdBevyPlugin might still try to use Hierarchy, so we add it if needed
    // but UsdAvianPlugin/UsdSimPlugin only need Observers and Queries.

    app.add_plugins((UsdAvianPlugin, UsdSimPlugin));

    // 1. Setup a mock USD stage with a Chassis and a Wheel
    // A wheel DECLARES itself with `PhysxVehicleWheelAPI` — the loader detects the
    // applied schema, not the presence of a radius. And every `LunCoWheelAPI` knob is
    // required: they have no fallbacks, in the schema or in Rust.
    //
    // Real wheels get the physical and LunCo wheel values from
    // `components/mobility/wheel.usda` (plus suspension and tire arcs). The vehicle
    // wheel instance applies the standard wheel and attachment APIs because it owns
    // the complete attachment topology. This fixture composes nothing, so it spells
    // out that same complete contract and pins the reader's requirements.
    let usda_content = r#"#usda 1.0
def Xform "Rover" {
    def Cube "Chassis" (
        prepend apiSchemas = ["PhysicsRigidBodyAPI"]
    ) {
        bool physics:rigidBodyEnabled = true
        float physics:mass = 500.0
    }
    def Cylinder "Wheel" (
        prepend apiSchemas = [
            "PhysxVehicleWheelAttachmentAPI", "PhysxVehicleWheelAPI", "LunCoWheelAPI",
            "PhysxVehicleSuspensionAPI", "LunCoSuspensionAPI",
            "PhysxVehicleTireAPI", "PhysicsMaterialAPI",
        ]
    ) {
        uniform token axis = "Z"
        double height = 0.3
        float physxVehicleWheel:radius = 0.4
        int physxVehicleWheelAttachment:index = 0
        float inputs:drive.connect = </Rover.outputs:drive_left>

        # The unified reader (`lunco_usd_sim::wheel_params`) requires the FULL
        # drivetrain set — these previously fell back to Rust constants and are
        # now part of the contract this fixture pins.
        float physxVehicleWheel:mass = 25.0
        float physxVehicleWheel:width = 0.3
        float physxVehicleWheel:moi = 2.0
        float physxVehicleWheel:dampingRate = 0.45
        float physxVehicleWheel:maxBrakeTorque = 1500.0
        float physxVehicleTire:longitudinalStiffness = 8000.0
        float2 physxVehicleTire:lateralStiffnessGraph = (1.0, 1160.0)
        float physxVehicleTire:restLoad = 400.0

        float lunco:suspension:restLength = 0.7
        float physxVehicleSuspension:springStrength = 5000.0
        float physxVehicleSuspension:springDamperRate = 600.0

        float physics:dynamicFriction = 0.8
        float physics:staticFriction = 0.8

        double3 lunco:wheel:steerAxis = (0, 1, 0)
    }
}
"#;

    // Synthetic single-layer stage (no external references) → the live canonical
    // stage builds on demand from this in-memory recipe.
    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let stage_handle = stages.add(UsdStageAsset {
        recipe: Some(StageRecipe::from_source("scene.usda", usda_content)),
    });

    // 2. Spawn the root entity
    app.world_mut().spawn((
        Name::new("Rover"),
        UsdPrimPath {
            stage_handle: stage_handle.clone(),
            path: "/Rover".to_string(),
        },
    ));

    // 3. Manually spawn children to simulate what UsdBevyPlugin would do
    // (We skip UsdBevyPlugin's sync_visuals system to avoid asset dependencies)
    // This test skips `UsdBevyPlugin` (no `instantiate_usd_prim`), so it
    // stands in for that step by spawning the prim entities directly +
    // marking them `UsdVisualSynced` — the trigger `process_usd_avian_prims`
    // / `process_usd_sim_prims` observe to read physics from USD.
    let chassis = app
        .world_mut()
        .spawn((
            Name::new("Chassis"),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: "/Rover/Chassis".to_string(),
            },
            UsdVisualSynced,
        ))
        .id();

    // Create mesh handle first
    let wheel_mesh_handle: Handle<Mesh> = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        meshes.add(Cylinder::new(0.4, 0.3))
    };

    let wheel = app
        .world_mut()
        .spawn((
            Name::new("Wheel"),
            UsdPrimPath {
                stage_handle,
                path: "/Rover/Wheel".to_string(),
            },
            // Mesh3d is required for wheel processing (matches real pipeline behavior)
            Mesh3d(wheel_mesh_handle),
            UsdVisualSynced,
        ))
        .id();

    // 4. Run systems to process mapping
    // Observers trigger on Add, then Update systems run
    app.update();
    app.update();

    // 5. Verify Chassis (Basic Physics)
    let rb = app
        .world()
        .get::<RigidBody>(chassis)
        .expect("Chassis should have RigidBody");
    let mass = app
        .world()
        .get::<Mass>(chassis)
        .expect("Chassis should have Mass");

    assert_eq!(*rb, RigidBody::Dynamic);
    assert_eq!(mass.0, 500.0);

    // 6. Verify Wheel (Intercepted Simulation Physics)
    let wheel_comp = app
        .world()
        .get::<WheelRaycast>(wheel)
        .expect("Wheel should have WheelRaycast");
    assert!((wheel_comp.wheel_radius - 0.4).abs() < 1e-6);
    let susp_comp = app
        .world()
        .get::<Suspension>(wheel)
        .expect("Wheel should have Suspension");
    assert!((susp_comp.spring_k - 5000.0).abs() < 1e-6);

    // 7. Verify Intercept Priority (Wheel should NOT have standard physics)
    assert!(
        app.world().get::<RigidBody>(wheel).is_none(),
        "Intercepted wheel should NOT have standard RigidBody"
    );
}
