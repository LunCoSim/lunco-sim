//! Scene properties as scalar connection targets.
//!
//! USD `inputs:light_*`, `inputs:translation_*` and `inputs:scale_*`
//! connections project model results into native light and transform components
//! through the shared port registry. The backend declares inputs only; readable
//! input samples use the registry's explicit input-side access contract.
//!
//! Native component membership determines which ports exist. Cached slots and
//! live metadata share that owner, and the registry validates the complete input
//! transaction before any commit. Fixed units come from `EngineUnit`; bounds
//! and coordinate frames are derived from the current native components.
//! Commits validate the explicit f64-to-f32 scene boundary and preserve Bevy
//! change detection when the written value is bit-identical.
//!
//! Drive transform ports only on prims without another transform owner, such as
//! a rigid body or joint. A connected transform remains an authored consequence
//! of the source model, rather than a second motion controller.

use bevy::light::{PointLight, SpotLight};
use bevy::prelude::*;
use lunco_core::GlobalEntityId;
use lunco_port_core::EngineUnit;
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
    /// Resolve ownership from native component membership without sampling a value.
    fn has_target(self, world: &World, entity: Entity) -> bool {
        match self {
            Self::LightIntensity
            | Self::LightRadius
            | Self::LightColorR
            | Self::LightColorG
            | Self::LightColorB => {
                world.get::<PointLight>(entity).is_some()
                    || world.get::<SpotLight>(entity).is_some()
            }
            Self::TranslationX
            | Self::TranslationY
            | Self::TranslationZ
            | Self::ScaleX
            | Self::ScaleY
            | Self::ScaleZ => world.get::<Transform>(entity).is_some(),
        }
    }

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
    slot.has_target(world, entity).then_some(slot as u64)
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
        ScenePropertySlot::LightIntensity => (
            Some(EngineUnit::Lumen.reference()),
            Some(0.0),
            Some(float_max),
            None,
        ),
        ScenePropertySlot::LightRadius => (
            Some(EngineUnit::Meter.reference()),
            Some(0.0),
            Some(float_max),
            None,
        ),
        ScenePropertySlot::LightColorR
        | ScenePropertySlot::LightColorG
        | ScenePropertySlot::LightColorB => (
            Some(EngineUnit::Dimensionless.reference()),
            None,
            Some(float_max),
            None,
        ),
        ScenePropertySlot::TranslationX
        | ScenePropertySlot::TranslationY
        | ScenePropertySlot::TranslationZ => (
            Some(EngineUnit::Meter.reference()),
            Some(-float_max),
            Some(float_max),
            transform_translation_frame(world, entity),
        ),
        ScenePropertySlot::ScaleX | ScenePropertySlot::ScaleY | ScenePropertySlot::ScaleZ => (
            Some(EngineUnit::Dimensionless.reference()),
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

/// Input-only scene-property owner with component-gated slots and live contracts.
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
        if !out.accepts_direction(PortDirection::In) {
            return;
        }
        if let Some(name) = out.requested_name() {
            if let Some(slot) = resolve_input_slot(world, entity, name) {
                out.declare(name, PortDirection::In, Some(slot));
            }
            return;
        }
        // Listing exactly what the entity HAS is what keeps `ListPorts` and
        // the resolved slot writer telling the same story: every name reported here is one a
        // write would be accepted for, and no name is hidden because it currently
        // happens to hold a default.
        if world.get::<PointLight>(entity).is_some() || world.get::<SpotLight>(entity).is_some() {
            for name in LIGHT_PORTS {
                out.declare(
                    name,
                    PortDirection::In,
                    resolve_input_slot(world, entity, name),
                );
            }
        }
        if world.get::<Transform>(entity).is_some() {
            for name in TRANSFORM_PORTS {
                out.declare(
                    name,
                    PortDirection::In,
                    resolve_input_slot(world, entity, name),
                );
            }
        }
    },
    metadata: scene_property_metadata,
    read_output: |_, _, _| None,
    read_input,
    initial_expression: None,

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
fn mark_point_light_surface_ready(trigger: On<Add<PointLight>>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_port_core::PortSurfaceReady);
}

fn mark_spot_light_surface_ready(trigger: On<Add<SpotLight>>, mut commands: Commands) {
    commands
        .entity(trigger.entity)
        .try_insert(lunco_port_core::PortSurfaceReady);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measure contract construction without renderer or scene scheduling work.
    #[test]
    #[ignore = "manual scene-property metadata performance measurement"]
    fn scene_property_metadata_benchmark() {
        let mut world = World::new();
        let entity = world
            .spawn((
                GlobalEntityId::from_raw(1),
                Transform::default(),
                PointLight::default(),
            ))
            .id();
        let iterations = 131_072;
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            for name in LIGHT_PORTS.iter().chain(TRANSFORM_PORTS.iter()) {
                std::hint::black_box(scene_property_metadata(
                    std::hint::black_box(&world),
                    entity,
                    std::hint::black_box(name),
                    PortDirection::In,
                ));
            }
        }
        println!(
            "SCENE_PROPERTY_METADATA_BENCH elapsed_ns={} calls={}",
            started.elapsed().as_nanos(),
            iterations * (LIGHT_PORTS.len() + TRANSFORM_PORTS.len())
        );
    }

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
