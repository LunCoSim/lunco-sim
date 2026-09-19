//! Renderer-independent Inspector readout state and change gating.
//!
//! This package owns only the ECS snapshot needed by Inspector presentation.
//! It deliberately does not depend on egui, USD authoring, or the Workbench
//! shell. The rendered Inspector adapts this resource to its panels.

use bevy::prelude::*;
use lunco_camera_core::camera_display_labels;
use lunco_physics::joint::{JOINT_ANGLE_PORT, joint_angle_holder};
use lunco_port_core::ports::PortRegistry;
use lunco_render::SceneCamera;
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::UsdPrimPath;

/// Live scene-sun readout for the Environment section.
#[derive(Clone)]
pub struct SunReadout {
    pub name: String,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub illuminance: f32,
    pub shadow_maps_enabled: bool,
    pub rgb: [f32; 3],
    pub shadow_first: Option<f32>,
    pub shadow_max: Option<f32>,
}

/// Live joint readout for the selected entity's `angle` port.
#[derive(Clone, Copy)]
pub struct JointReadout {
    pub holder: Entity,
    pub measured: f64,
    pub commanded: f64,
    pub wired: bool,
}

/// Change-driven view-model for the Inspector (WP-8). The Environment,
/// Camera, and Joint sections read query-derived world state that the
/// rendered panel cannot gather during paint; `populate_inspector_view`
/// flattens it at its change-driven update boundary.
#[derive(Resource, Default)]
pub struct InspectorView {
    /// The primary selection used to derive the joint readout.
    pub selected: Option<Entity>,
    /// The presentation camera used to derive exposure and bloom readouts.
    pub active_camera: Option<Entity>,
    /// The unique authored scene sun, if the scene has one.
    pub sun: Option<SunReadout>,
    /// Global ambient brightness, if the resource exists.
    pub ambient_brightness: Option<f32>,
    /// Earthshine fill-light illuminance, if present.
    pub earthshine_lux: Option<f32>,
    /// Active presentation camera's exposure EV100, if any.
    pub exposure_ev100: Option<f32>,
    /// Active presentation camera's bloom intensity, if any.
    pub bloom_intensity: Option<f32>,
    /// Compact shared-policy label for the selected authored camera.
    pub selected_display_name: Option<String>,
    /// Joint readout for the primary-selected entity, if it drives one.
    pub joint: Option<JointReadout>,
}

/// Producer for [`InspectorView`]. Exclusive (needs `&mut World` for the
/// scans + `joint_angle_holder`); runs in `Update` before the egui pass,
/// gated by [`inspector_inputs_changed`] so the world scans are skipped on
/// a quiescent scene. All reads are bounded single-entity lookups or small
/// scans the panel used to do in-paint.
pub fn populate_inspector_view(world: &mut World) {
    use bevy::camera::Exposure;
    use bevy::camera::visibility::RenderLayers;
    use bevy::light::{CascadeShadowConfig, DirectionalLight, GlobalAmbientLight};
    use bevy::post_process::bloom::Bloom;

    // ── Scene sun (skip preview / earthshine lights, same rule as the
    // horizon system's pick_sun).
    let sun_entity = world
        .query_filtered::<Entity, (
            With<DirectionalLight>,
            Without<RenderLayers>,
            Without<lunco_environment::Earthshine>,
        )>()
        .single(world)
        .ok();
    let sun = sun_entity.map(|e| {
        let name = world
            .get::<Name>(e)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        let (yaw_deg, pitch_deg) = world
            .get::<Transform>(e)
            .map(|tf| {
                let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
                (yaw.to_degrees(), pitch.to_degrees())
            })
            .unwrap_or((0.0, 0.0));
        let (illuminance, shadow_maps_enabled, rgb) = world
            .get::<DirectionalLight>(e)
            .map(|l| {
                let lin = l.color.to_linear();
                (
                    l.illuminance,
                    l.shadow_maps_enabled,
                    [lin.red, lin.green, lin.blue],
                )
            })
            .unwrap_or((0.0, false, [1.0, 1.0, 1.0]));
        let (shadow_first, shadow_max) = world
            .get::<CascadeShadowConfig>(e)
            .map(|cfg| {
                (
                    Some(cfg.bounds.first().copied().unwrap_or(40.0)),
                    Some(cfg.bounds.last().copied().unwrap_or(1500.0)),
                )
            })
            .unwrap_or((None, None));
        SunReadout {
            name,
            yaw_deg,
            pitch_deg,
            illuminance,
            shadow_maps_enabled,
            rgb,
            shadow_first,
            shadow_max,
        }
    });

    let ambient_brightness = world
        .get_resource::<GlobalAmbientLight>()
        .map(|a| a.brightness);
    let earthshine_lux = world
        .query_filtered::<&DirectionalLight, With<lunco_environment::Earthshine>>()
        .single(world)
        .ok()
        .map(|l| l.illuminance);

    // ── Camera.
    let active_camera = world
        .get_resource::<lunco_viewport_core::SceneViewport>()
        .and_then(|viewport| viewport.active_camera);
    let exposure_ev100 = active_camera
        .and_then(|entity| world.get::<Exposure>(entity))
        .map(|exposure| exposure.ev100);
    let bloom_intensity = active_camera
        .and_then(|entity| world.get::<Bloom>(entity))
        .map(|bloom| bloom.intensity);

    // ── Joint for the primary-selected entity.
    let selected = world
        .get_resource::<SelectedEntities>()
        .and_then(|s| s.primary());
    let camera_identities: Vec<(Entity, String)> = {
        let mut cameras =
            world.query_filtered::<(Entity, &Name, Option<&UsdPrimPath>), With<SceneCamera>>();
        cameras
            .iter(world)
            .map(|(entity, name, path)| {
                (
                    entity,
                    path.map(|path| path.path.clone())
                        .unwrap_or_else(|| name.as_str().to_string()),
                )
            })
            .collect()
    };
    let camera_names: Vec<String> = camera_identities
        .iter()
        .map(|(_, identity)| identity.clone())
        .collect();
    let camera_labels = camera_display_labels(&camera_names);
    let selected_display_name = selected.and_then(|selected| {
        camera_identities
            .iter()
            .zip(camera_labels)
            .find(|((entity, _), _)| *entity == selected)
            .map(|((_, _), label)| label)
    });
    let joint = if let Some(entity) = selected {
        if let Some(holder) = joint_angle_holder(world, entity) {
            let registry = world.resource::<PortRegistry>().clone();
            let measured = registry
                .read_output_port(world, holder, JOINT_ANGLE_PORT)
                .unwrap_or(0.0);
            let commanded = registry
                .read_input_port(world, holder, JOINT_ANGLE_PORT)
                .unwrap_or(0.0);
            let mut cq = world.query::<&lunco_cosim_core::SimConnection>();
            let wired = cq
                .iter(world)
                .any(|c| c.end_element == holder && c.end_connector == JOINT_ANGLE_PORT);
            Some(JointReadout {
                holder,
                measured,
                commanded,
                wired,
            })
        } else {
            None
        }
    } else {
        None
    };

    let mut view = world.resource_mut::<InspectorView>();
    view.selected = selected;
    view.active_camera = active_camera;
    view.sun = sun;
    view.ambient_brightness = ambient_brightness;
    view.earthshine_lux = earthshine_lux;
    view.exposure_ev100 = exposure_ev100;
    view.bloom_intensity = bloom_intensity;
    view.selected_display_name = selected_display_name;
    view.joint = joint;
}

