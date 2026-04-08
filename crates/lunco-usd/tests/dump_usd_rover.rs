/// Dump USD rover component state for debugging.
/// Shows EXACTLY what components each entity has and their values.

use bevy::prelude::*;
use bevy::asset::AssetPlugin;
use lunco_usd_bevy::*;
use lunco_usd_avian::*;
use lunco_usd_sim::*;
use avian3d::prelude::*;
use lunco_mobility::WheelRaycast;
use lunco_fsw::FlightSoftware;
use lunco_usd_composer::UsdComposer;
use openusd::usda::TextReader;
use std::sync::Arc;
use std::path::Path;
use big_space::prelude::CellCoord;

#[test]
fn test_dump_usd_rover_state() {
    let usd_path = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap()
        .join("assets/vessels/rovers/sandbox_rover_1.usda");

    let raw = std::fs::read_to_string(&usd_path).unwrap();
    let mut parser = openusd::usda::parser::Parser::new(&raw);
    let data = parser.parse().unwrap();
    let reader = TextReader::from_data(data);
    let composed = UsdComposer::flatten(&reader, usd_path.parent().unwrap()).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdStageAsset>();
    app.init_asset::<Mesh>();
    app.init_asset::<StandardMaterial>();
    app.init_asset::<Image>();
    app.add_plugins((UsdBevyPlugin, UsdAvianPlugin, UsdSimPlugin));

    let mut stages = app.world_mut().resource_mut::<Assets<UsdStageAsset>>();
    let handle = stages.add(UsdStageAsset { reader: Arc::new(composed) });

    // Spawn with position
    let rover = app.world_mut().spawn((
        Name::new("USD_Rover"),
        UsdPrimPath {
            stage_handle: handle,
            path: "/SandboxRover".to_string(),
        },
        Transform::from_translation(Vec3::new(-15.0, 6.0, -10.0)),
        CellCoord::default(),
        Visibility::Visible,
        InheritedVisibility::default(),
        ViewVisibility::default(),
    )).id();

    // Process
    for _ in 0..20 {
        app.update();
    }
    app.world_mut().flush();

    // Dump rover entity
    println!("\n========== ROVER ENTITY (id={:?}) ==========", rover);
    dump_components(&app, rover);
    
    // Check FSW ports
    if let Some(fsw) = app.world().get::<FlightSoftware>(rover) {
        println!("\n========== FSW PORTS ==========");
        for (name, &port_ent) in &fsw.port_map {
            println!("  {} -> {:?}", name, port_ent);
        }
    }
    
    // Dump wires
    println!("\n========== ALL WIRES ==========");
    {
        let mut q_wires = app.world_mut().query_filtered::<(Entity, &lunco_core::architecture::Wire), With<lunco_core::architecture::Wire>>();
        for (wire_ent, wire) in q_wires.iter(app.world()) {
            let src_name = app.world().get::<Name>(wire.source).map(|n| n.as_str().to_string()).unwrap_or_else(|| "unknown".to_string());
            let tgt_name = app.world().get::<Name>(wire.target).map(|n| n.as_str().to_string()).unwrap_or_else(|| "unknown".to_string());
            println!("  Wire {:?}: {} ({:?}) -> {} ({:?}) scale={}",
                wire_ent, src_name, wire.source, tgt_name, wire.target, wire.scale);
        }
    }

    // Dump children
    let child_entities: Vec<Entity> = app.world().get::<Children>(rover)
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    println!("\n========== CHILDREN (count={}) ==========", child_entities.len());
    for child in child_entities {
        if let Some(name) = app.world().get::<Name>(child) {
            let name_str = name.as_str();
            println!("\n--- Entity {:?}: {} ---", child, name_str);
            dump_components(&app, child);

            // Check for visual grandchildren
            let gc_entities: Vec<Entity> = app.world().get::<Children>(child)
                .map(|c| c.iter().collect())
                .unwrap_or_default();
            if !gc_entities.is_empty() {
                println!("  └─ Grandchildren (count={})", gc_entities.len());
                for gc in gc_entities {
                    if let Some(gc_name) = app.world().get::<Name>(gc) {
                        println!("     └─ Entity {:?}: {}", gc, gc_name.as_str());
                        dump_components(&app, gc);
                    }
                }
            }

            // Print WheelRaycast wiring details
            if let Some(wheel) = app.world().get::<WheelRaycast>(child) {
                let drive_port = wheel.drive_port;
                let steer_port = wheel.steer_port;
                let visual_ent = wheel.visual_entity;
                println!("  WheelRaycast wiring: drive_port={:?}, steer_port={:?}, visual={:?}",
                    drive_port, steer_port, visual_ent);
                // Check what wires connect to this wheel's drive_port
                let mut q_wires = app.world_mut().query_filtered::<(Entity, &lunco_core::architecture::Wire), With<lunco_core::architecture::Wire>>();
                for (wire_ent, wire) in q_wires.iter(app.world()) {
                    if wire.target == drive_port {
                        println!("    Wire {:?}: source={:?} -> target={:?} (scale={})",
                            wire_ent, wire.source, wire.target, wire.scale);
                        // Find what digital port this is
                        if let Some(name) = app.world().get::<Name>(wire.source) {
                            println!("      Source name: {}", name.as_str());
                        }
                    }
                }
            }
        }
    }

    // Count wheels with WheelRaycast
    let mut wheel_count = 0;
    let mut wheel_details: Vec<(String, Vec3, Quat, Option<Dir3>)> = Vec::new();
    if let Some(children) = app.world().get::<Children>(rover) {
        for child in children.iter() {
            if let Some(wheel) = app.world().get::<WheelRaycast>(child) {
                wheel_count += 1;
                let name = app.world().get::<Name>(child).map(|n| n.as_str().to_string()).unwrap_or_default();
                let tf = app.world().get::<Transform>(child).cloned().unwrap_or_default();
                let rc_dir = app.world().get::<RayCaster>(child).map(|r| r.direction);
                wheel_details.push((name, tf.translation, tf.rotation, rc_dir));
            }
        }
    }
    println!("\n========== SUMMARY ==========");
    println!("Wheels with WheelRaycast: {}", wheel_count);
    for (name, pos, rot, rc_dir) in &wheel_details {
        println!("  {} - pos={:?}, rot={:?}, ray_dir={:?}", name, pos, rot, rc_dir);
    }

    // Verify wheel transforms
    for (name, _pos, rot, _rc_dir) in &wheel_details {
        let angle_from_identity = rot.angle_between(Quat::IDENTITY);
        let angle_from_90z = rot.angle_between(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2));
        println!("\n  {} rotation analysis:", name);
        println!("    Angle from IDENTITY: {:.2}°", angle_from_identity.to_degrees());
        println!("    Angle from 90° Z: {:.2}°", angle_from_90z.to_degrees());
    }
}

