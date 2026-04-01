use bevy::prelude::*;

mod blueprint_extension;
mod ui;
use blueprint_extension::BlueprintMaterial;
use ui::LunCoSimUiPlugin;

fn main() {
    let mut app = App::new();
    // CRITICAL: We are using big_space 0.12.0 for higher-precision planetary physics.
    // big_space REQUIREs Bevy's default TransformPlugin to be disabled to avoid 
    // coordinate fighting. However, disabling it 'blinds' the standard UI hit-testing.
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>()) 
        .add_plugins(lunco_sim_core::LunCoSimCorePlugin);

    // THE GOLDEN BRIDGE: This system manually calculates 'GlobalTransform' for 
    // entities that are NOT part of the big_space grid (like Windows and UI Cameras).
    // This allows bevy_egui and bevy_ui to continue 'hearing' mouse clicks even
    // when the engine's default transform plugin is turned off.
    app.add_systems(Update, fix_spatial_components_for_non_grid_entities);

    #[cfg(feature = "sandbox")]
    {
        // Sandbox features currently disabled to focus on celestial stabilization
    }

    #[cfg(not(feature = "sandbox"))]
    {
        app.add_plugins(lunco_sim_celestial::CelestialPlugin)
            .insert_resource(ClearColor(Color::BLACK));
    }

    app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoSimUiPlugin) 
        .add_systems(Update, toggle_slow_motion)
        .run();
}


fn toggle_slow_motion(keyboard: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    if keyboard.just_pressed(KeyCode::KeyT) {
        if time.relative_speed() < 1.0 {
            time.set_relative_speed(1.0);
        } else {
            time.set_relative_speed(0.01);
        }
    }
}

/// THE RECOVERY BRIDGE MAPPING: 
/// In Bevy with big_space, TransformPlugin is DISABLED engine-wide.
/// This means GlobalTransform is NOT updated automatically.
/// 
/// However, bevy_egui and bevy_ui DEPEND on GlobalTransform to perform hit-tests
/// between the Cursor and the Windows/Buttons. This system manually 'backfills' 
/// those components for entities that are OUTSIDE of the big_space grid system 
/// (like the Window itself and the 2D HUD Camera). 
fn fix_spatial_components_for_non_grid_entities(
    mut commands: Commands,
    mut set: ParamSet<(
        Query<(Entity, &Transform, &mut GlobalTransform, Option<&ChildOf>), Without<big_space::prelude::CellCoord>>,
        Query<(Entity, &GlobalTransform)>,
    )>,
    q_non_spatial: Query<Entity, (Or<(With<Window>, With<Visibility>)>, Without<GlobalTransform>, Without<big_space::prelude::CellCoord>)>,
) {
    // 1. Correct components for windows/ui if missing
    for entity in q_non_spatial.iter() {
        commands.entity(entity).insert((Transform::default(), GlobalTransform::default(), InheritedVisibility::default(), ViewVisibility::default()));
    }

    // 2. Propagate passes
    for _ in 0..3 {
        let mut gtfs = std::collections::HashMap::new();
        for (entity, gtf) in set.p1().iter() {
            gtfs.insert(entity, *gtf);
        }

        for (entity, transform, mut global_transform, child_of_opt) in set.p0().iter_mut() {
            let mut parent_gtf_val = None;
            if let Some(child_of) = child_of_opt {
                if let Some(parent_gtf) = gtfs.get(&child_of.parent()) {
                    parent_gtf_val = Some(*parent_gtf);
                }
            }

            let new_gtf = if let Some(parent_gtf) = parent_gtf_val {
                parent_gtf.mul_transform(*transform)
            } else {
                GlobalTransform::from(*transform)
            };

            if *global_transform != new_gtf {
                *global_transform = new_gtf;
                gtfs.insert(entity, new_gtf);
            }
        }
    }
}
