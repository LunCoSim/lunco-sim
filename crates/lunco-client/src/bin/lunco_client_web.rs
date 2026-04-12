//! LunCo Client — Web entry point.
//!
//! Web-optimized version of the full celestial simulation client.
//! Loads Earth, planets, orbital mechanics with UI and workbench docking.

use bevy::prelude::*;
use big_space::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
pub fn main() {}

#[cfg(not(target_arch = "wasm32"))]
pub fn main() {
    run();
}

/// Returns BigSpace plugins configured for the current platform.
fn get_big_space_plugins() -> impl bevy::prelude::PluginGroup {
    #[cfg(not(target_arch = "wasm32"))]
    {
        BigSpaceDefaultPlugins.build().disable::<big_space::validation::BigSpaceValidationPlugin>()
    }
    #[cfg(target_arch = "wasm32")]
    {
        BigSpaceDefaultPlugins.build()
    }
}

/// Browser entry point. Called automatically when the WASM module loads.
#[cfg_attr(target_arch = "wasm32", wasm_bindgen(start))]
pub fn run() {
    #[cfg(target_arch = "wasm32")]
    console_error_panic_hook::set_once();

    let mut app = App::new();
    app.insert_resource(Time::<Fixed>::from_hz(60.0))
        .insert_resource(ClearColor(Color::BLACK))
        .add_plugins(DefaultPlugins.build().disable::<TransformPlugin>().set(WindowPlugin {
            primary_window: Some(Window {
                title: "LunCo Client".into(),
                resolution: bevy::window::WindowResolution::new(1280, 720),
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                prevent_default_event_handling: true,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(get_big_space_plugins())
        .add_plugins(lunco_core::LunCoCorePlugin)
        .add_plugins(lunco_celestial::CelestialPlugin)
        .add_plugins(lunco_mobility::LunCoMobilityPlugin)
        .add_plugins(lunco_controller::LunCoControllerPlugin)
        .add_plugins(lunco_avatar::LunCoAvatarPlugin)
        .add_plugins(bevy_workbench::WorkbenchPlugin {
            config: bevy_workbench::WorkbenchConfig {
                show_menu_bar: false,
                show_toolbar: false,
                enable_game_view: true,
                show_console: false,
                ..default()
            },
        });

    // THE UNIVERSAL SYNC BRIDGE
    // Required since TransformPlugin is disabled for BigSpace support.
    app.add_systems(PreUpdate, global_transform_propagation_system);
    app.add_systems(PostUpdate, global_transform_propagation_system.after(avian3d::prelude::PhysicsSystems::Writeback));

    #[cfg(not(target_arch = "wasm32"))]
    app.run();
}

/// A robust multi-pass system to propagate [GlobalTransform] and [Visibility] across grids.
fn global_transform_propagation_system(
    mut commands: Commands,
    q_needs: Query<Entity, (Or<(With<Visibility>, With<Mesh3d>, With<Text2d>, With<Transform>)>, Without<InheritedVisibility>, Without<CellCoord>)>,
    mut q_spatial: Query<(Entity, &mut GlobalTransform, &Transform, Option<&ChildOf>)>,
    mut q_visibility: Query<(Entity, &mut InheritedVisibility, &mut ViewVisibility, &Visibility, Option<&ChildOf>)>,
) {
    for ent in q_needs.iter() {
        commands.entity(ent).insert((
            InheritedVisibility::default(),
            ViewVisibility::default(),
            GlobalTransform::default(),
        ));
    }
    for _ in 0..4 {
        let mut gtf_cache = std::collections::HashMap::new();
        for (ent, gtf, _, _) in q_spatial.iter() { gtf_cache.insert(ent, *gtf); }
        for (_ent, mut gtf, local_tf, child_of_opt) in q_spatial.iter_mut() {
            let parent_gtf = if let Some(child_of) = child_of_opt {
                gtf_cache.get(&child_of.parent()).cloned().unwrap_or_default()
            } else {
                GlobalTransform::default()
            };
            *gtf = parent_gtf.mul_transform(*local_tf);
        }
    }
    for _ in 0..4 {
        let mut vis_cache = std::collections::HashMap::new();
        for (ent, inherited, _, _, _) in q_visibility.iter() { vis_cache.insert(ent, inherited.get()); }
        for (_, mut inherited, _view, visibility, child_of_opt) in q_visibility.iter_mut() {
            let parent_visible = if let Some(child_of) = child_of_opt {
                *vis_cache.get(&child_of.parent()).unwrap_or(&true)
            } else { true };
            let is_visible = parent_visible && visibility != Visibility::Hidden;
            if inherited.get() != is_visible {
                *inherited = if is_visible { InheritedVisibility::VISIBLE } else { InheritedVisibility::HIDDEN };
            }
        }
    }
}
