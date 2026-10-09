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
//! `lunco-usd-sim-shader`'s `ports`: `rewire_usd_connections` already turns
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
use lunco_core::GlobalEntityId;
use lunco_engineering_values::{Dimension, Unit, UnitReference, UnitScaleExactness};
use lunco_port_core::ports::{PortBackend, PortDirection, PortMetadata, PortRegistry};

/// The light ports, in declaration order.
const LIGHT_PORTS: [&str; 5] = [
    "light_intensity",
    "light_radius",
    "light_color_r",
    "light_color_g",
    "light_color_b",
];

/// The transform ports, in declaration order.
const TRANSFORM_PORTS: [&str; 6] = [
    "translation_x",
    "translation_y",
    "translation_z",
    "scale_x",
    "scale_y",
    "scale_z",
];

#[repr(u64)]
#[derive(Clone, Copy)]
enum ScenePropertySlot {
    LightIntensity,
    LightRadius,
    LightColorR,
    LightColorG,
    LightColorB,
    TranslationX,
    TranslationY,
    TranslationZ,
    ScaleX,
    ScaleY,
    ScaleZ,
}

impl ScenePropertySlot {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "light_intensity" => Self::LightIntensity,
            "light_radius" => Self::LightRadius,
            "light_color_r" => Self::LightColorR,
            "light_color_g" => Self::LightColorG,
            "light_color_b" => Self::LightColorB,
            "translation_x" => Self::TranslationX,
            "translation_y" => Self::TranslationY,
            "translation_z" => Self::TranslationZ,
            "scale_x" => Self::ScaleX,
            "scale_y" => Self::ScaleY,
            "scale_z" => Self::ScaleZ,
            _ => return None,
        })
    }

    fn from_slot(slot: u64) -> Option<Self> {
        Some(match slot {
            0 => Self::LightIntensity,
            1 => Self::LightRadius,
            2 => Self::LightColorR,
            3 => Self::LightColorG,
            4 => Self::LightColorB,
            5 => Self::TranslationX,
            6 => Self::TranslationY,
            7 => Self::TranslationZ,
            8 => Self::ScaleX,
            9 => Self::ScaleY,
            10 => Self::ScaleZ,
            _ => return None,
        })
    }
}

fn read_light_slot(world: &World, entity: Entity, slot: ScenePropertySlot) -> Option<f32> {
    let read = |color: Color, intensity: f32, radius: f32| match slot {
        ScenePropertySlot::LightIntensity => Some(intensity),
        ScenePropertySlot::LightRadius => Some(radius),
        ScenePropertySlot::LightColorR => Some(color.to_linear().red),
        ScenePropertySlot::LightColorG => Some(color.to_linear().green),
        ScenePropertySlot::LightColorB => Some(color.to_linear().blue),
        _ => None,
    };
    if let Some(light) = world.get::<PointLight>(entity) {
        return read(light.color, light.intensity, light.radius);
    }
    world
        .get::<SpotLight>(entity)
        .and_then(|light| read(light.color, light.intensity, light.radius))
}

fn read_transform_slot(world: &World, entity: Entity, slot: ScenePropertySlot) -> Option<f32> {
    let transform = world.get::<Transform>(entity)?;
    Some(match slot {
        ScenePropertySlot::TranslationX => transform.translation.x,
        ScenePropertySlot::TranslationY => transform.translation.y,
        ScenePropertySlot::TranslationZ => transform.translation.z,
        ScenePropertySlot::ScaleX => transform.scale.x,
        ScenePropertySlot::ScaleY => transform.scale.y,
        ScenePropertySlot::ScaleZ => transform.scale.z,
        _ => return None,
    })
}

fn read_value_slot(world: &World, entity: Entity, slot: ScenePropertySlot) -> Option<f32> {
    read_light_slot(world, entity, slot).or_else(|| read_transform_slot(world, entity, slot))
}

fn read_input(world: &World, entity: Entity, name: &str) -> Option<f64> {
    read_value_slot(world, entity, ScenePropertySlot::from_name(name)?).map(f64::from)
}

fn resolve_input_slot(world: &World, entity: Entity, name: &str) -> Option<u64> {
    let slot = ScenePropertySlot::from_name(name)?;
    read_value_slot(world, entity, slot)?;
    Some(slot as u64)
}

