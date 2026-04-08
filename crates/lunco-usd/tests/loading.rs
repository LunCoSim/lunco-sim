use bevy::prelude::*;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use avian3d::prelude::*;
use lunco_mobility::WheelRaycast;
use std::sync::Arc;
use openusd::usda::TextReader;

#[test]
fn test_rover_loading_physics() {
    let mut app = App::new();
    
    // Core Bevy functionality for testing mapping
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    
    // Register types and assets manually to avoid RenderPlugin dependency
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();
    
    // Add our mapping plugins
    // Note: UsdBevyPlugin might still try to use Hierarchy, so we add it if needed
    // but UsdAvianPlugin/UsdSimPlugin only need Observers and Queries.
    
    app.add_plugins((
        UsdAvianPlugin,
        UsdSimPlugin,
    ));

    // 1. Setup a mock USD stage with a Chassis and a Wheel
    let usda_content = r#"#usda 1.0
def Xform "Rover" {
    def Cube "Chassis" {
        bool physics:rigidBodyEnabled = true
        float physics:mass = 500.0
    }
    def Cylinder "Wheel" {
        float physxVehicleWheel:radius = 0.4
        float physxVehicleSuspension:springStiffness = 5000.0
    }
}
"#;

    let mut parser = openusd::usda::parser::Parser::new(usda_content);
    let data_map = parser.parse().unwrap();
    let reader = Arc::new(TextReader::from_data(data_map));

    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let stage_handle = stages.add(UsdStageAsset { reader });

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
    let chassis = app.world_mut().spawn((
        Name::new("Chassis"),
        UsdPrimPath {
            stage_handle: stage_handle.clone(),
            path: "/Rover/Chassis".to_string(),
        },
    )).id();

    // Create mesh handle first
    let wheel_mesh_handle: Handle<Mesh> = {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        meshes.add(Cylinder::new(0.4, 0.3))
    };

    let wheel = app.world_mut().spawn((
        Name::new("Wheel"),
        UsdPrimPath {
            stage_handle: stage_handle.clone(),
            path: "/Rover/Wheel".to_string(),
        },
        // Mesh3d is required for wheel processing (matches real pipeline behavior)
        Mesh3d(wheel_mesh_handle),
    )).id();

    // 4. Run systems to process mapping
    // Observers trigger on Add, then Update systems run
    app.update();
    app.update(); 

    // 5. Verify Chassis (Basic Physics)
    let rb = app.world().get::<RigidBody>(chassis).expect("Chassis should have RigidBody");
    let mass = app.world().get::<Mass>(chassis).expect("Chassis should have Mass");
    
    assert_eq!(*rb, RigidBody::Dynamic);
    assert_eq!(mass.0, 500.0);

    // 6. Verify Wheel (Intercepted Simulation Physics)
    let wheel_comp = app.world().get::<WheelRaycast>(wheel).expect("Wheel should have WheelRaycast");
    assert!((wheel_comp.wheel_radius - 0.4).abs() < 1e-6);
    assert!((wheel_comp.spring_k - 5000.0).abs() < 1e-6);

    // 7. Verify Intercept Priority (Wheel should NOT have standard physics)
    assert!(app.world().get::<RigidBody>(wheel).is_none(), "Intercepted wheel should NOT have standard RigidBody");
}
