//! Solar environment domain — the sun's direction as a co-simulation source.
//!
//! The lighting analog of the gravity bridge. Semantic [`SunState`] is the
//! provider contract; the render `DirectionalLight` is only its projection.
//! This module caches the semantic direction per-entity as [`LocalSolar`] and
//! publishes it into the co-sim graph as ordinary `SimComponent` **outputs**,
//! so a sun-tracking model receives it through a plain output→input wire — the
//! ontology's `RadiationProvider → LocalRadiation → solar models` pipeline.
//!
//! Values are published on explicit [`crate::EnvironmentProbe`] source prims.
//! Models consume them through ordinary USD connections, so provider and
//! consumer remain distinct graph nodes.
//!
//! ## Provider note
//!
//! There is no separate `SolarProvider` component yet: [`SunState`] is the
//! provider contract (its direction is published by ephemeris or an explicit
//! command). A richer provider (irradiance
//! model, eclipse occlusion, per-site horizon visibility) would attach here
//! later, exactly as `GravityProvider` carries the gravity model — the
//! [`LocalSolar`] cache already gives each entity its own slot for that.

use bevy::prelude::*;

use crate::Earthshine;
use lunco_cosim::{SUN_MOUNT_X_CONNECTOR, SUN_MOUNT_Y_CONNECTOR, SUN_MOUNT_Z_CONNECTOR};

/// Semantic sun state produced by the selected physical/provider model.
///
/// Render lights and co-simulation ports are projections of this resource;
/// neither is read back as a source of truth. `None` means the provider has not
/// produced a valid direction for the current scene/epoch.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct SunState {
    /// Unit direction from the observed site toward the sun, in the active
    /// site's ENU axes. Consumers must project it through the bound active
    /// frame before treating it as a world/render direction.
    pub direction_to_sun: Option<Vec3>,
    /// Optional calibrated direct-sun irradiance in lux.
    pub irradiance_lux: Option<f32>,
    /// Monotonic semantic revision for change-gated projections.
    pub revision: u64,
}

impl Default for SunState {
    fn default() -> Self {
        Self {
            direction_to_sun: None,
            irradiance_lux: None,
            revision: 0,
        }
    }
}

impl SunState {
    /// Return the canonical unit form of a non-zero finite direction.
    pub fn normalized_direction(direction: Vec3) -> Option<Vec3> {
        (direction.is_finite() && direction.length_squared() >= 1.0e-12)
            .then(|| direction.normalize())
    }