fn write_light_slot(
    world: &mut World,
    entity: Entity,
    slot: ScenePropertySlot,
    value: f32,
) -> bool {
    let Some(current) = read_light_slot(world, entity, slot) else {
        return false;
    };
    if current.to_bits() == value.to_bits() {
        return true;
    }
    macro_rules! update_light {
        ($component:ty) => {
            if let Some(mut light) = world.get_mut::<$component>(entity) {
                match slot {
                    ScenePropertySlot::LightIntensity => light.intensity = value,
                    ScenePropertySlot::LightRadius => light.radius = value,
                    ScenePropertySlot::LightColorR
                    | ScenePropertySlot::LightColorG
                    | ScenePropertySlot::LightColorB => {
                        let mut color = light.color.to_linear();
                        match slot {
                            ScenePropertySlot::LightColorR => color.red = value,
                            ScenePropertySlot::LightColorG => color.green = value,
                            ScenePropertySlot::LightColorB => color.blue = value,
                            _ => return false,
                        }
                        light.color = Color::LinearRgba(color);
                    }
                    _ => return false,
                }
                return true;
            }
        };
    }
    update_light!(PointLight);
    update_light!(SpotLight);
    false
}

fn write_transform_slot(
    world: &mut World,
    entity: Entity,
    slot: ScenePropertySlot,
    value: f32,
) -> bool {
    let Some(current) = read_transform_slot(world, entity, slot) else {
        return false;
    };
    if current.to_bits() == value.to_bits() {
        return true;
    }
    let Some(mut transform) = world.get_mut::<Transform>(entity) else {
        return false;
    };
    match slot {
        ScenePropertySlot::TranslationX => transform.translation.x = value,
        ScenePropertySlot::TranslationY => transform.translation.y = value,
        ScenePropertySlot::TranslationZ => transform.translation.z = value,
        ScenePropertySlot::ScaleX => transform.scale.x = value,
        ScenePropertySlot::ScaleY => transform.scale.y = value,
        ScenePropertySlot::ScaleZ => transform.scale.z = value,
        _ => return false,
    }
    true
}

fn write_scene_property_slot(world: &mut World, entity: Entity, slot: u64, value: f64) {
    let slot = ScenePropertySlot::from_slot(slot)
        .expect("prepared scene-property slot remains live through commit");
    write_scene_property(world, entity, slot, value)
}

fn write_scene_property(world: &mut World, entity: Entity, slot: ScenePropertySlot, value: f64) {
    // USD scene-property ports target Bevy's native `f32` Transform and light
    // components. Their metadata bounds writes to the finite `f32` range before
    // this explicit storage-boundary conversion.
    let scene_value_f32 = value as f32;
    assert!(
        scene_value_f32.is_finite(),
        "validated scene property must fit f32"
    );
    let applied = match slot {
        ScenePropertySlot::LightIntensity
        | ScenePropertySlot::LightRadius
        | ScenePropertySlot::LightColorR
        | ScenePropertySlot::LightColorG
        | ScenePropertySlot::LightColorB => write_light_slot(world, entity, slot, scene_value_f32),
        ScenePropertySlot::TranslationX
        | ScenePropertySlot::TranslationY
        | ScenePropertySlot::TranslationZ
        | ScenePropertySlot::ScaleX
        | ScenePropertySlot::ScaleY
        | ScenePropertySlot::ScaleZ => write_transform_slot(world, entity, slot, scene_value_f32),
    };
    assert!(
        applied,
        "prepared scene-property slot must commit successfully"
    );
}

fn transform_translation_frame(
    world: &World,
    entity: Entity,
) -> Option<lunco_engineering_values::CoordinateFrameId> {
    let frame = if let Some(parent) = world.get::<ChildOf>(entity) {
        let identity = world.get::<GlobalEntityId>(parent.parent())?.get();
        format!("lunco:entity/{identity}/local")
    } else {
        let identity = world.get::<GlobalEntityId>(entity)?.get();
        format!("lunco:scene/{identity}/world")
    };
    lunco_engineering_values::CoordinateFrameId::new(frame).ok()
}

fn meters() -> UnitReference {
    UnitReference::resolved(
        Unit::new_with_exactness("m", Dimension::LENGTH, 1.0, 0.0, UnitScaleExactness::Exact)
            .expect("meter unit definition is valid"),
    )
}

fn lumens() -> UnitReference {
    UnitReference::resolved(
        Unit::new_with_exactness(
            "lm",
            Dimension([0, 0, 0, 0, 0, 0, 1]),
            1.0,
            0.0,
            UnitScaleExactness::Exact,
        )
        .expect("lumen unit definition is valid"),
    )
}

