//! The sun's angles as connection targets — `sun_azimuth` / `sun_elevation` on a
//! `DirectionalLight`, readable and drivable through the shared
//! [`PortRegistry`](lunco_core::ports::PortRegistry).
//!
//! ```usda
//! float inputs:sun_azimuth.connect = </SolarRoverTest/SunOrbit.outputs:sun_azimuth>
//! ```
//!
//! # Why it lives here and not in the cosim engine
//!
//! These ports used to be an entry in `lunco-cosim`'s **avian** backend table,
//! gated on `w.get::<DirectionalLight>(e)`. A light is not an avian body, and
//! `lunco-cosim` declares no `bevy_light` feature — the code compiled only when
//! some *other* crate in the same build graph happened to switch that feature on
//! (`cargo test -p lunco-controller`, whose graph has no renderer, failed on the
//! unresolved `DirectionalLight`). Feature unification was standing in for a
//! dependency.
//!
//! A backend belongs to the crate that owns the COMPONENT, which is why
//! `register_builtin_port_backends` is documented as an extension point and why
//! `lunco-render-bevy::shader_ports` registers the shader-uniform backend from the
//! material crate. This one is that same move for lighting: this crate already owns
//! the scene's lighting state, already declares `bevy_light` (render-free — the
//! component types, not the pipeline), and is present on the headless server, so a
//! scripted or wired sun still drives with no renderer in the app.
//!
//! No resolver and no per-frame system: `rewire_usd_connections` turns
//! `inputs:sun_azimuth.connect` into a `SimConnection` and `propagate_connections`
//! routes the write through `PortRegistry::write_port`. Registering the backend is
//! the whole integration.

use bevy::light::DirectionalLight;
use bevy::prelude::*;
use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};

/// One named scalar on the sun light. The canonical qualified names are the
/// connection contract for scene-authored lighting data.
struct SunPort {
    name: &'static str,
    read: fn(&World, Entity) -> Option<f64>,
    write: fn(&mut World, Entity, f64) -> bool,
}

const SUN_PORTS: &[SunPort] = &[
    SunPort {
        name: "sun_azimuth",
        read: read_sun_azimuth,
        write: write_sun_azimuth,
    },
    SunPort {
        name: "sun_elevation",
        read: read_sun_elevation,
        write: write_sun_elevation,
    },
];

/// Does this entity carry the gating component? A port backend must answer for
/// EVERY entity, so the light check is the backend's whole membership test.
fn is_sun(world: &World, entity: Entity) -> bool {
    world.get::<DirectionalLight>(entity).is_some()
}

fn port_index(world: &World, entity: Entity, name: &str) -> Option<usize> {
    if !is_sun(world, entity) {
        return None;
    }
    SUN_PORTS.iter().position(|p| p.name == name)
}

fn read_sun_azimuth(world: &World, entity: Entity) -> Option<f64> {
    let tf = world.get::<Transform>(entity)?;
    let (yaw, _, _) = tf.rotation.to_euler(EulerRot::YXZ);
    Some(yaw as f64)
}

fn write_sun_azimuth(world: &mut World, entity: Entity, value: f64) -> bool {
    let Some(mut tf) = world.get_mut::<Transform>(entity) else {
        return false;
    };
    if !value.is_finite() {
        return true;
    }
    let (_, cur_pitch, cur_roll) = tf.rotation.to_euler(EulerRot::YXZ);
    tf.rotation = Quat::from_euler(EulerRot::YXZ, value as f32, cur_pitch, cur_roll);
    let new_tf = *tf;
    if let Some(mut gt) = world.get_mut::<GlobalTransform>(entity) {
        *gt = GlobalTransform::from(new_tf);
    }
    true
}