fn dump_components(app: &App, entity: Entity) {
    if let Some(tf) = app.world().get::<Transform>(entity) {
        println!("  Transform: pos={:?}, rot={:?} ({:.2}° from identity)",
            tf.translation, tf.rotation, tf.rotation.angle_between(Quat::IDENTITY).to_degrees());
    }
    if let Some(rb) = app.world().get::<RigidBody>(entity) {
        println!("  RigidBody: {:?}", rb);
    } else {
        println!("  RigidBody: NONE");
    }
    if let Some(mass) = app.world().get::<Mass>(entity) {
        println!("  Mass: {}", mass.0);
    }
    if let Some(ld) = app.world().get::<LinearDamping>(entity) {
        println!("  LinearDamping: {}", ld.0);
    }
    if let Some(ad) = app.world().get::<AngularDamping>(entity) {
        println!("  AngularDamping: {}", ad.0);
    }
    if let Some(col) = app.world().get::<Collider>(entity) {
        if let Some(cuboid) = col.shape().as_cuboid() {
            println!("  Collider: cuboid half_extents={:?}", cuboid.half_extents);
        }
    } else {
        println!("  Collider: NONE");
    }
    if app.world().get::<lunco_core::Vessel>(entity).is_some() {
        println!("  Vessel: YES");
    }
    if app.world().get::<lunco_core::RoverVessel>(entity).is_some() {
        println!("  RoverVessel: YES");
    }
    if let Some(wheel) = app.world().get::<WheelRaycast>(entity) {
        println!("  WheelRaycast: radius={}, rest={}, k={}, c={}",
            wheel.wheel_radius, wheel.rest_length, wheel.spring_k, wheel.damping_c);
    }
    if let Some(rc) = app.world().get::<RayCaster>(entity) {
        println!("  RayCaster: dir={:?}", rc.direction);
    }
    if app.world().get::<Mesh3d>(entity).is_some() {
        println!("  Mesh3d: YES");
    } else {
        println!("  Mesh3d: NONE");
    }
}
