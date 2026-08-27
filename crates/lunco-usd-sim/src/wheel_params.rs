//! # Unified wheel parameter model
//!
//! ONE reader for BOTH wheel kinds. A wheel prim's full dynamics — tire
//! (μ, slip stiffness), inertia and
//! optional suspension compliance — are read here into a single [`WheelParams`],
//! regardless of whether the wheel is realised as a raycast wheel
//! (`lunco_mobility::WheelRaycast`, analytical suspension + contact) or a
//! physical wheel (Avian `RevoluteJoint` + normal-contact solver). Both
//! realizations call the same tire/drivetrain laws; only suspension and normal
//! contact acquisition differ. Every number they act on comes from the same
//! composed attributes with the same strictness.
//!
//! ## Attribute provenance
//!
//! PhysX-compatible names are used where NVIDIA's vehicle schema models the
//! concept; the runtime owns the realization-specific integration law — see
//! `core/physxSchema.usda`. `lunco:` names cover LunCo-only concepts:
//!
//! | Param | Attribute | Required |
//! |---|---|---|
//! | radius | `physxVehicleWheel:radius` | yes |
//! | width | `physxVehicleWheel:width` | yes |
//! | mass | `physxVehicleWheel:mass` | yes |
//! | moment of inertia | `physxVehicleWheel:moi` | yes (0 ⇒ derived ½·m·r² from authored mass+radius) |
//! | drive torque and shaft speed | authored Modelica mechanical network | via motor/gearbox |
//! | bearing damping | `physxVehicleWheel:dampingRate` | yes |
//! | brake torque | `physxVehicleWheel:maxBrakeTorque` | yes |
//! | slip stiffness (longitudinal) | `physxVehicleTire:longitudinalStiffness` | yes |
//! | lateral stiffness graph | `physxVehicleTire:lateralStiffnessGraph` + `restLoad` | yes |
//! | Coulomb μ | `physics:dynamicFriction` (`UsdPhysicsMaterialAPI`) | yes |
//! | steer axis | `lunco:wheel:steerAxis` | yes |
//! | suspension | `lunco:suspension:restLength` + `physxVehicleSuspension:springStrength`/`:springDamperRate` | raycast only |
//!
//! The mechanical network is the one source of drive torque and shaft speed for
//! both realizations. The Rust projections only bind that solved boundary to
//! Avian or the raycast tire; they do not re-derive a motor curve.
//!
//! ## Strictness
//!
//! NO Rust fallback values. Every required attribute missing from the composed
//! prim is an asset error, collected so one bad wheel reports ALL of them, not
//! just the first. The authored defaults live in
//! `components/mobility/wheel.usda`, which every wheel composes — one authored
//! set is what makes "same defaults for both variants" true.

use avian3d::prelude::{Collider, ColliderDensity, Friction, Position, RevoluteJoint, Rotation};
use bevy::asset::AssetId;
use bevy::log::{error, info};
use bevy::math::DVec3;
use bevy::prelude::{Entity, Quat, World};
use lunco_hardware::SteeringActuator;
use lunco_mobility::{JointedWheelTire, Suspension, TireLateralStiffnessGraph, WheelRaycast};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
use std::collections::{HashMap, HashSet};

/// Authored suspension compliance, shared by both wheel implementations. The
/// raycast wheel emulates this spring analytically; a joint wheel is a rigid
/// axle and does not need it.
///
/// `spring_k` / `damping_c` come from NVIDIA's canonical
/// `PhysxVehicleSuspensionAPI` names (`physxVehicleSuspension:springStrength` /
/// `:springDamperRate`). `rest_length` has no PhysX equivalent — PhysX models
/// travel as `travelDistance` + `sprungMass` — so it is authored as
/// `lunco:suspension:restLength`.
#[derive(Clone, Copy, Debug)]
pub struct SuspensionParams {
    /// Natural standoff of the wheel below its mount (raycast resting length), m.
    pub rest_length: f64,
    /// Spring stiffness, N/m.
    pub spring_k: f64,
    /// Spring damping, N·s/m.
    pub damping_c: f64,
}

/// One standard PhysX wheel-attachment binding resolved from a composed stage.
/// The attachment owns the index; the wheel, suspension, and tire may be the
/// attachment itself (the direct API form) or separate relationship targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WheelAttachmentBinding {
    pub suspension: String,
    pub tire: String,
    pub index: i32,
}

/// Canonical wheel-attachment topology shared by the live projector and
/// `ValidateAsset`. Keeping resolution here prevents the validator from
/// inventing a second relationship or direct-API interpretation.
#[derive(Clone, Debug, Default)]
pub struct WheelAttachmentTopology {
    bindings: HashMap<String, WheelAttachmentBinding>,
    invalid_wheels: HashSet<String>,
}

impl WheelAttachmentTopology {
    /// Return the one resolved binding for a composed wheel, if valid.
    pub fn binding_for(&self, wheel_path: &str) -> Option<&WheelAttachmentBinding> {
        if self.is_invalid(wheel_path) {
            None
        } else {
            self.bindings.get(wheel_path)
        }
    }

    /// Return whether the stage authored an invalid or ambiguous attachment
    /// for this wheel.
    pub fn is_invalid(&self, wheel_path: &str) -> bool {
        self.invalid_wheels.contains(wheel_path)
    }

    pub(crate) fn bindings(&self) -> impl Iterator<Item = (&String, &WheelAttachmentBinding)> {
        self.bindings.iter()
    }

    pub(crate) fn invalid_wheels(&self) -> impl Iterator<Item = &String> {
        self.invalid_wheels.iter()
    }
}

