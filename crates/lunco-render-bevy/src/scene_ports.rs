//! Scene properties as connection targets — a light's output and a prim's
//! transform, driven from USD `.connect` wires.
//!
//! These are the render-side properties the simulation legitimately drives. A
//! plume light brightens because the engine is throttled up; a control surface
//! deflects because an actuator moved. Both are CONSEQUENCES of something the
//! solver computed, and both used to be written by a script that sampled a port
//! every tick and pushed the result into a component. This backend makes them
//! ordinary port sinks instead, so the value travels the same graph a thruster
//! force does:
//!
//! ```usda
//! def SphereLight "PlumeLight"
//! {
//!     float inputs:light_intensity.connect = </…/Photometry.outputs:intensity>
//!     float inputs:light_radius.connect    = </…/Photometry.outputs:radius>
//! }
//! ```
//!
//! **This module adds no resolver and no per-frame system**, exactly like
//! [`shader_ports`](crate::shader_ports): `rewire_usd_connections` already turns
//! any `inputs:foo.connect` into a `SimConnection` with no check on what kind of
//! thing the target is, and `propagate_connections` already routes every write
//! through [`PortRegistry::write_port`]. Making a scene property drivable is one
//! registered [`PortBackend`].
//!
//! ## Is a driven `Transform` just animation again?
//!
//! No, and the distinction is worth stating because it is the whole reason the
//! transform ports exist. A transform COMPUTED per tick in a script is animation:
//! the script owns the motion, the numbers live in an interpreted file, and
//! nothing else in the scene can see where they came from. A transform WIRED to a
//! port is a consequence: some model or joint published the number, the wire is
//! visible in the stage, and the value is the same one every other consumer reads.
//! The rule from `AGENTS.md` is unchanged — publish the physical RESULT, wire it,
//! and never re-derive it on the render side.
//!
//! A caveat the author has to keep: a wire onto `translation_*` / `scale_*`
//! competes with anything else that owns the prim's transform (the USD projector,
//! a rigid body, a joint). Drive a transform only on a prim nothing else moves.
//!
//! ## Scalars only, and one direction only
//!
//! A connection carries a single `f64`, so vectors and colours are exposed
//! per-component (`translation_x`, `light_color_r`). Names are snake_case, as
//! everywhere else in the port graph.
//!
//! Every port here is an **In**: [`read_output`](PortBackend::read_output) returns
//! `None`, so a scene property can never be a connection SOURCE. That is what
//! stops a render value feeding back into the simulation — the same discipline
//! that keeps a shader uniform from closing a loop.
//!
//! ## Claiming only what the entity has
//!
//! Registration order IS resolution precedence and plugin add-order is not a
//! contract, so a backend that accepts a name it does not own silently swallows
//! another layer's write and returns `true`, leaving propagation nothing to
//! report. Every op here is gated on the COMPONENT that would receive the value:
//! is accepted provisionally.

use bevy::light::{PointLight, SpotLight};
use bevy::prelude::*;
use lunco_core::ports::{PortBackend, PortDirection, PortRef, PortRegistry};

/// The light ports, in `list` order.
const LIGHT_PORTS: [&str; 5] = [
    "light_intensity",
    "light_radius",
    "light_color_r",
    "light_color_g",
    "light_color_b",
];

/// The transform ports, in `list` order.
const TRANSFORM_PORTS: [&str; 6] = [
    "translation_x",
    "translation_y",
    "translation_z",
    "scale_x",
    "scale_y",
    "scale_z",
];

fn read_light(world: &World, entity: Entity, name: &str) -> Option<f32> {
    if let Some(light) = world.get::<PointLight>(entity) {
        return match name {
            "light_intensity" => Some(light.intensity),
            "light_radius" => Some(light.radius),
            "light_color_r" => Some(light.color.to_linear().red),
            "light_color_g" => Some(light.color.to_linear().green),
            "light_color_b" => Some(light.color.to_linear().blue),
            _ => None,
        };
    }
    if let Some(light) = world.get::<SpotLight>(entity) {
        return match name {
            "light_intensity" => Some(light.intensity),
            "light_radius" => Some(light.radius),
            "light_color_r" => Some(light.color.to_linear().red),
            "light_color_g" => Some(light.color.to_linear().green),
            "light_color_b" => Some(light.color.to_linear().blue),
            _ => None,
        };
    }
    None
}