/// Run condition for [`populate_inspector_view`]: skip the world scans on a
/// quiescent scene, the way the sibling `populate_entity_tree_view`
/// gates on [`super::entity_list::scene_topology_changed`]. Runs when any
/// readout the Inspector shows could have changed — the selection moved, the
/// scene sun / camera exposure / bloom / ambient was edited, or a directional
/// light was removed (despawn) — and keeps running every frame while a joint
/// readout is live (`view.joint.is_some()`) so the measured angle stays fresh
/// during a sim. The `Local` flag forces one initial build (a freshly-added
/// system does not see pre-existing entities as `Changed`). On an idle scene
/// with nothing selected this returns `false` and every scan is skipped.
/// ⚠ **VALUE COMPARISON, NOT `Changed<…>`** — and that is forced, not stylistic.
///
/// This gate used to ask `Changed<Transform>` on the sun. It fired on 296 of 300
/// frames, i.e. it gated nothing while the Inspector paid for a full world scan
/// every frame. Celestial transforms are projection state and can legitimately
/// change as the shared epoch advances, so a component change tick is not the
/// same thing as a changed inspector value. The gate compares the displayed
/// values directly instead.
///
/// So the gate compares the handful of scalars the view actually holds against
/// the world's current values: a few single-entity component reads, versus the
/// producer's `&mut World` scans. When the sun's aim moves by a ULP and the
/// rendered readout would print the same degrees, nothing runs. Selection and
/// ambient use the same value comparison; their Bevy change ticks are not an
/// input because unrelated systems can borrow those resources while leaving
/// the displayed value unchanged.
pub fn inspector_inputs_changed(
    mut first: Local<bool>,
    mut joint_poll: Local<f32>,
    time: Res<Time>,
    view: Res<InspectorView>,
    selection: Res<lunco_scene_selection::SelectedEntities>,
    ambient: Option<Res<bevy::light::GlobalAmbientLight>>,
    // The SAME sun the producer reads (non-preview, non-fill), so the comparison
    // is against the value that would land in the view.
    viewport: Option<Res<lunco_viewport_core::SceneViewport>>,
    lights: Query<
        (
            &Transform,
            &bevy::light::DirectionalLight,
            Option<&bevy::light::CascadeShadowConfig>,
        ),
        (
            Without<lunco_environment::Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    exposures: Query<(Entity, &bevy::camera::Exposure)>,
    blooms: Query<(Entity, &bevy::post_process::bloom::Bloom)>,
    mut removed_lights: RemovedComponents<bevy::light::DirectionalLight>,
) -> bool {
    use bevy::math::EulerRot;

    // Drain the removal buffer every frame (so it doesn't accumulate) and note
    // whether a directional light despawned since last frame.
    let removed = removed_lights.read().count() > 0;

    // The readout is printed to a tenth of a degree / whole lux, so compare at
    // that resolution: a difference the panel cannot show is not a reason to
    // rebuild the view.
    let sun_moved = {
        let live = lights.single().ok().map(|(tf, light, cascades)| {
            let (yaw, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
            let lin = light.color.to_linear();
            (
                yaw.to_degrees(),
                pitch.to_degrees(),
                light.illuminance,
                light.shadow_maps_enabled,
                cascades.map(|c| {
                    (
                        c.bounds.first().copied().unwrap_or(40.0),
                        c.bounds.last().copied().unwrap_or(1500.0),
                    )
                }),
                [lin.red, lin.green, lin.blue],
            )
        });
        match (&view.sun, live) {
            (None, None) => false,
            (Some(cached), Some((yaw, pitch, lux, shadows, shadow_bounds, rgb))) => {
                let shadow_changed = match ((cached.shadow_first, cached.shadow_max), shadow_bounds)
                {
                    ((None, None), None) => false,
                    ((Some(cached_first), Some(cached_max)), Some((live_first, live_max))) => {
                        (cached_first - live_first).abs() > 1.0e-3
                            || (cached_max - live_max).abs() > 1.0e-3
                    }
                    _ => true,
                };
                (cached.yaw_deg - yaw).abs() > 0.05
                    || (cached.pitch_deg - pitch).abs() > 0.05
                    || (cached.illuminance - lux).abs() > 1.0
                    || cached.shadow_maps_enabled != shadows
                    || shadow_changed
                    || cached
                        .rgb
                        .iter()
                        .zip(rgb)
                        .any(|(cached, live)| (cached - live).abs() > 1.0e-4)
            }
            // Appeared or disappeared — the view is stale either way.
            _ => true,
        }
    };

    let camera_changed = {
        let active_camera = viewport.as_deref().and_then(|vp| vp.active_camera);
        let live_ev = active_camera.and_then(|entity| {
            exposures
                .get(entity)
                .ok()
                .map(|(_, exposure)| exposure.ev100)
        });
        let live_bloom = active_camera
            .and_then(|entity| blooms.get(entity).ok().map(|(_, bloom)| bloom.intensity));
        let ev_moved = match (view.exposure_ev100, live_ev) {
            (Some(a), Some(b)) => (a - b).abs() > 1.0e-3,
            (None, None) => false,
            _ => true,
        };
        let bloom_moved = match (view.bloom_intensity, live_bloom) {
            (Some(a), Some(b)) => (a - b).abs() > 1.0e-4,
            (None, None) => false,
            _ => true,
        };
        ev_moved || bloom_moved
    };
    let active_camera = viewport.as_deref().and_then(|vp| vp.active_camera);
    let camera_binding_changed = view.active_camera != active_camera;

    // A joint's measured angle is a continuously changing Avian value, but the
    // Inspector is a human readout rather than a telemetry oscilloscope. Poll
    // it at 10 Hz so the producer remains genuinely gated while the displayed
    // value stays responsive. The old `view.joint.is_some()` clause made the
    // supposedly change-driven system unconditional for every selected joint.
    *joint_poll += time.delta_secs();
    let joint_due = view.joint.is_some() && *joint_poll >= 0.1;
    if joint_due {
        *joint_poll = 0.0;
    }

    let selection_changed = view.selected != selection.primary();
    let ambient_changed = match (
        view.ambient_brightness,
        ambient.as_ref().map(|a| a.brightness),
    ) {
        (None, None) => false,
        (Some(cached), Some(live)) => (cached - live).abs() > 1.0e-4,
        _ => true,
    };

    let run = !*first
        || selection_changed
        || ambient_changed
        || sun_moved
        || camera_changed
        || camera_binding_changed
        || removed
        || joint_due;
    *first = true;
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ProducerRuns(u32);

    fn touch_viewport(mut viewport: ResMut<lunco_viewport_core::SceneViewport>) {
        // A mutable resource borrow marks the resource changed even though its
        // presentation binding remains identical. The Inspector gate must use
        // the binding value, not this incidental change tick.
        let _ = &mut *viewport;
    }

    fn count_producer_run(mut runs: ResMut<ProducerRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn inspector_gate_ignores_unchanged_viewport_binding() {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default())
            .insert_resource(SelectedEntities::default())
            .insert_resource(InspectorView::default())
            .insert_resource(lunco_viewport_core::SceneViewport::default())
            .init_resource::<ProducerRuns>()
            .add_systems(
                Update,
                (
                    touch_viewport,
                    count_producer_run.run_if(inspector_inputs_changed),
                )
                    .chain(),
            );

        app.update();
        app.update();

        assert_eq!(app.world().resource::<ProducerRuns>().0, 1);
    }
}