    /// Publish a new provider sample and advance the projection revision only
    /// when semantic values changed.
    pub fn publish(&mut self, direction_to_sun: Vec3, irradiance_lux: Option<f32>) -> bool {
        let Some(direction_to_sun) = Self::normalized_direction(direction_to_sun) else {
            return false;
        };
        if irradiance_lux.is_some_and(|lux| !lux.is_finite() || lux < 0.0) {
            return false;
        }
        if self.direction_to_sun != Some(direction_to_sun) || self.irradiance_lux != irradiance_lux
        {
            self.direction_to_sun = Some(direction_to_sun);
            self.irradiance_lux = irradiance_lux;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    /// Remove the current provider sample rather than retaining stale lighting.
    pub fn clear(&mut self) {
        if self.direction_to_sun.take().is_some() || self.irradiance_lux.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    /// Change only the calibrated irradiance while preserving the provider's
    /// direction. Runtime lighting commands use this instead of mutating a
    /// render light and asking the semantic provider to discover the change.
    pub fn set_irradiance(&mut self, irradiance_lux: Option<f32>) -> bool {
        if irradiance_lux.is_some_and(|lux| !lux.is_finite() || lux < 0.0) {
            return false;
        }
        if self.irradiance_lux != irradiance_lux {
            self.irradiance_lux = irradiance_lux;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }
}

/// Render-facing projection of the semantic sun direction.
///
/// This is produced once by the environment boundary from [`SunState`] and
/// the explicitly bound physics-frame orientation. Horizon baking and shader
/// wiring consume this resource; they never read a `DirectionalLight` pose
/// back as a provider value.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SunRenderState {
    /// Unit direction toward the Sun in the canonical render/world frame.
    pub direction_to_sun_world: Option<Vec3>,
    pub revision: u64,
}

impl Default for SunRenderState {
    fn default() -> Self {
        Self {
            direction_to_sun_world: None,
            revision: 0,
        }
    }
}

impl SunRenderState {
    fn publish(&mut self, direction_to_sun_world: Vec3) {
        if self.direction_to_sun_world != Some(direction_to_sun_world) {
            self.direction_to_sun_world = Some(direction_to_sun_world);
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(crate) fn clear(&mut self) {
        if self.direction_to_sun_world.take().is_some() {
            self.revision = self.revision.wrapping_add(1);
        }
    }
}

/// Unit direction toward the Sun in an entity's authored mount frame.
///
/// The lighting analog of `LocalGravity`. Today the value is global (one sun,
/// no occlusion) so every entity gets the same direction, but it is cached
/// per-entity so a future per-site horizon/eclipse model can vary it without
/// touching consumers.
///
/// The convention is explicit and shared with antenna tracking: `+X` right,
/// `+Y` up, `-Z` forward.  The full world→mount rotation is applied before a
/// model selects joint angles, so vehicle yaw, pitch and roll cannot be
/// mistaken for a solar bearing.
#[derive(Component, Debug, Clone, Copy, PartialEq, Reflect, Default)]
#[reflect(Component)]
pub struct LocalSolar {
    /// Complete active-world→mount direction, kept as a vector until a
    /// consumer needs its own coordinates.
    pub direction: Vec3,
}

/// Computes [`LocalSolar`] for every explicit environment probe from the scene sun.
///
/// Semantic [`SunState`] is the provider. Render-layer-scoped preview lights
/// and earthshine never participate in this source path. Writes `LocalSolar`
/// only when the direction actually changes, to avoid a per-frame
/// change-detection storm — mirrors `compute_local_gravity`.
///
/// Targets entities that carry [`crate::EnvironmentProbe`] so the cache lands
/// exactly where [`inject_local_solar_into_cosim`] will publish it.
pub fn compute_local_solar(
    mut commands: Commands,
    sun: Option<Res<SunState>>,
    active_frame: Option<Res<lunco_core::ActivePhysicsFrame>>,
    q_frames: Query<&GlobalTransform>,
    q_targets: Query<
        (Entity, Option<&LocalSolar>, Option<&GlobalTransform>),
        With<crate::EnvironmentProbe>,
    >,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if q_targets.is_empty() {
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        }
        return;
    }
    let Some(direction_to_sun) = sun
        .as_deref()
        .and_then(|state| state.direction_to_sun)
        .and_then(SunState::normalized_direction)
    else {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        }
        return;
    };

    let Some(active_frame) = active_frame else {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "LocalSolar".to_string(),
                    message: "a semantic SunState exists but no ActivePhysicsFrame is bound"
                        .to_string(),
                }],
            );
        }
        return;
    };
    let Some(frame_gt) = q_frames.get(active_frame.0).ok() else {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: format!("frame:{:?}", active_frame.0),
                    message: "the bound ActivePhysicsFrame has no live GlobalTransform".to_string(),
                }],
            );
        }
        return;
    };
    let direction_to_sun_world = frame_gt.rotation().mul_vec3(direction_to_sun);
    let Some(direction_to_sun_world) = SunState::normalized_direction(direction_to_sun_world)
    else {
        for (entity, existing, _) in &q_targets {
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
        }
        if let Some(mut diagnostics) = diagnostics {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "LocalSolar".to_string(),
                    message:
                        "the semantic SunState direction is invalid after active-frame projection"
                            .to_string(),
                }],
            );
        }
        return;
    };
    let mut missing_mounts = 0;
    for (entity, existing, mount) in &q_targets {
        let Some(mount) = mount else {
            missing_mounts += 1;
            if existing.is_some() {
                commands.entity(entity).remove::<LocalSolar>();
            }
            continue;
        };
        let next = LocalSolar {
            direction: crate::mount_frame::direction_in_mount_frame(direction_to_sun_world, mount),
        };
        if existing == Some(&next) {
            continue;
        }
        commands.entity(entity).try_insert(next);
    }
    if let Some(mut diagnostics) = diagnostics {
        if missing_mounts == 0 {
            diagnostics.replace_producer("environment-solar", std::iter::empty());
        } else {
            diagnostics.replace_producer(
                "environment-solar",
                [lunco_core::RuntimeDiagnostic {
                    code: "solar-mount".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-solar".to_string(),
                    subject: "EnvironmentProbe".to_string(),
                    message: format!(
                        "{missing_mounts} environment probe(s) have no GlobalTransform for solar projection"
                    ),
                }],
            );
        }
    }
}