/// Resolve every standard wheel attachment in a composed stage.
///
/// A relationship is singular in the vehicle schema. Multi-target authoring,
/// duplicate attachments, malformed endpoint types, and missing required
/// endpoints are rejected; the first or last list target is never selected.
pub fn collect_wheel_attachment_topology(
    reader: &lunco_usd_bevy::StageView<'_>,
) -> WheelAttachmentTopology {
    let mut topology = WheelAttachmentTopology::default();
    for attachment in reader.prim_paths() {
        if !reader.has_api_schema(&attachment, "PhysxVehicleWheelAttachmentAPI") {
            continue;
        }

        let endpoint = |relationship: &str, api: &str| {
            attachment_endpoint(reader, &attachment, relationship, api)
        };
        let wheel = match endpoint("physxVehicleWheelAttachment:wheel", "PhysxVehicleWheelAPI") {
            Ok(Some(target)) => target,
            Ok(None) => {
                error!(
                    "USD wheel attachment {} has neither a wheel relationship nor a direct PhysxVehicleWheelAPI",
                    attachment
                );
                topology
                    .invalid_wheels
                    .insert(attachment.as_str().to_owned());
                continue;
            }
            Err(reason) => {
                error!("USD wheel attachment {} is invalid: {}", attachment, reason);
                topology
                    .invalid_wheels
                    .insert(attachment.as_str().to_owned());
                continue;
            }
        };

        let suspension = match endpoint(
            "physxVehicleWheelAttachment:suspension",
            "PhysxVehicleSuspensionAPI",
        ) {
            Ok(Some(target)) => target,
            Ok(None) => {
                error!(
                    "USD wheel attachment {} for {} has neither a suspension relationship nor a direct PhysxVehicleSuspensionAPI",
                    attachment, wheel
                );
                topology.invalid_wheels.insert(wheel.clone());
                continue;
            }
            Err(reason) => {
                error!("USD wheel attachment {} is invalid: {}", attachment, reason);
                topology.invalid_wheels.insert(wheel.clone());
                continue;
            }
        };

        let tire = match endpoint("physxVehicleWheelAttachment:tire", "PhysxVehicleTireAPI") {
            Ok(Some(target)) => target,
            Ok(None) => {
                error!(
                    "USD wheel attachment {} for {} has neither a tire relationship nor a direct PhysxVehicleTireAPI",
                    attachment, wheel
                );
                topology.invalid_wheels.insert(wheel.clone());
                continue;
            }
            Err(reason) => {
                error!("USD wheel attachment {} is invalid: {}", attachment, reason);
                topology.invalid_wheels.insert(wheel.clone());
                continue;
            }
        };

        let Some(index) = reader.scalar::<i32>(&attachment, "physxVehicleWheelAttachment:index")
        else {
            error!(
                "USD wheel attachment {} has no valid physxVehicleWheelAttachment:index",
                attachment
            );
            topology.invalid_wheels.insert(wheel.clone());
            continue;
        };

        if topology.bindings.contains_key(&wheel) {
            error!(
                "USD wheel {} is targeted by more than one PhysxVehicleWheelAttachmentAPI; one attachment is required",
                wheel
            );
            topology.bindings.remove(&wheel);
            topology.invalid_wheels.insert(wheel);
            continue;
        }

        topology.bindings.insert(
            wheel,
            WheelAttachmentBinding {
                suspension,
                tire,
                index,
            },
        );
    }
    topology
}

/// Resolve a composed relationship only when it has exactly one target. The
/// standard wheel attachment relationships are singular links; accepting the
/// first element of a multi-target list would make topology depend on list-op
/// ordering rather than authored intent.
fn one_attachment_target(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    name: &str,
) -> Result<Option<String>, usize> {
    let targets = reader.rel_targets(prim, name);
    match targets.as_slice() {
        [] => Ok(None),
        [target] => Ok(Some(target.as_str().to_owned())),
        _ => Err(targets.len()),
    }
}

/// Resolve one standard attachment endpoint. A relationship target is
/// authoritative when present; the standard schema's direct-API form uses the
/// attachment prim itself when the relationship is omitted. In both cases the
/// resolved prim must carry the API that defines the endpoint.
fn attachment_endpoint(
    reader: &lunco_usd_bevy::StageView<'_>,
    attachment: &SdfPath,
    relationship: &str,
    api_schema: &str,
) -> Result<Option<String>, String> {
    let target = one_attachment_target(reader, attachment, relationship).map_err(|count| {
        format!(
            "{} has {} relationship targets; exactly one is required",
            relationship, count
        )
    })?;
    let target = target.or_else(|| {
        reader
            .has_api_schema(attachment, api_schema)
            .then(|| attachment.as_str().to_owned())
    });
    let Some(target) = target else {
        return Ok(None);
    };
    let target_path = SdfPath::new(&target)
        .map_err(|_| format!("{} resolves to invalid target {}", relationship, target))?;
    if !reader.has_api_schema(&target_path, api_schema) {
        return Err(format!(
            "{} targets {} without {}",
            relationship, target, api_schema
        ));
    }
    Ok(Some(target))
}