fn read_transform(world: &World, entity: Entity, name: &str) -> Option<f32> {
    let t = world.get::<Transform>(entity)?;
    match name {
        "translation_x" => Some(t.translation.x),
        "translation_y" => Some(t.translation.y),
        "translation_z" => Some(t.translation.z),
        "scale_x" => Some(t.scale.x),
        "scale_y" => Some(t.scale.y),
        "scale_z" => Some(t.scale.z),
        _ => None,
    }
}

fn read_value(world: &World, entity: Entity, name: &str) -> Option<f32> {
    read_light(world, entity, name).or_else(|| read_transform(world, entity, name))
}

/// True when the value already at `name` is bit-identical to `v`.
///
/// Compared by BITS, not by `==`: a NaN — which `src * factor + offset` in
/// propagation produces the moment a Modelica source diverges — is never equal to
/// itself, so a value comparison would mark the component changed every tick
/// forever. This guard is what keeps a static scene free: mutably dereferencing a
/// `Transform` or a `PointLight` sets `Changed`, and Bevy's transform propagation
/// and light clustering both do real work per change.
fn unchanged(world: &World, entity: Entity, name: &str, v: f32) -> bool {
    read_value(world, entity, name).is_some_and(|cur| cur.to_bits() == v.to_bits())
}

fn write_light(world: &mut World, entity: Entity, name: &str, v: f32) -> bool {
    if !LIGHT_PORTS.contains(&name) {
        return false;
    }
    if unchanged(world, entity, name, v) {
        return true;
    }
    if let Some(mut light) = world.get_mut::<PointLight>(entity) {
        match name {
            "light_intensity" => light.intensity = v,
            "light_radius" => light.radius = v,
            _ => {
                let mut lin = light.color.to_linear();
                match name {
                    "light_color_r" => lin.red = v,
                    "light_color_g" => lin.green = v,
                    "light_color_b" => lin.blue = v,
                    _ => return false,
                }
                light.color = Color::LinearRgba(lin);
            }
        }
        return true;
    }
    if let Some(mut light) = world.get_mut::<SpotLight>(entity) {
        match name {
            "light_intensity" => light.intensity = v,
            "light_radius" => light.radius = v,
            _ => {
                let mut lin = light.color.to_linear();
                match name {
                    "light_color_r" => lin.red = v,
                    "light_color_g" => lin.green = v,
                    "light_color_b" => lin.blue = v,
                    _ => return false,
                }
                light.color = Color::LinearRgba(lin);
            }
        }
        return true;
    }
    false
}

fn write_transform(world: &mut World, entity: Entity, name: &str, v: f32) -> bool {
    if world.get::<Transform>(entity).is_none() || !TRANSFORM_PORTS.contains(&name) {
        return false;
    }
    if unchanged(world, entity, name, v) {
        return true;
    }
    let Some(mut t) = world.get_mut::<Transform>(entity) else {
        return false;
    };
    match name {
        "translation_x" => t.translation.x = v,
        "translation_y" => t.translation.y = v,
        "translation_z" => t.translation.z = v,
        "scale_x" => t.scale.x = v,
        "scale_y" => t.scale.y = v,
        "scale_z" => t.scale.z = v,
        _ => return false,
    }
    true
}

/// Scene properties are **inputs**: something the simulation writes into, never a
/// source another prim reads. See the module docs for why that is not negotiable.
pub(crate) const SCENE_PROPERTY_BACKEND: PortBackend = PortBackend {
    list: |world, entity, out| {
        // Listing exactly what the entity HAS is what keeps `ListPorts` and
        // `write_input` telling the same story: every name reported here is one a
        // write would be accepted for, and no name is hidden because it currently
        // happens to hold a default.
        if world.get::<PointLight>(entity).is_some() || world.get::<SpotLight>(entity).is_some() {
            for name in LIGHT_PORTS {
                out.push(PortRef {
                    name: name.to_string(),
                    direction: PortDirection::In,
                    value: read_light(world, entity, name).unwrap_or(0.0) as f64,
                });
            }
        }
        if world.get::<Transform>(entity).is_some() {
            for name in TRANSFORM_PORTS {
                out.push(PortRef {
                    name: name.to_string(),
                    direction: PortDirection::In,
                    value: read_transform(world, entity, name).unwrap_or(0.0) as f64,
                });
            }
        }
    },
    read_output: |_, _, _| None,
    read_input: |world, entity, name| read_value(world, entity, name).map(|v| v as f64),
    write_input: |world, entity, name, value| {
        let v = value as f32;
        write_light(world, entity, name, v) || write_transform(world, entity, name, v)
    },
    resolve_output: None,
    resolve_input: None,
    read_slot: None,
    write_slot: None,
};

