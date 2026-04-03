use bevy::{prelude::*, asset::io::AssetSourceBuilder};
use big_space::prelude::CellCoord;

mod ui;
use lunco_celestial::BlueprintMaterial;
use ui::LunCoUiPlugin;

fn main() {
    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .register_asset_source(
            "cached_textures",
            AssetSourceBuilder::platform_default("../../.cache/textures", None),
        )
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>()) 
        .add_plugins(big_space::prelude::BigSpaceDefaultPlugins)
        .add_plugins(lunco_core::LunCoCorePlugin);

    // THE UNIVERSAL SYNC BRIDGE
    app.add_systems(PreUpdate, global_transform_propagation_system);
    app.add_systems(PostUpdate, global_transform_propagation_system.after(avian3d::prelude::PhysicsSystems::Writeback));

    #[cfg(not(feature = "sandbox"))]
    {
        app.add_plugins(lunco_celestial::CelestialPlugin)
            .insert_resource(ClearColor(Color::BLACK));
    }

    app.add_plugins(MaterialPlugin::<BlueprintMaterial>::default())
        .add_plugins(LunCoUiPlugin) 
        .add_plugins(lunco_physics::LunCoPhysicsPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        .add_plugins(lunco_rover_raycast::LunCoRoverRaycastPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_systems(Update, toggle_slow_motion)
        .run();
}

fn toggle_slow_motion(keyboard: Res<ButtonInput<KeyCode>>, mut time: ResMut<Time<Virtual>>) {
    if keyboard.just_pressed(KeyCode::KeyT) {
        if time.relative_speed() < 1.0 { time.set_relative_speed(1.0); } else { time.set_relative_speed(0.01); }
    }
}

/// A robust multi-pass system to propagate GlobalTransform & Visibility across grids.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    // 1. Initial backfill 
    for ent in q_needs.iter() {
        commands.entity(ent).insert((
            InheritedVisibility::default(),
            ViewVisibility::default(),
            GlobalTransform::default(),
        ));
    }

    // 2. Transform propagation (Manual fallback for TransformPlugin)
    // We do multiple passes to handle hierarchies (up to 4 levels)
    for _ in 0..4 {
        let mut gtf_cache = std::collections::HashMap::new();
        for (ent, gtf, _, _) in q_spatial.iter() {
            gtf_cache.insert(ent, *gtf);
        }

        for (ent, mut gtf, local_tf, child_of_opt) in q_spatial.iter_mut() {
            let parent_gtf = if let Some(child_of) = child_of_opt {
                gtf_cache.get(&child_of.parent()).cloned().unwrap_or_default()
            } else {
                GlobalTransform::default()
            };
            
            let new_gtf = parent_gtf.mul_transform(*local_tf);
            if gtf.to_matrix() != new_gtf.to_matrix() {
                *gtf = new_gtf;
            }
        }
    }

    // 3. Visibility propagation (Boolean sync)
    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() {
            vis_cache.insert(ent, inherited.get());
        }

        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            let parent_visible = if let Some(child_of) = child_of_opt {
                *vis_cache.get(&child_of.parent()).unwrap_or(&true)
            } else {
                true
            };
            
            let is_visible = parent_visible && visibility != Visibility::Hidden;
            if inherited.get() != is_visible {
                *inherited = if is_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
            }
        }
    }
}