/// The complete authored dynamics of one wheel — the single source both
/// `setup_raycast_wheel` and `setup_physical_wheel` consume, and the single
/// struct the live resync path re-derives.
#[derive(Clone, Copy, Debug)]
pub struct WheelParams {
    /// Wheel radius, m (`physxVehicleWheel:radius`).
    pub radius: f64,
    /// Wheel width along its authored cylinder axis, m (`physxVehicleWheel:width`).
    /// This standard wheel value drives the collider in both realizations.
    pub width: f64,
    /// Authored cylinder/axle axis (`axis` token).  Avian's primitive cylinder
    /// uses local +Y, so the physical projection rotates that primitive onto this
    /// authored axis and the revolute joint uses the same vector.
    pub axle_axis: DVec3,
    /// Wheel mass, kg (`physxVehicleWheel:mass`). The same authored value feeds both
    /// realizations; any difference in feel must come from the solver, not a Rust fork.
    pub mass: f64,
    /// Explicit axle moment of inertia, kg·m² (`physxVehicleWheel:moi`).
    /// An authored zero means the documented solid-cylinder derivation
    /// `½·m·r²` from the authored mass and radius; the attribute itself is still
    /// required by the standard PhysX wheel schema.
    pub moment_of_inertia: f64,
    /// Bearing + rolling drag, N·m·s (`physxVehicleWheel:dampingRate`). A
    /// physical property of the hub in its own right — REQUIRED, never inferred
    /// from the drive torque.
    pub bearing_damping: f64,
    /// Lock-up authority, N·m (`physxVehicleWheel:maxBrakeTorque`).
    pub brake_torque_max: f64,
    /// Tire longitudinal stiffness (`physxVehicleTire:longitudinalStiffness`).
    pub slip_stiffness: f64,
    /// Standard PhysX load-dependent lateral stiffness graph and reference load.
    pub lateral_stiffness_graph: TireLateralStiffnessGraph,
    /// Lower edge of the tire's measured steady-state cornering-speed envelope.
    pub min_validated_speed: f64,
    /// Coulomb μ from the wheel's standard `UsdPhysicsMaterialAPI`, composed
    /// through the `tire` variant. Both realizations use it as the shared tire
    /// cone; Avian's generic tangent friction is disabled for jointed tire
    /// contacts so it cannot double-count this force.
    pub friction_mu: f64,
    /// Raked steering-head axis, wheel-local (`lunco:wheel:steerAxis`).
    pub steer_axis: DVec3,
    /// Suspension compliance; `None` ⇒ none resolves. A raycast wheel treats
    /// that as a hard asset error, a joint wheel does not need it.
    pub suspension: Option<SuspensionParams>,
}

