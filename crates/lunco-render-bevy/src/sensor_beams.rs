//! Draws range-sensor beams — the render half of `lunco_cosim::RangeSensor`.
//!
//! `lunco-cosim` is a render-free simulation crate: it casts the ray, stores the
//! result (`distance`, `hit`), and declares the *intent* to visualise it
//! (`visualize`). It never names `Gizmos` — doing so pulled
//! `bevy_gizmos → bevy_render → wgpu + naga` into every build, including the
//! `--no-ui` server and the wasm worker.
//!
//! So the beam is drawn here, from the sensor's stored result. **It re-casts
//! nothing** — the sensing already happened on the fixed step; this reads it.
//! That separation is the point: the raycast is simulation and must run headless;
//! the red line is not.
//!
//! See `docs/architecture/render-decoupling.md`.

use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use lunco_cosim::sensors::RangeSensor;

pub(crate) fn build(app: &mut App) {
    // GATED ON THE GIZMO STORE, and it must stay that way. A `Gizmos` system param
    // PANICS when `GizmoPlugin` is absent — which is every `MinimalPlugins` test and
    // every headless app that happens to link this crate. The original beam system
    // in `lunco-cosim` carried this same gate for the same reason; dropping it in the
    // move is what broke three tests here.
    app.add_systems(
        Update,
        draw_range_sensor_beams.run_if(resource_exists::<GizmoConfigStore>),
    );
}

fn draw_range_sensor_beams(
    q: Query<(&RangeSensor, &GlobalTransform)>,
    mut gizmos: Gizmos,
) {
    for (s, transform) in &q {
        if !s.visualize {
            continue;
        }
        let origin = transform.translation().as_dvec3() + transform.rotation().as_dquat() * s.offset;
        let dir_world = transform.rotation().as_dquat() * s.axis;
        // The stored `distance` when the cast hit, else the full range — so the
        // beam shows what the sensor actually reported, not a fresh cast that
        // could disagree with the value the simulation is using.
        let beam = if s.hit { s.distance } else { s.max_distance };
        let end = origin + dir_world * beam;
        let color = if s.hit {
            Color::srgb(1.0, 0.1, 0.1) // hit-locked
        } else {
            Color::srgba(1.0, 0.1, 0.1, 0.4) // out of range
        };
        gizmos.line(origin.as_vec3(), end.as_vec3(), color);
        if s.hit {
            gizmos.sphere(end.as_vec3(), 0.15, color);
        }
    }
}