/// Project semantic [`SunState`] into the unique unscoped render sun.
///
/// This is the only system that writes the render light's direction from
/// semantic sun state. Zero or multiple candidate lights is a contract error
/// for the render host; no arbitrary light is selected.
fn replace_sun_diagnostic(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    finding: Option<lunco_core::RuntimeDiagnostic>,
) {
    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        diagnostics.replace_producer(
            "environment-sun",
            finding
                .into_iter()
                .chain(std::iter::empty::<lunco_core::RuntimeDiagnostic>()),
        );
    }
}

pub fn project_sun_state_to_light(
    sun: Option<Res<SunState>>,
    mount: Option<Res<lunco_core::SceneMountState>>,
    active_frame: Option<Res<lunco_core::ActivePhysicsFrame>>,
    q_frames: Query<&GlobalTransform>,
    q_parents: Query<&GlobalTransform>,
    mut q_sun: Query<
        (
            &mut Transform,
            &mut bevy::light::DirectionalLight,
            Option<&ChildOf>,
        ),
        (
            Without<Earthshine>,
            Without<bevy::camera::visibility::RenderLayers>,
        ),
    >,
    mut render_state: ResMut<SunRenderState>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    let active_scene = mount
        .as_deref()
        .is_some_and(|mount| mount.active_root().is_some());
    let sun_count = q_sun.iter().count();
    if active_scene && sun_count != 1 {
        let message = if sun_count == 0 {
            "active scene has no unscoped scene sun; author exactly one UsdLux DistantLight"
                .to_string()
        } else {
            format!(
                "active scene has {sun_count} unscoped scene suns; author exactly one UsdLux DistantLight"
            )
        };
        replace_sun_diagnostic(
            &mut diagnostics,
            Some(lunco_core::RuntimeDiagnostic {
                code: "sun-contract".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "environment-sun".to_string(),
                subject: "scene-sun".to_string(),
                message,
            }),
        );
        render_state.clear();
        return;
    }
    let mut diagnostics = diagnostics;
    replace_sun_diagnostic(&mut diagnostics, None);
    let Some(direction_to_sun) = sun.as_deref().and_then(|state| state.direction_to_sun) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "active scene has no valid semantic SunState direction".to_string(),
                }),
            );
        }
        render_state.clear();
        return;
    };
    let Some(active_frame) = active_frame else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "physics-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "active scene has semantic sun state but no bound ActivePhysicsFrame"
                        .to_string(),
                }),
            );
        }
        render_state.clear();
        return;
    };
    let Ok(frame_gt) = q_frames.get(active_frame.0) else {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "physics-frame".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: format!(
                        "ActivePhysicsFrame {:?} is not a live entity with GlobalTransform",
                        active_frame.0
                    ),
                }),
            );
        }
        render_state.clear();
        return;
    };
    let direction_to_sun_world = frame_gt.rotation().mul_vec3(direction_to_sun);
    if !direction_to_sun_world.is_finite() || direction_to_sun_world.length_squared() < 1.0e-12 {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message:
                        "semantic SunState direction is non-finite or zero after frame projection"
                            .to_string(),
                }),
            );
        }
        render_state.clear();
        return;
    }
    if sun
        .as_deref()
        .and_then(|state| state.irradiance_lux)
        .is_some_and(|lux| !lux.is_finite() || lux < 0.0)
    {
        if active_scene {
            replace_sun_diagnostic(
                &mut diagnostics,
                Some(lunco_core::RuntimeDiagnostic {
                    code: "sun-state".to_string(),
                    severity: lunco_core::DiagnosticSeverity::Error,
                    producer: "environment-sun".to_string(),
                    subject: "semantic-sun".to_string(),
                    message: "semantic SunState irradiance is non-finite or negative".to_string(),
                }),
            );
        }
        render_state.clear();
        return;
    }
    let direction_to_sun_world = direction_to_sun_world.normalize();
    render_state.publish(direction_to_sun_world);
    let parent_rotation = match q_sun.single() {
        Ok((_, _, Some(parent))) => match q_parents.get(parent.parent()) {
            Ok(parent_transform) => Some(parent_transform.rotation()),
            Err(_) => {
                replace_sun_diagnostic(
                    &mut diagnostics,
                    Some(lunco_core::RuntimeDiagnostic {
                        code: "sun-parent".to_string(),
                        severity: lunco_core::DiagnosticSeverity::Error,
                        producer: "environment-sun".to_string(),
                        subject: "scene-sun".to_string(),
                        message: "scene sun has a parent without a live GlobalTransform"
                            .to_string(),
                    }),
                );
                render_state.clear();
                return;
            }
        },
        Ok((_, _, None)) => None,
        Err(_) => return,
    };
    let Ok((mut transform, mut light, _)) = q_sun.single_mut() else {
        return;
    };
    // A root light has no parent-local rotation: its Transform is already in
    // the world frame. A child light must use the live parent projection above.
    let emit_direction = match parent_rotation {
        Some(rotation) => rotation.inverse().mul_vec3(-direction_to_sun_world),
        None => -direction_to_sun_world,
    };
    let up = if emit_direction.dot(Vec3::Y).abs() > 0.99 {
        Vec3::X
    } else {
        Vec3::Y
    };
    if transform.forward().angle_between(emit_direction) > 2.0e-5 {
        transform.look_to(emit_direction, up);
    }
    if let Some(irradiance) = sun.as_deref().and_then(|state| state.irradiance_lux) {
        if (light.illuminance - irradiance).abs() > irradiance.abs().max(1.0) * 5.0e-3 {
            light.illuminance = irradiance;
        }
    }
}