impl WheelParams {
    /// Read wheel, tire, and suspension attributes from their standard composed
    /// prims, collecting ALL missing required names into the error.
    /// `attachment_suspension` and `attachment_tire` are selected by the
    /// standard `PhysxVehicleWheelAttachmentAPI`; direct API composition passes
    /// the wheel itself for each. A wheel without an attachment is
    /// under-authored and is rejected.
    ///
    pub fn read(
        reader: &lunco_usd_bevy::StageView<'_>,
        wheel: &SdfPath,
        attachment_suspension: Option<&SdfPath>,
        attachment_tire: Option<&SdfPath>,
    ) -> Result<WheelParams, Vec<String>> {
        let mut missing = Vec::new();
        let tire = attachment_tire.unwrap_or(wheel);
        if attachment_tire.is_none() || !reader.has_api_schema(tire, "PhysxVehicleTireAPI") {
            missing.push("PhysxVehicleTireAPI".to_owned());
        }
        let axle_axis = match reader.text(wheel, "axis").as_deref() {
            Some("X") => DVec3::X,
            Some("Y") => DVec3::Y,
            Some("Z") => DVec3::Z,
            _ => {
                missing.push("axis".to_owned());
                DVec3::X
            }
        };
        let radius = read_required_real(reader, wheel, "physxVehicleWheel:radius", &mut missing);
        let width = read_required_real(reader, wheel, "physxVehicleWheel:width", &mut missing);
        let mass = read_required_real(reader, wheel, "physxVehicleWheel:mass", &mut missing);
        let bearing_damping =
            read_required_real(reader, wheel, "physxVehicleWheel:dampingRate", &mut missing);
        let brake_torque_max = read_required_real(
            reader,
            wheel,
            "physxVehicleWheel:maxBrakeTorque",
            &mut missing,
        );
        let slip_stiffness = read_required_real(
            reader,
            tire,
            "physxVehicleTire:longitudinalStiffness",
            &mut missing,
        );
        let mut lateral_stiffness_graph =
            match reader.scalar::<[f32; 2]>(tire, "physxVehicleTire:lateralStiffnessGraph") {
                Some([minimum_normalized_load, max_stiffness]) => TireLateralStiffnessGraph {
                    minimum_normalized_load: minimum_normalized_load as f64,
                    max_stiffness: max_stiffness as f64,
                    rest_load: 0.0,
                },
                None => {
                    missing.push("physxVehicleTire:lateralStiffnessGraph".to_owned());
                    TireLateralStiffnessGraph::default()
                }
            };
        let rest_load = read_required_real(reader, tire, "physxVehicleTire:restLoad", &mut missing);
        lateral_stiffness_graph.rest_load = rest_load;
        let min_validated_speed =
            read_optional_real(reader, tire, "lunco:tire:minValidatedSpeed", &mut missing);
        let friction_mu = read_required_real(reader, tire, "physics:dynamicFriction", &mut missing);
        // A zero is an authored, documented solid-cylinder derivation. The
        // standard PhysX wheel attribute itself remains required, so an
        // omitted value is reported with the other missing contract fields.
        let moment_of_inertia =
            read_required_real(reader, wheel, "physxVehicleWheel:moi", &mut missing);

        let steer_axis = match lunco_usd_bevy::read_vec3_f64(reader, wheel, "lunco:wheel:steerAxis")
        {
            Some(v) => DVec3::new(v[0], v[1], v[2]),
            None => {
                missing.push("lunco:wheel:steerAxis".to_owned());
                DVec3::Y
            }
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        validate_wheel_values(
            &mut missing,
            radius,
            width,
            mass,
            moment_of_inertia,
            bearing_damping,
            brake_torque_max,
            slip_stiffness,
            lateral_stiffness_graph,
            rest_load,
            min_validated_speed,
            friction_mu,
            steer_axis,
        );
        validate_wheel_schema_hints(
            reader,
            wheel,
            &mut missing,
            [
                ("physxVehicleWheel:radius", radius),
                ("physxVehicleWheel:moi", moment_of_inertia),
                ("physxVehicleWheel:dampingRate", bearing_damping),
                ("physxVehicleWheel:maxBrakeTorque", brake_torque_max),
            ],
        );
        validate_wheel_schema_hints(
            reader,
            tire,
            &mut missing,
            [
                ("physxVehicleTire:longitudinalStiffness", slip_stiffness),
                ("physxVehicleTire:restLoad", rest_load),
                ("lunco:tire:minValidatedSpeed", min_validated_speed),
            ],
        );
        if !missing.is_empty() {
            return Err(missing);
        }

        // The shipped compact composition applies the attachment, wheel, and
        // suspension APIs to one composed prim. That is an explicit standard
        // direct-composition form. Relationship-form
        // assets pass the referenced suspension from the stage topology map.
        let direct_suspension = (attachment_suspension.is_none()
            && reader.has_api_schema(wheel, "PhysxVehicleWheelAttachmentAPI")
            && reader.has_api_schema(wheel, "PhysxVehicleSuspensionAPI"))
        .then_some(wheel);
        let suspension_prim = attachment_suspension.or(direct_suspension);
        let suspension = match suspension_prim {
            Some(susp) => match read_suspension_attrs(reader, susp) {
                Ok(params) => Some(params),
                Err(errors) => {
                    missing.extend(errors);
                    None
                }
            },
            None => {
                missing.push("PhysxVehicleWheelAttachmentAPI".to_owned());
                None
            }
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        Ok(WheelParams {
            radius,
            width,
            axle_axis,
            mass,
            moment_of_inertia,
            bearing_damping,
            brake_torque_max,
            slip_stiffness,
            lateral_stiffness_graph,
            min_validated_speed,
            friction_mu,
            steer_axis,
            suspension,
        })
    }

    /// The raycast realisation: a `WheelRaycast` carrying these numbers.
    pub fn to_wheel_raycast(
        &self,
        drive_port: Entity,
        speed_port: Entity,
        steer_port: Entity,
        visual_entity: Option<Entity>,
    ) -> WheelRaycast {
        let mut wheel = WheelRaycast {
            wheel_radius: self.radius,
            visual_entity,
            drive_port,
            speed_port,
            steer_port,
            ..Default::default()
        };
        self.apply_to_raycast(&mut wheel);
        wheel
    }

    /// Write the tunable numbers into an existing `WheelRaycast` — the same
    /// mapping `to_wheel_raycast` uses, exposed so the live resync path can
    /// re-derive a spawned wheel in place (ports/visual/state untouched).
    pub fn apply_to_raycast(&self, wheel: &mut WheelRaycast) {
        wheel.wheel_radius = self.radius;
        wheel.mass = self.mass;
        wheel.moment_of_inertia = self.moment_of_inertia;
        wheel.bearing_damping = self.bearing_damping;
        wheel.friction_mu = self.friction_mu;
        wheel.slip_stiffness = self.slip_stiffness;
        wheel.lateral_stiffness_graph = self.lateral_stiffness_graph;
        wheel.min_validated_speed = self.min_validated_speed;
        wheel.brake_torque_max = self.brake_torque_max;
        wheel.steer_axis = self.steer_axis;
    }

    /// Write the suspension compliance into an existing `Suspension`.
    /// Returns `false` (untouched) when this wheel resolves no suspension.
    pub fn apply_to_suspension(&self, suspension: &mut Suspension) -> bool {
        let Some(susp) = self.suspension else {
            return false;
        };
        suspension.rest_length = susp.rest_length;
        suspension.spring_k = susp.spring_k;
        suspension.damping_c = susp.damping_c;
        true
    }

    /// Complete axle moment of inertia, kg·m² — authored
    /// `physxVehicleWheel:moi` when stated, otherwise the solid-disk derivation
    /// `½·m·r²` from the authored mass and radius. A composed wheel assembly
    /// authors its tire plus attached drivetrain inertia here because Avian owns
    /// the wheel's rotational state at the co-simulation boundary.
    ///
    /// The same authored value applies on the raycast side. The physical wheel's
    /// collider density still derives its mass independently; it must not replace
    /// an authored assembly inertia with a collider-only estimate.
    pub fn axle_inertia(&self) -> f64 {
        let tire = if self.moment_of_inertia > 0.0 {
            self.moment_of_inertia
        } else {
            0.5 * self.mass * self.radius * self.radius
        };
        tire
    }

    /// Collider density realising `physxVehicleWheel:mass` on the physical wheel's
    /// cylinder collider (`cylinder(r, h = physxVehicleWheel:width)` ⇒ volume
    /// = π·r²·width).
    ///
    /// Mass goes in via DENSITY, not a forced `Mass`: avian derives
    /// `AngularInertia` from the collider at `ColliderDensity` even when `Mass`
    /// is set, and a forced mass desyncs mass from angular inertia — the
    /// contact+joint solver then can't build enough support impulse and the
    /// rover sinks through the one-sided terrain heightfield.
    pub fn wheel_density(&self) -> f32 {
        let volume = std::f64::consts::PI * self.radius.powi(2) * self.width;
        (self.mass / volume) as f32
    }
}

/// Resolve a wheel's attachment suspension prim via the standard attachment
/// topology. The map belongs to one composed stage, so its keys are stage-local
/// paths; independent instances retain independent topology maps.
pub(crate) fn attachment_suspension_path(
    wheel_path: &str,
    wheel_attachment_targets: &HashMap<String, String>,
) -> Option<SdfPath> {
    wheel_attachment_targets
        .get(wheel_path)
        .and_then(|s| SdfPath::new(s).ok())
}

/// Resolve a wheel's attachment tire prim via the standard attachment
/// topology. The map belongs to one composed stage, so its keys are stage-local
/// paths; independent instances retain independent topology maps.
pub(crate) fn attachment_tire_path(
    wheel_path: &str,
    wheel_attachment_tires: &HashMap<String, String>,
) -> Option<SdfPath> {
    wheel_attachment_tires
        .get(wheel_path)
        .and_then(|s| SdfPath::new(s).ok())
}

/// Read the three suspension attrs off one prim. `None` unless all three are
/// authored — partial authoring is treated as missing (no per-field defaults).
fn read_suspension_attrs(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
) -> Result<SuspensionParams, Vec<String>> {
    let mut missing = Vec::new();
    let read = |name: &str, missing: &mut Vec<String>| {
        reader.real(prim, name).or_else(|| {
            missing.push(name.to_owned());
            None
        })
    };
    let rest_length = read("lunco:suspension:restLength", &mut missing);
    let spring_k = read("physxVehicleSuspension:springStrength", &mut missing);
    let damping_c = read("physxVehicleSuspension:springDamperRate", &mut missing);
    if !missing.is_empty() {
        return Err(missing);
    }

    let (Some(rest_length), Some(spring_k), Some(damping_c)) = (rest_length, spring_k, damping_c)
    else {
        unreachable!("missing suspension values were rejected above")
    };
    let mut invalid = Vec::new();
    validate_suspension_values(&mut invalid, rest_length, spring_k, damping_c);
    validate_suspension_schema_hints(
        reader,
        prim,
        &mut invalid,
        [
            ("lunco:suspension:restLength", rest_length),
            ("physxVehicleSuspension:springStrength", spring_k),
            ("physxVehicleSuspension:springDamperRate", damping_c),
        ],
    );
    if !invalid.is_empty() {
        return Err(invalid);
    }

    Ok(SuspensionParams {
        rest_length,
        spring_k,
        damping_c,
    })
}

fn read_optional_real(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    name: &str,
    missing: &mut Vec<String>,
) -> f64 {
    match reader.real(prim, name) {
        Some(value) => value,
        None if reader.has_authored_attribute(prim, name) => {
            missing.push(name.to_owned());
            0.0
        }
        None => 0.0,
    }
}

fn read_required_real(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    name: &str,
    missing: &mut Vec<String>,
) -> f64 {
    match reader.real(prim, name) {
        Some(value) => value,
        None => {
            missing.push(name.to_owned());
            0.0
        }
    }
}

fn validate_suspension_values(
    errors: &mut Vec<String>,
    rest_length: f64,
    spring_k: f64,
    damping_c: f64,
) {
    validate_nonnegative(errors, "lunco:suspension:restLength", rest_length);
    validate_nonnegative(errors, "physxVehicleSuspension:springStrength", spring_k);
    validate_nonnegative(errors, "physxVehicleSuspension:springDamperRate", damping_c);
}

fn validate_wheel_values(
    errors: &mut Vec<String>,
    radius: f64,
    width: f64,
    mass: f64,
    moment_of_inertia: f64,
    bearing_damping: f64,
    brake_torque_max: f64,
    slip_stiffness: f64,
    lateral_stiffness_graph: TireLateralStiffnessGraph,
    rest_load: f64,
    min_validated_speed: f64,
    friction_mu: f64,
    steer_axis: DVec3,
) {
    validate_positive(errors, "physxVehicleWheel:radius", radius);
    validate_positive(errors, "physxVehicleWheel:width", width);
    validate_positive(errors, "physxVehicleWheel:mass", mass);
    validate_nonnegative(errors, "physxVehicleWheel:moi", moment_of_inertia);
    validate_nonnegative(errors, "physxVehicleWheel:dampingRate", bearing_damping);
    validate_nonnegative(errors, "physxVehicleWheel:maxBrakeTorque", brake_torque_max);
    validate_nonnegative(
        errors,
        "physxVehicleTire:longitudinalStiffness",
        slip_stiffness,
    );
    validate_nonnegative(
        errors,
        "physxVehicleTire:lateralStiffnessGraph:minNormalizedLoad",
        lateral_stiffness_graph.minimum_normalized_load,
    );
    validate_positive(
        errors,
        "physxVehicleTire:lateralStiffnessGraph:maxStiffness",
        lateral_stiffness_graph.max_stiffness,
    );
    validate_positive(errors, "physxVehicleTire:restLoad", rest_load);
    validate_nonnegative(errors, "lunco:tire:minValidatedSpeed", min_validated_speed);
    validate_nonnegative(errors, "physics:dynamicFriction", friction_mu);
    if !(steer_axis.is_finite() && steer_axis.length_squared() > 0.0) {
        errors.push(format!(
            "lunco:wheel:steerAxis must be finite and non-zero, got {steer_axis:?}"
        ));
    }
}

fn validate_wheel_schema_hints(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    errors: &mut Vec<String>,
    values: impl IntoIterator<Item = (&'static str, f64)>,
) {
    for (name, value) in values {
        validate_schema_hint(reader, prim, name, value, errors);
    }
}

fn validate_suspension_schema_hints(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    errors: &mut Vec<String>,
    values: [(&str, f64); 3],
) {
    for (name, value) in values {
        validate_schema_hint(reader, prim, name, value, errors);
    }
}

fn validate_schema_hint(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
    name: &str,
    value: f64,
    errors: &mut Vec<String>,
) {
    let Some(hint) = reader.attr_ui_hint(prim, name) else {
        return;
    };
    if let Some(min) = hint.min {
        if value < min {
            errors.push(format!(
                "{name} must be >= schema minimum {min}, got {value}"
            ));
        }
    }
    if let Some(max) = hint.max {
        if value > max {
            errors.push(format!(
                "{name} must be <= schema maximum {max}, got {value}"
            ));
        }
    }
}

fn validate_positive(errors: &mut Vec<String>, name: &str, value: f64) {
    if !(value.is_finite() && value > 0.0) {
        errors.push(format!("{name} must be finite and > 0, got {value}"));
    }
}

fn validate_nonnegative(errors: &mut Vec<String>, name: &str, value: f64) {
    if !(value.is_finite() && value >= 0.0) {
        errors.push(format!("{name} must be finite and >= 0, got {value}"));
    }
}

// ---------------------------------------------------------------------------
// Live resync — the USD-based update path for spawned wheels.
//
// Wheel params are a PROJECTION of the document: the only writer is the USD
// document itself (`ApplyUsdOp SetAttribute` → registry → the change funnels in
// `twin_projection`/`live_consume`), and this module is how the projection
// catches up — by RE-READING the composed stage, never by accepting values from
// a side channel. Both funnels call [`resync_wheels_for_stage`] for edits that
// [`claims_edit`] recognises, INSTEAD of their generic
// `refresh_prim_subtree`/`reinstantiate_entity` path. That path is
// actively destructive for wheels: it despawns the wheel's synthesized
// `Port` children and visual child while `UsdSimProcessed` survives, so
// the sim params are never re-derived, the solved joint boundary points at a dead
// port, and the chassis-owned joint dangles. The resync mutates the spawned
// components in place — entity ids, joints, `JointCollisionDisabled`, ports and
// `UsdSimProcessed` are never touched.
// ---------------------------------------------------------------------------

/// Attribute families [`resync_wheels_for_stage`] claims from the generic
/// refresh path. Prim-scoped where a name is not wheel-specific:
/// `physxVehicleWheel:mass` is claimed only on a wheel prim — on a chassis it must keep
/// the normal refresh path (mass overrides are rebuilt by `lunco-usd-avian`).
pub fn claims_edit(reader: &lunco_usd_bevy::StageView<'_>, prim: &SdfPath, attr: &str) -> bool {
    if attr.starts_with("physxVehicleWheel:") {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
    }
    if attr == "lunco:wheel:steerAxis" {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
    }
    if attr.starts_with("lunco:suspension:") || attr.starts_with("physxVehicleSuspension:") {
        return reader.has_api_schema(prim, "PhysxVehicleSuspensionAPI");
    }
    if attr.starts_with("lunco:tire:") || attr.starts_with("physxVehicleTire:") {
        return reader.has_api_schema(prim, "PhysxVehicleTireAPI");
    }
    if matches!(attr, "physics:dynamicFriction" | "physics:staticFriction") {
        return reader.has_api_schema(prim, "PhysxVehicleTireAPI");
    }
    if matches!(
        attr,
        "physxVehicleWheelAttachment:wheel"
            | "physxVehicleWheelAttachment:tire"
            | "physxVehicleWheelAttachment:suspension"
            | "physxVehicleWheelAttachment:index"
    ) {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAttachmentAPI");
    }
    // Vehicle-root knobs: steering lock and drive-kernel selection re-derive in
    // place; a subtree refresh of the whole rover root would tear down live
    // physics bodies.
    if attr == "physxVehicleAckermannSteering:maxSteerAngle"
        || attr == "physxVehicleAckermannSteering:strength"
        || attr == "lunco:driveKernel"
    {
        return true;
    }
    // A connection transform on a `DriveMix` term prim (`lunco:factor:throttle`
    // and friends). `resync_wheels_for_stage` re-derives EVERY vehicle root of
    // the stage, so claiming the edit on the term prim resyncs the mix it
    // belongs to without the caller resolving the owning vessel. The prefix is
    // shared with the co-simulation port graph, so the claim is scoped to prims
    // under a `DriveMix` scope — a factor on a cosim connection is not a wheel
    // edit and must keep the normal refresh path.
    if attr.starts_with("lunco:factor:") {
        return prim
            .as_str()
            .rsplit_once('/')
            .and_then(|(parent, _)| parent.rsplit_once('/'))
            .is_some_and(|(_, scope)| scope == "DriveMix");
    }
    false
}

/// One wheel's re-read result, staged so the `!Send` stage borrow is released
/// before the world is mutated.
struct WheelUpdate {
    entity: Entity,
    physical: bool,
    params: WheelParams,
    /// Steering lock from the wheel's vehicle, when it has a steering system.
    max_steer_angle: Option<f64>,
    /// Ackermann correction strength from the owning vehicle.
    ackermann_strength: f64,
}

/// Re-derive every spawned wheel (and vehicle-root drive mix) of `stage` from
/// the live composed stage, IN PLACE. Resyncs ALL wheels of the stage rather
/// than only the edited prim: suspension/tire attrs may be authored on a
/// separate referenced prim (attachment topology), vehicle-level attrs fan out
/// to every wheel, and a rover has ≤6 wheels — re-reading them all is cheap and
/// makes the resync a fixed point (double-firing from both funnels is
/// harmless).
///
/// A wheel whose re-read fails is a terminal authored-state error for the active
/// scene. The resync raises the shared runtime fault and safety hold after the
/// stage borrow is released; it never continues with stale wheel parameters.
pub fn resync_wheels_for_stage(world: &mut World, id: AssetId<UsdStageAsset>) {
    // 1. Collect this stage's spawned wheels + vehicle roots (plain data out).
    let mut rows: Vec<(Entity, String, bool)> = Vec::new();
    {
        let mut q = world.query::<(
            Entity,
            &UsdPrimPath,
            Option<&WheelRaycast>,
            Option<&crate::PhysicalWheel>,
        )>();
        for (e, prim, rc, pw) in q.iter(world) {
            if prim.stage_handle.id() != id || (rc.is_none() && pw.is_none()) {
                continue;
            }
            rows.push((e, prim.path.clone(), pw.is_some()));
        }
    }
    let mut vehicles: Vec<(Entity, String)> = Vec::new();
    {
        // `OutputPorts` identifies a VEHICLE ROOT here (only a rover root carries
        // one). Deliberately not `DriveMix`: a root whose mix failed to derive still
        // needs to appear in this list, because the re-derive below is exactly what
        // can give it one.
        let mut q = world.query::<(Entity, &UsdPrimPath, &lunco_core::OutputPorts)>();
        for (e, prim, _) in q.iter(world) {
            if prim.stage_handle.id() == id {
                vehicles.push((e, prim.path.clone()));
            }
        }
    }
    if rows.is_empty() && vehicles.is_empty() {
        return;
    }

    // 2. Re-read under one short borrow of the `!Send` stage, then release it —
    //    the appliers below mutate the world (same pattern as
    //    `refresh_domes_live`).
    let mut updates: Vec<WheelUpdate> = Vec::new();
    let mut mixes: Vec<(Entity, Option<lunco_mobility::kernels::DriveMix>)> = Vec::new();
    let mut failures: Vec<(Option<Entity>, String, String)> = Vec::new();
    {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        // This runs only for a stage-change resync, so rebuilding its small
        // stage-local topology snapshot is correct and avoids coupling this
        // exclusive live-edit path to the normal projector cache.
        let mut topology = crate::StageJointTopology::default();
        crate::collect_joint_scan_read(&view, &mut topology);
        for (entity, path, physical) in &rows {
            let Ok(sp) = SdfPath::new(path) else { continue };
            if topology.invalid_wheel_attachments.contains(path) {
                failures.push((
                    Some(*entity),
                    path.clone(),
                    "malformed or ambiguous wheel attachment topology".to_owned(),
                ));
                continue;
            }
            let susp = topology
                .wheel_attachment_targets
                .get(path)
                .and_then(|s| SdfPath::new(s).ok());
            let tire = topology
                .wheel_attachment_tires
                .get(path)
                .and_then(|s| SdfPath::new(s).ok());
            match WheelParams::read(&view, &sp, susp.as_ref(), tire.as_ref()) {
                Ok(params) => {
                    let (max_steer_angle, ackermann_strength) =
                        match crate::steering_vehicle_of(&view, path) {
                            Some(vehicle) => {
                                match crate::steering_vehicle_params(&view, &vehicle) {
                                    Ok((max, strength)) => (Some(max), strength),
                                    Err(reason) => {
                                        failures.push((
                                            Some(*entity),
                                            path.clone(),
                                            format!("invalid Ackermann steering: {reason}"),
                                        ));
                                        continue;
                                    }
                                }
                            }
                            None => (None, 0.0),
                        };
                    updates.push(WheelUpdate {
                        entity: *entity,
                        physical: *physical,
                        params,
                        max_steer_angle,
                        ackermann_strength,
                    });
                }
                Err(missing) => failures.push((
                    Some(*entity),
                    path.clone(),
                    format!("missing or invalid required attributes: {missing:?}"),
                )),
            }
        }
        for (e, path) in &vehicles {
            let Ok(sp) = SdfPath::new(path) else { continue };
            mixes.push((*e, crate::derive_drive_mix(&view, &sp, path)));
        }
    }

    for (entity, subject, detail) in failures {
        let first = world
            .get_resource_or_insert_with(lunco_core::RuntimeFaults::default)
            .raise(
                "usd-wheel-resync-invalid",
                entity,
                subject.clone(),
                detail.clone(),
            );
        world
            .get_resource_or_insert_with(lunco_physics::PhysicsHolds::default)
            .set(lunco_physics::PhysicsHolds::SAFETY_FAILURE, true);
        if first {
            error!("[wheel resync] terminal authored-state failure on {subject}: {detail}");
        }
    }

    // 3. Apply in place. NEVER touch entity existence, `JointCollisionDisabled`,
    //    `Position`, or `UsdSimProcessed`.
    let wheel_count = updates.len();
    for u in &updates {
        if !u.physical {
            if let Some(mut wheel) = world.get_mut::<WheelRaycast>(u.entity) {
                u.params.apply_to_raycast(&mut wheel);
            }
            if let Some(mut susp) = world.get_mut::<Suspension>(u.entity) {
                u.params.apply_to_suspension(&mut susp);
            }
            if let (Some(susp), Some(mut ray)) = (
                u.params.suspension,
                world.get_mut::<avian3d::prelude::RayCaster>(u.entity),
            ) {
                ray.origin = DVec3::new(
                    0.0,
                    lunco_mobility::strut_offset(susp.rest_length, u.params.radius),
                    0.0,
                );
                ray.max_distance = lunco_mobility::suspension_ray_max_distance(susp.rest_length);
            }
            if let (Some(lock), Some(mut steer)) = (
                u.max_steer_angle,
                world.get_mut::<SteeringActuator>(u.entity),
            ) {
                steer.max_steer_angle = lock;
                steer.ackermann_strength = u.ackermann_strength;
            }
            continue;
        }

        // Physical wheel: body-side numbers…
        let (old_radius, old_width, axis_rot) = match world.get::<crate::PhysicalWheel>(u.entity) {
            Some(pw) => (pw.wheel_radius, pw.wheel_width, pw.axis_rot),
            None => continue,
        };
        if let Some(mut pw) = world.get_mut::<crate::PhysicalWheel>(u.entity) {
            pw.wheel_radius = u.params.radius as f32;
            pw.wheel_width = u.params.width as f32;
        }
        if let Some(mut density) = world.get_mut::<ColliderDensity>(u.entity) {
            density.0 = u.params.wheel_density();
        }
        if let Some(mut friction) = world.get_mut::<Friction>(u.entity) {
            friction.dynamic_coefficient = u.params.friction_mu;
            friction.static_coefficient = u.params.friction_mu;
        }
        if let Some(mut tire) = world.get_mut::<JointedWheelTire>(u.entity) {
            tire.radius = u.params.radius;
            tire.axle_inertia = u.params.axle_inertia();
            tire.slip_stiffness = u.params.slip_stiffness;
            tire.lateral_stiffness_graph = u.params.lateral_stiffness_graph;
            tire.min_validated_speed = u.params.min_validated_speed;
            tire.friction_mu = u.params.friction_mu;
            tire.bearing_damping = u.params.bearing_damping;
        }
        // Keep the physical wheel's tensor in lock-step with the composed
        // authored assembly MOI. Updating only density
        // would leave an edited `physxVehicleWheel:moi` inert until a scene
        // reload, while the raycast wheel would apply it immediately.
        world.entity_mut(u.entity).insert((
            crate::physical_wheel_angular_inertia(&u.params, axis_rot),
            avian3d::prelude::NoAutoAngularInertia,
        ));
        // …the collider only when radius or width actually moved (a swap
        // mid-contact can pop the rover; accept as an editing-time artifact,
        // don't pay it for unrelated edits).
        if (old_radius as f64 - u.params.radius).abs() > 1e-6
            || (old_width as f64 - u.params.width).abs() > 1e-6
        {
            let radius = u.params.radius;
            let cyl = Collider::cylinder(radius, u.params.width);
            let collider = if axis_rot.abs_diff_eq(Quat::IDENTITY, 1e-5) {
                cyl
            } else {
                Collider::compound(vec![(
                    Position(DVec3::ZERO),
                    Rotation(axis_rot.as_dquat()),
                    cyl,
                )])
            };
            world.entity_mut(u.entity).insert(collider);
        }
        // …and the joint-side authored steering numbers, on the synthesized
        // joint whose `body2` is this wheel. Torque remains on the solved
        // mechanical port and therefore needs no parameter copy here.
        let mut joint_entity: Option<Entity> = None;
        {
            let mut q = world.query::<(Entity, &RevoluteJoint)>();
            for (je, joint) in q.iter(world) {
                if joint.body2 == u.entity {
                    joint_entity = Some(je);
                    break;
                }
            }
        }
        let Some(je) = joint_entity else { continue };
        if let Some(mut actuator) = world.get_mut::<lunco_cosim::JointTorqueActuator>(je) {
            actuator.brake_torque = u.params.brake_torque_max;
            actuator.rotational_inertia = u.params.axle_inertia();
        }
        if let (Some(lock), Some(mut steer)) =
            (u.max_steer_angle, world.get_mut::<SteeringActuator>(je))
        {
            steer.max_steer_angle = lock;
            steer.ackermann_strength = u.ackermann_strength;
        }
    }
    for (e, mix) in mixes {
        if let Some(mix) = mix {
            world.entity_mut(e).insert(mix);
        } else {
            world
                .entity_mut(e)
                .remove::<lunco_mobility::kernels::DriveMix>();
        }
    }
    info!(
        "[wheel resync] stage {:?}: re-derived {} wheel(s), {} vehicle root(s) in place",
        id,
        wheel_count,
        vehicles.len()
    );
}

#[cfg(test)]
mod tests {
    use super::{validate_suspension_values, validate_wheel_values};
    use bevy::math::DVec3;
    use lunco_mobility::TireLateralStiffnessGraph;

    #[test]
    fn authored_wheel_values_accept_the_documented_contract() {
        let mut errors = Vec::new();
        validate_wheel_values(
            &mut errors,
            0.3,
            0.2,
            12.0,
            0.54,
            0.5,
            120.0,
            14_000.0,
            TireLateralStiffnessGraph {
                minimum_normalized_load: 1.0,
                max_stiffness: 1_000.0,
                rest_load: 400.0,
            },
            400.0,
            0.0,
            1.5,
            DVec3::Y,
        );
        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
    }

    #[test]
    fn authored_wheel_values_reject_nonfinite_and_out_of_contract_numbers() {
        let mut errors = Vec::new();
        validate_wheel_values(
            &mut errors,
            f64::NAN,
            0.0,
            f64::INFINITY,
            -1.0,
            -1.0,
            -1.0,
            -1.0,
            TireLateralStiffnessGraph {
                minimum_normalized_load: -1.0,
                max_stiffness: -1.0,
                rest_load: -1.0,
            },
            -1.0,
            -0.1,
            -1.0,
            DVec3::ZERO,
        );
        for name in [
            "physxVehicleWheel:radius",
            "physxVehicleWheel:width",
            "physxVehicleWheel:mass",
            "physxVehicleWheel:moi",
            "physxVehicleWheel:dampingRate",
            "physxVehicleWheel:maxBrakeTorque",
            "physxVehicleTire:longitudinalStiffness",
            "physxVehicleTire:lateralStiffnessGraph:minNormalizedLoad",
            "physxVehicleTire:lateralStiffnessGraph:maxStiffness",
            "physxVehicleTire:restLoad",
            "lunco:tire:minValidatedSpeed",
            "physics:dynamicFriction",
            "lunco:wheel:steerAxis",
        ] {
            assert!(
                errors.iter().any(|error| error.starts_with(name)),
                "missing validation error for {name}: {errors:?}"
            );
        }
    }

    #[test]
    fn authored_suspension_values_reject_nonfinite_and_out_of_contract_numbers() {
        let mut errors = Vec::new();
        validate_suspension_values(&mut errors, -0.01, f64::NAN, -1.0);
        assert_eq!(errors.len(), 3, "unexpected validation errors: {errors:?}");
        assert!(errors[0].starts_with("lunco:suspension:restLength"));
        assert!(errors[1].starts_with("physxVehicleSuspension:springStrength"));
        assert!(errors[2].starts_with("physxVehicleSuspension:springDamperRate"));
    }

    #[test]
    fn zero_suspension_rest_length_is_valid_only_as_an_authored_rigid_mount() {
        let mut errors = Vec::new();
        validate_suspension_values(&mut errors, 0.0, 15_000.0, 5_000.0);
        assert!(
            errors.is_empty(),
            "unexpected validation errors: {errors:?}"
        );
    }
}
