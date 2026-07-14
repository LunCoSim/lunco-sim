use bevy::prelude::*;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use avian3d::prelude::*;
use lunco_mobility::WheelRaycast;

fn main() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());

    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<Image>();
    // The avian/sim extractors read the LIVE canonical stage; without
    // `UsdBevyPlugin` this harness must provide the resource itself.
    app.init_non_send::<CanonicalStages>();

    app.add_plugins((
        UsdAvianPlugin,
        UsdSimPlugin,
    ));

    println!("\n--- Loading Rucheyok Rover Physics ---");

    // Compose from disk (synchronous, no async AssetServer) so the referenced
    // wheel / panel attributes the physics mapping reads are resolved, then
    // publish the composed stage as the live canonical stage.
    let path = "assets/vessels/rovers/rucheyok/rucheyok.usda";
    let stage_handle = {
        let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
        stages.add(UsdStageAsset { recipe: None })
    };
    let stage = compose_file_to_stage(std::path::Path::new(path))
        .expect("Failed to compose rucheyok.usda");
    let cstage = CanonicalStage::from_stage(stage, path.to_string());
    app.world_mut()
        .get_non_send_mut::<CanonicalStages>()
        .expect("CanonicalStages resource")
        .insert(stage_handle.id(), cstage);

    // Spawn manually
    let entities = vec![
        ("Chassis", "/Rucheyok/Chassis"),
        ("Wheel_FL", "/Rucheyok/Wheel_FL"),
        ("Wheel_FR", "/Rucheyok/Wheel_FR"),
        ("Wheel_RL", "/Rucheyok/Wheel_RL"),
        ("Wheel_RR", "/Rucheyok/Wheel_RR"),
    ];

    let mut spawned = Vec::new();
    for (name, prim_path) in entities {
        let id = app.world_mut().spawn((
            Name::new(name.to_string()),
            UsdPrimPath {
                stage_handle: stage_handle.clone(),
                path: prim_path.to_string(),
            },
        )).id();
        spawned.push(id);
    }

    // Process mapping observers
    app.update();

    println!("\n--- Physical Mapping Report ---\n");

    for entity in spawned {
        let name = app.world().get::<Name>(entity).unwrap();
        let rb = app.world().get::<RigidBody>(entity);
        let wheel = app.world().get::<WheelRaycast>(entity);
        let mass = app.world().get::<Mass>(entity);

        let mut report = format!("Entity: {:<15}", name);
        
        if let Some(_) = rb { 
            let m = mass.map(|m| m.0).unwrap_or(0.0);
            report.push_str(&format!(" | [Avian] RigidBody (Mass: {}kg)", m)); 
        }
        
        if let Some(w) = wheel { 
            report.push_str(&format!(" | [Sim] WheelRaycast (Radius: {}m, Spring K: {})", w.wheel_radius, w.spring_k)); 
        }
        
        println!("{}", report);
    }
    
    println!("\n--- Inspection Complete ---");
}