fn dimensionless() -> UnitReference {
    UnitReference::resolved(
        Unit::new_with_exactness("1", Dimension::NONE, 1.0, 0.0, UnitScaleExactness::Exact)
            .expect("dimensionless unit definition is valid"),
    )
}

fn scene_property_metadata(
    world: &World,
    entity: Entity,
    name: &str,
    direction: PortDirection,
) -> PortMetadata {
    let float_max = f32::MAX as f64;
    let slot = ScenePropertySlot::from_name(name)
        .expect("scene property metadata is requested only for a declared slot");
    let (unit, min, max, frame) = match slot {
        ScenePropertySlot::LightIntensity => (Some(lumens()), Some(0.0), Some(float_max), None),
        ScenePropertySlot::LightRadius => (Some(meters()), Some(0.0), Some(float_max), None),
        ScenePropertySlot::LightColorR
        | ScenePropertySlot::LightColorG
        | ScenePropertySlot::LightColorB => (Some(dimensionless()), None, Some(float_max), None),
        ScenePropertySlot::TranslationX
        | ScenePropertySlot::TranslationY
        | ScenePropertySlot::TranslationZ => (
            Some(meters()),
            Some(-float_max),
            Some(float_max),
            transform_translation_frame(world, entity),
        ),
        ScenePropertySlot::ScaleX | ScenePropertySlot::ScaleY | ScenePropertySlot::ScaleZ => (
            Some(dimensionless()),
            Some(-float_max),
            Some(float_max),
            None,
        ),
    };
    PortMetadata::scalar(
        direction,
        unit,
        min,
        max,
        "USD scene property",
        "scene author",
        true,
        frame,
    )
}

/// True when the value already at `name` is bit-identical to `v`.
///
/// Compared by BITS, not by `==`: a NaN — which `src * factor + offset` in
/// propagation produces the moment a Modelica source diverges — is never equal to
/// itself, so a value comparison would mark the component changed every tick
/// forever. This guard is what keeps a static scene free: mutably dereferencing a
/// `Transform` or a `PointLight` sets `Changed`, and Bevy's transform propagation
/// and light clustering both do real work per change.
/// Scene properties are **inputs**: something the simulation writes into, never a
/// source another prim reads. See the module docs for why that is not negotiable.
pub(crate) const SCENE_PROPERTY_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<PointLight>>()
                .iter(world),
        );
        out.extend(
            world
                .query_filtered::<Entity, With<SpotLight>>()
                .iter(world),
        );
        out.extend(
            world
                .query_filtered::<Entity, With<Transform>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        (if world.get::<PointLight>(entity).is_some() {
            1
        } else {
            0
        }) | (if world.get::<SpotLight>(entity).is_some() {
            1 << 1
        } else {
            0
        }) | (if world.get::<Transform>(entity).is_some() {
            1 << 2
        } else {
            0
        })
    },
    declare_ports: |world, entity, out| {
        // Listing exactly what the entity HAS is what keeps `ListPorts` and
        // the resolved slot writer telling the same story: every name reported here is one a
        // write would be accepted for, and no name is hidden because it currently
        // happens to hold a default.
        if world.get::<PointLight>(entity).is_some() || world.get::<SpotLight>(entity).is_some() {
            for name in LIGHT_PORTS {
                out.declare(name, PortDirection::In);
            }
        }
        if world.get::<Transform>(entity).is_some() {
            for name in TRANSFORM_PORTS {
                out.declare(name, PortDirection::In);
            }
        }
    },
    metadata: scene_property_metadata,
    read_output: |_, _, _| None,
    read_input,
    resolve_output: None,
    resolve_input: Some(resolve_input_slot),
    read_slot: None,
    read_input_slot: Some(|world, entity, slot| {
        read_value_slot(world, entity, ScenePropertySlot::from_slot(slot)?).map(f64::from)
    }),
    write_slot: Some(write_scene_property_slot),
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
///
/// Installs the Bevy scene-property port backend.
///
/// The backend exposes authored scene properties such as light channels and
/// transform components as writable simulation sinks. It is an application
/// adapter and is therefore composed explicitly by hosts that install the
/// complete USD runtime.
pub struct UsdScenePortsPlugin;

impl Plugin for UsdScenePortsPlugin {
    fn build(&self, app: &mut App) {
        build(app);
    }
}