/// Register the scene-property backend.
///
/// Registration order is resolution precedence, and `LuncoRenderPlugin` is added
/// before `CoSimPlugin`, so this sits ahead of the simulation backends. That is
/// safe for one reason only: every op above is gated on the component that would
/// receive the write, and the names are prefixed (`light_*`) or compound
/// (`translation_x`) rather than bare, so there is no simulation port on any prim
/// for this to shadow. Widening a name here — accepting `intensity`, say — would
/// break that guarantee, because `inputs:intensity` is also stock UsdLux.
pub(crate) fn build(app: &mut App) {
    app.init_resource::<PortRegistry>()
        .world_mut()
        .resource_mut::<PortRegistry>()
        .register(SCENE_PROPERTY_BACKEND);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        build(&mut app);
        app
    }

    /// The whole point: a value arriving through the ordinary port graph lands on
    /// a light, with no script and no per-frame system between the two.
    #[test]
    fn a_port_write_drives_a_light() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn(PointLight {
                intensity: 0.0,
                ..default()
            })
            .id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "light_intensity", 680_000.0));
        assert!(reg.write_port(app.world_mut(), e, "light_radius", 0.66));

        let light = app.world().get::<PointLight>(e).unwrap();
        assert_eq!(light.intensity, 680_000.0);
        assert_eq!(light.radius, 0.66);
        assert_eq!(
            reg.read_input_port(app.world(), e, "light_intensity"),
            Some(680_000.0)
        );
    }

    #[test]
    fn a_port_write_drives_a_spot_light() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn(SpotLight {
                intensity: 0.0,
                ..default()
            })
            .id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "light_intensity", 500_000.0));
        assert!(reg.write_port(app.world_mut(), e, "light_radius", 0.5));

        let light = app.world().get::<SpotLight>(e).unwrap();
        assert_eq!(light.intensity, 500_000.0);
        assert_eq!(light.radius, 0.5);
        assert_eq!(
            reg.read_input_port(app.world(), e, "light_intensity"),
            Some(500_000.0)
        );
    }

    /// A scene property must never resolve as a connection SOURCE — that is what
    /// stops a render value being wired back into the simulation.
    #[test]
    fn a_scene_property_is_never_an_output() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((PointLight::default(), Transform::default()))
            .id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert_eq!(
            reg.read_output_port(app.world(), e, "light_intensity"),
            None
        );
        assert_eq!(reg.read_output_port(app.world(), e, "translation_y"), None);
    }

    /// A prim with no `PointLight` is not this backend's business. Returning false
    /// is what lets the next backend claim the name and, failing that, what makes
    /// `propagate_connections` report the wire as dangling instead of eating it.
    #[test]
    fn a_light_port_on_a_prim_with_no_light_is_refused() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(!reg.write_port(app.world_mut(), e, "light_intensity", 1.0));
        // …while the transform on the same entity still works.
        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.0));
    }

    /// A name this backend does not own is refused even when the component is
    /// present. Accepting provisionally would swallow a simulation write and
    /// report success for it.
    #[test]
    fn an_unowned_name_is_refused() {
        let mut app = app();
        let e = app
            .world_mut()
            .spawn((PointLight::default(), Transform::default()))
            .id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(!reg.write_port(app.world_mut(), e, "intensity", 1.0));
        assert!(!reg.write_port(app.world_mut(), e, "throttle", 1.0));
    }

    /// Holding a value must not dirty the component: `Changed<Transform>` drives
    /// Bevy's transform propagation, so a constant wire would re-propagate the
    /// hierarchy every tick for the lifetime of the scene.
    #[test]
    fn rewriting_the_same_value_does_not_dirty_the_component() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.5));
        app.world_mut().clear_trackers();

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.5));
        assert!(!app
            .world()
            .entity(e)
            .get_ref::<Transform>()
            .unwrap()
            .is_changed());

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 3.0));
        assert!(app
            .world()
            .entity(e)
            .get_ref::<Transform>()
            .unwrap()
            .is_changed());
    }

    /// `list` reports exactly what the entity has, so `ListPorts` and `write_input`
    /// cannot disagree about which names exist.
    #[test]
    fn list_reports_only_the_components_present() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();
        let names: Vec<String> = reg
            .entity_ports(app.world(), e)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert!(names.contains(&"scale_y".to_string()));
        assert!(!names.contains(&"light_intensity".to_string()));
    }
}