fn read_sun_elevation(world: &World, entity: Entity) -> Option<f64> {
    let tf = world.get::<Transform>(entity)?;
    let (_, pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
    Some(-pitch as f64)
}

fn write_sun_elevation(world: &mut World, entity: Entity, value: f64) -> bool {
    let Some(mut tf) = world.get_mut::<Transform>(entity) else {
        return false;
    };
    if !value.is_finite() {
        return true;
    }
    let (cur_yaw, _, cur_roll) = tf.rotation.to_euler(EulerRot::YXZ);
    tf.rotation = Quat::from_euler(EulerRot::YXZ, cur_yaw, -value as f32, cur_roll);
    let new_tf = *tf;
    if let Some(mut gt) = world.get_mut::<GlobalTransform>(entity) {
        *gt = GlobalTransform::from(new_tf);
    }
    true
}

/// The sun-light backend. Every port is `InOut`: the angles are readable state AND
/// the thing a wire drives (a Modelica sun-orbit model publishes `sun_azimuth`; the
/// inspector reads the same name back).
const SUN_BACKEND: PortBackend = PortBackend {
    list: |w, e, out| {
        if !is_sun(w, e) {
            return;
        }
        for p in SUN_PORTS {
            // A port whose backing `Transform` is absent simply doesn't list —
            // same rule the avian backend applies to an absent component.
            let Some(value) = (p.read)(w, e) else {
                continue;
            };
            out.push(PortRef {
                name: p.name.to_string(),
                direction: PortDirection::InOut,
                value,
            });
        }
    },
    read_output: |w, e, n| port_index(w, e, n).and_then(|i| (SUN_PORTS[i].read)(w, e)),
    read_input: |w, e, n| port_index(w, e, n).and_then(|i| (SUN_PORTS[i].read)(w, e)),
    write_input: |w, e, n, v| match port_index(w, e, n) {
        Some(i) => (SUN_PORTS[i].write)(w, e, v),
        None => false,
    },
    // Slot fast path: the slot IS the index into `SUN_PORTS`, so a wired sun
    // exchanges by index with one component read per tick and no name scan.
    resolve_output: Some(|w, e, n| port_index(w, e, n).map(|i| i as u64)),
    resolve_input: Some(|w, e, n| port_index(w, e, n).map(|i| i as u64)),
    read_slot: Some(|w, e, slot| {
        let p = SUN_PORTS.get(slot as usize)?;
        is_sun(w, e).then(|| (p.read)(w, e)).flatten()
    }),
    write_slot: Some(|w, e, slot, v| match SUN_PORTS.get(slot as usize) {
        Some(p) if is_sun(w, e) => (p.write)(w, e, v),
        _ => false,
    }),
};

/// Register the sun backend. Called from
/// [`EnvironmentPlugin`](crate::EnvironmentPlugin); `init_resource` first so the
/// order in which this crate and the cosim engine are added does not matter.
pub(crate) fn build(app: &mut App) {
    app.init_resource::<PortRegistry>();
    app.world_mut()
        .resource_mut::<PortRegistry>()
        .register(SUN_BACKEND);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reading and driving the sun goes through the registry — and a non-light
    /// entity is NOT claimed by this backend (the membership test is the gate,
    /// so a rover's `Transform` can never answer to `sun_azimuth`).
    #[test]
    fn sun_angles_read_and_write_through_the_registry() {
        let mut app = App::new();
        build(&mut app);
        let reg = app.world().resource::<PortRegistry>().clone();

        let sun = app
            .world_mut()
            .spawn((DirectionalLight::default(), Transform::default()))
            .id();
        let rover = app.world_mut().spawn(Transform::default()).id();

        assert!(reg.write_port(app.world_mut(), sun, "sun_azimuth", 0.5));
        let read = reg
            .read_output_port(app.world(), sun, "sun_azimuth")
            .expect("the sun must report the angle it was driven to");
        assert!((read - 0.5).abs() < 1e-5, "got {read}");

        assert!(
            !reg.write_port(app.world_mut(), rover, "sun_azimuth", 0.5),
            "an entity with no DirectionalLight is not a sun"
        );
    }
}