/// Publishes each entity's [`LocalSolar`] as `SimComponent` **outputs**
/// [`SUN_MOUNT_X_CONNECTOR`] / [`SUN_MOUNT_Y_CONNECTOR`] /
/// [`SUN_MOUNT_Z_CONNECTOR`].
///
/// Runs after [`compute_local_solar`] and before cosim propagation, so the
/// fresh outputs are read the same tick. Writes every tick because a model's
/// own output sync may rewrite its outputs map (same reasoning as the gravity
/// bridge). If no scene sun is available, removes only the solar outputs while
/// retaining the schema-declared source contract for later binding.
pub fn inject_local_solar_into_cosim(
    mut q: Query<
        (Option<&LocalSolar>, &mut lunco_cosim::SimComponent),
        With<crate::EnvironmentProbe>,
    >,
) {
    for (solar, mut comp) in &mut q {
        let Some(solar) = solar else {
            comp.outputs.remove(SUN_MOUNT_X_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Y_CONNECTOR);
            comp.outputs.remove(SUN_MOUNT_Z_CONNECTOR);
            continue;
        };
        comp.outputs
            .insert(SUN_MOUNT_X_CONNECTOR.to_string(), solar.direction.x as f64);
        comp.outputs
            .insert(SUN_MOUNT_Y_CONNECTOR.to_string(), solar.direction.y as f64);
        comp.outputs
            .insert(SUN_MOUNT_Z_CONNECTOR.to_string(), solar.direction.z as f64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sun_removes_cached_local_direction() {
        let mut app = App::new();
        app.add_systems(Update, compute_local_solar);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                LocalSolar {
                    direction: Vec3::NEG_Z,
                },
            ))
            .id();
        app.update();
        assert!(
            app.world().get::<LocalSolar>(probe).is_none(),
            "a scene without a sun must not retain a stale solar direction"
        );
    }

    #[test]
    fn active_scene_without_a_sun_is_a_persistent_diagnostic() {
        let mut app = App::new();
        let root = app.world_mut().spawn_empty().id();
        let mut mount = lunco_core::SceneMountState::default();
        mount.register_root(root, true);
        app.insert_resource(mount);
        app.init_resource::<SunState>();
        app.init_resource::<SunRenderState>();
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.add_systems(Update, project_sun_state_to_light);

        app.update();

        let diagnostics = app.world().resource::<lunco_core::RuntimeDiagnostics>();
        assert_eq!(diagnostics.findings.len(), 1);
        assert_eq!(diagnostics.findings[0].code, "sun-contract");
        assert_eq!(
            diagnostics.findings[0].severity,
            lunco_core::DiagnosticSeverity::Error
        );
    }

    #[test]
    fn missing_solar_direction_removes_only_solar_outputs() {
        let mut app = App::new();
        let mut sim = lunco_cosim::SimComponent::default();
        sim.outputs.insert(SUN_MOUNT_X_CONNECTOR.to_owned(), 1.0);
        sim.outputs.insert(SUN_MOUNT_Y_CONNECTOR.to_owned(), 2.0);
        sim.outputs.insert(SUN_MOUNT_Z_CONNECTOR.to_owned(), 3.0);
        sim.outputs
            .insert(lunco_cosim::GRAVITY_SOURCE_CONNECTOR.to_owned(), 9.81);
        let entity = app.world_mut().spawn((crate::EnvironmentProbe, sim)).id();
        app.add_systems(Update, inject_local_solar_into_cosim);

        app.update();

        let outputs = &app
            .world()
            .get::<lunco_cosim::SimComponent>(entity)
            .unwrap()
            .outputs;
        assert!(!outputs.contains_key(SUN_MOUNT_X_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Y_CONNECTOR));
        assert!(!outputs.contains_key(SUN_MOUNT_Z_CONNECTOR));
        assert_eq!(
            outputs.get(lunco_cosim::GRAVITY_SOURCE_CONNECTOR),
            Some(&9.81)
        );
    }

    #[test]
    fn rotated_site_frame_is_projected_before_mount_conversion() {
        let mut app = App::new();
        let site_rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let frame = app
            .world_mut()
            .spawn(GlobalTransform::from(Transform::from_rotation(
                site_rotation,
            )))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        app.add_systems(Update, compute_local_solar);

        let mount_rotation = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        let probe = app
            .world_mut()
            .spawn((
                crate::EnvironmentProbe,
                GlobalTransform::from(Transform::from_rotation(mount_rotation)),
            ))
            .id();
        app.update();

        let expected = mount_rotation.inverse() * (site_rotation * Vec3::NEG_Z);
        let got = app
            .world()
            .get::<LocalSolar>(probe)
            .expect("projected solar direction");
        assert!(
            got.direction.abs_diff_eq(expected.normalize(), 1e-5),
            "site ENU must become active-world before mount conversion: got {:?}, expected {:?}",
            got.direction,
            expected
        );
    }

    #[test]
    fn a_probe_without_mount_transform_reports_an_error() {
        let mut app = App::new();
        let frame = app.world_mut().spawn(GlobalTransform::IDENTITY).id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(frame));
        app.insert_resource(SunState {
            direction_to_sun: Some(Vec3::NEG_Z),
            ..Default::default()
        });
        app.init_resource::<lunco_core::RuntimeDiagnostics>();
        app.add_systems(Update, compute_local_solar);
        let probe = app
            .world_mut()
            .spawn((crate::EnvironmentProbe, LocalSolar { direction: Vec3::X }))
            .id();

        app.update();

        assert!(app.world().get::<LocalSolar>(probe).is_none());
        assert!(app
            .world()
            .resource::<lunco_core::RuntimeDiagnostics>()
            .findings
            .iter()
            .any(|finding| finding.code == "solar-mount"));
    }
}