fn build(app: &mut App) {
    app.add_observer(mark_point_light_surface_ready)
        .add_observer(mark_spot_light_surface_ready)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<PointLight>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<PointLight>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<SpotLight>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<SpotLight>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_add::<Transform>)
        .add_observer(lunco_port_core::ports::bump_port_topology_on_remove::<Transform>)
        .init_resource::<PortRegistry>()
        .init_resource::<lunco_port_core::ports::PortTopologyRevision>()
        .init_resource::<lunco_port_core::ports::PortTopologyState>()
        .world_mut()
        .resource_mut::<PortRegistry>()
        .register(SCENE_PROPERTY_BACKEND);
}

/// Publish the generic port-surface lifecycle event after Bevy has installed a
/// light component.  USD projection and runtime-created lights use the same
/// backend, so this belongs at the scene-property boundary rather than in a
/// campaign scene or a connection special case.
fn mark_point_light_surface_ready(trigger: On<Add, PointLight>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_port_core::PortSurfaceReady);
}

fn mark_spot_light_surface_ready(trigger: On<Add, SpotLight>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_port_core::PortSurfaceReady);
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

        assert!(
            reg.write_port(app.world_mut(), e, "light_intensity", 680_000.0)
                .is_ok()
        );
        assert!(
            reg.write_port(app.world_mut(), e, "light_radius", 0.66)
                .is_ok()
        );

        let light = app.world().get::<PointLight>(e).unwrap();
        assert_eq!(light.intensity, 680_000.0);
        assert_eq!(light.radius, 0.66);
        assert_eq!(
            reg.read_input_port(app.world(), e, "light_intensity"),
            Some(680_000.0)
        );
        let resolved = reg
            .resolve_input(app.world(), e, "light_intensity")
            .expect("light input resolves once to a typed slot");
        assert_eq!(
            reg.read_resolved(app.world(), e, &resolved),
            Some(680_000.0)
        );
        assert!(
            reg.write_resolved(app.world_mut(), e, &resolved, 700_000.0)
                .is_ok()
        );
        assert_eq!(
            app.world().get::<PointLight>(e).unwrap().intensity,
            700_000.0
        );
    }

    #[test]
    fn adding_a_light_publishes_its_port_surface_lifecycle() {
        let mut app = app();
        let e = app.world_mut().spawn_empty().id();

        app.world_mut().entity_mut(e).insert(PointLight::default());
        app.update();

        assert!(
            app.world()
                .get::<lunco_port_core::PortSurfaceReady>(e)
                .is_some()
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

        assert!(
            reg.write_port(app.world_mut(), e, "light_intensity", 500_000.0)
                .is_ok()
        );
        assert!(
            reg.write_port(app.world_mut(), e, "light_radius", 0.5)
                .is_ok()
        );

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

    /// A prim with no `PointLight` is not this backend's business. Returning an error
    /// is what lets the next backend claim the name and, failing that, what makes
    /// `propagate_connections` report the wire as dangling instead of eating it.
    #[test]
    fn a_light_port_on_a_prim_with_no_light_is_refused() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();
        assert!(
            reg.write_port(app.world_mut(), e, "light_intensity", 1.0)
                .is_err()
        );
        // …while the transform on the same entity still works.
        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.0).is_ok());
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
        assert!(
            reg.write_port(app.world_mut(), e, "intensity", 1.0)
                .is_err()
        );
        assert!(reg.write_port(app.world_mut(), e, "throttle", 1.0).is_err());
    }

    /// Holding a value must not dirty the component: `Changed<Transform>` drives
    /// Bevy's transform propagation, so a constant wire would re-propagate the
    /// hierarchy every tick for the lifetime of the scene.
    #[test]
    fn rewriting_the_same_value_does_not_dirty_the_component() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.5).is_ok());
        app.world_mut().clear_trackers();

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 2.5).is_ok());
        assert!(
            !app.world()
                .entity(e)
                .get_ref::<Transform>()
                .unwrap()
                .is_changed()
        );

        assert!(reg.write_port(app.world_mut(), e, "scale_y", 3.0).is_ok());
        assert!(
            app.world()
                .entity(e)
                .get_ref::<Transform>()
                .unwrap()
                .is_changed()
        );
    }

    /// `declare_ports` reports exactly what the entity has, so `ListPorts` and slot writes
    /// cannot disagree about which names exist.
    #[test]
    fn list_reports_only_the_components_present() {
        let mut app = app();
        let e = app.world_mut().spawn(Transform::default()).id();
        let reg = app.world().resource::<PortRegistry>().clone();
        let names: Vec<String> = reg
            .entity_port_owners(app.world(), e)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert!(names.contains(&"scale_y".to_string()));
        assert!(!names.contains(&"light_intensity".to_string()));
    }
}
