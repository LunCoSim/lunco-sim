//! # Unified wheel parameter model
//!
//! ONE reader for BOTH wheel kinds. A wheel prim's full dynamics — drivetrain
//! (peak torque, spin limits, brake), tire (μ, slip stiffness), inertia and
//! optional suspension compliance — are read here into a single [`WheelParams`],
//! regardless of whether the wheel is realised as a raycast wheel
//! (`lunco_mobility::WheelRaycast`, analytical spring + force-at-hub) or a
//! physical wheel (avian `RevoluteJoint` + velocity motor). The two kinds
//! differ ONLY in how force is generated; every number they act on comes from
//! the same attributes with the same strictness.
//!
//! ## Attribute provenance
//!
//! PhysX-compatible names are used where NVIDIA's vehicle schema models the
//! concept (we adopt NAMES, not PhysX runtime semantics — see
//! `core/physxSchema.usda`); `lunco:` names cover LunCo-only concepts:
//!
//! | Param | Attribute | Required |
//! |---|---|---|
//! | radius | `physxVehicleWheel:radius` | yes |
//! | mass | `physics:mass` | yes |
//! | moment of inertia | `physxVehicleWheel:moi` | no (0 ⇒ ½·m·r²) |
//! | peak drive torque | `physxVehicleEngine:peakTorque` | yes |
//! | max rotation speed | `physxVehicleEngine:maxRotationSpeed` | yes |
//! | bearing damping | `physxVehicleWheel:dampingRate` | no (⇒ peak/maxRot) |
//! | brake torque | `physxVehicleWheel:maxBrakeTorque` | yes |
//! | slip stiffness | `physxVehicleTire:longitudinalStiffness` | yes |
//! | Coulomb μ | `lunco:tire:frictionCoefficient` | yes |
//! | grip stiffness | `lunco:wheel:contactGripStiffness` | yes |
//! | drive force/normal | `lunco:wheel:driveForcePerNormal` | yes |
//! | steer axis | `lunco:wheel:steerAxis` | yes |
//! | motor target ω | `lunco:wheel:maxDriveOmega` | yes |
//! | motor damping | `lunco:wheel:driveDamping` | yes |
//! | stall torque gain | `lunco:wheel:stallTorqueGain` | yes |
//! | suspension | `lunco:suspension:restLength` + `physxVehicleSuspension:springStrength`/`:springDamperRate` | raycast only |
//!
//! ## Strictness
//!
//! NO Rust fallback values. Every required attribute missing from the composed
//! prim is an asset error, collected so one bad wheel reports ALL of them, not
//! just the first. The authored defaults live in
//! `components/mobility/wheel.usda`, which every wheel composes — one authored
//! set is what makes "same defaults for both variants" true.

use avian3d::prelude::{
    AngularMotor, Collider, ColliderDensity, MotorModel, Position, RevoluteJoint, Rotation,
};
use bevy::asset::{AssetId, Handle};
use bevy::math::DVec3;
use bevy::prelude::{Entity, Quat, World};
use bevy::log::{info, warn};
use lunco_hardware::{MotorActuator, SteeringActuator};
use lunco_mobility::{Suspension, WheelRaycast};
use lunco_usd_bevy::{CanonicalStages, UsdPrimPath, UsdRead, UsdStageAsset};
use openusd::sdf::Path as SdfPath;
use std::collections::HashMap;

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

/// The complete authored dynamics of one wheel — the single source both
/// `setup_raycast_wheel` and `setup_physical_wheel` consume, and the single
/// struct the live resync path re-derives.
#[derive(Clone, Copy, Debug)]
pub struct WheelParams {
    /// Wheel radius, m (`physxVehicleWheel:radius`).
    pub radius: f64,
    /// Wheel mass, kg (`physics:mass`). Same value for both kinds — the old
    /// raycast-25 / physical-100 Rust fork is gone; feel is authored.
    pub mass: f64,
    /// Explicit axle moment of inertia, kg·m² (`physxVehicleWheel:moi`);
    /// 0 ⇒ derive ½·m·r².
    pub moment_of_inertia: f64,
    /// Engine peak drive torque, N·m (`physxVehicleEngine:peakTorque`).
    pub peak_torque: f64,
    /// Free-spin bound, rad/s (`physxVehicleEngine:maxRotationSpeed`).
    pub max_rotation_speed: f64,
    /// Bearing drag, N·m·s (`physxVehicleWheel:dampingRate` when authored,
    /// else derived as peakTorque / maxRotationSpeed so airborne spin
    /// terminates at the engine's max rotation speed).
    pub bearing_damping: f64,
    /// Lock-up authority, N·m (`physxVehicleWheel:maxBrakeTorque`).
    pub brake_torque_max: f64,
    /// Tire longitudinal stiffness (`physxVehicleTire:longitudinalStiffness`).
    pub slip_stiffness: f64,
    /// Coulomb μ from the wheel's TIRE (`lunco:tire:frictionCoefficient`,
    /// composed through the `tire` variant).
    pub friction_mu: f64,
    /// Contact grip stiffness (`lunco:wheel:contactGripStiffness`).
    pub contact_grip_stiffness: f64,
    /// Drive force as a multiple of normal force (`lunco:wheel:driveForcePerNormal`).
    pub drive_force_per_normal: f64,
    /// Raked steering-head axis, wheel-local (`lunco:wheel:steerAxis`).
    pub steer_axis: DVec3,
    /// Motor target spin at full throttle, rad/s (`lunco:wheel:maxDriveOmega`).
    pub max_drive_omega: f64,
    /// Velocity-tracking aggressiveness, 1/s (`lunco:wheel:driveDamping`).
    pub drive_damping: f64,
    /// Motor stall torque = peakTorque × this (`lunco:wheel:stallTorqueGain`).
    pub stall_torque_gain: f64,
    /// Suspension compliance; `None` ⇒ none resolves. A raycast wheel treats
    /// that as a hard asset error, a joint wheel does not need it.
    pub suspension: Option<SuspensionParams>,
}

impl WheelParams {
    /// Read every wheel attribute off the composed prim, collecting ALL missing
    /// required names into the error. `attachment_suspension` is the suspension
    /// prim a `PhysxVehicleWheelAttachmentAPI` binds this wheel to (canonical
    /// Omniverse topology), if any; the flat path (attrs composed onto the
    /// wheel prim itself, LunCo's compact composition) is the fallback.
    pub fn read<R: UsdRead>(
        reader: &R,
        wheel: &SdfPath,
        attachment_suspension: Option<&SdfPath>,
    ) -> Result<WheelParams, Vec<&'static str>> {
        let mut missing: Vec<&'static str> = Vec::new();
        let mut req = |name: &'static str| -> f64 {
            match reader.real(wheel, name) {
                Some(v) => v,
                None => {
                    missing.push(name);
                    0.0
                }
            }
        };

        let radius = req("physxVehicleWheel:radius");
        let mass = req("physics:mass");
        let peak_torque = req("physxVehicleEngine:peakTorque");
        let max_rotation_speed = req("physxVehicleEngine:maxRotationSpeed").max(1e-3);
        let brake_torque_max = req("physxVehicleWheel:maxBrakeTorque");
        let slip_stiffness = req("physxVehicleTire:longitudinalStiffness");
        let friction_mu = req("lunco:tire:frictionCoefficient");
        let contact_grip_stiffness = req("lunco:wheel:contactGripStiffness");
        let drive_force_per_normal = req("lunco:wheel:driveForcePerNormal");
        let max_drive_omega = req("lunco:wheel:maxDriveOmega");
        let drive_damping = req("lunco:wheel:driveDamping");
        let stall_torque_gain = req("lunco:wheel:stallTorqueGain");

        // Optional, with DERIVED (not invented) defaults.
        let moment_of_inertia = reader.real(wheel, "physxVehicleWheel:moi").unwrap_or(0.0);
        let bearing_damping = reader
            .real(wheel, "physxVehicleWheel:dampingRate")
            .filter(|&d| d > 0.0)
            .unwrap_or(peak_torque / max_rotation_speed);

        let steer_axis = match lunco_usd_bevy::read_vec3_f64(reader, wheel, "lunco:wheel:steerAxis")
        {
            Some(v) => DVec3::new(v[0], v[1], v[2]),
            None => {
                missing.push("lunco:wheel:steerAxis");
                DVec3::Y
            }
        };

        if !missing.is_empty() {
            return Err(missing);
        }

        let suspension = attachment_suspension
            .and_then(|susp| read_suspension_attrs(reader, susp))
            // A half-authored attachment must not read as "no suspension" —
            // fall through to the flat path.
            .or_else(|| read_suspension_attrs(reader, wheel));

        Ok(WheelParams {
            radius,
            mass,
            moment_of_inertia,
            peak_torque,
            max_rotation_speed,
            bearing_damping,
            brake_torque_max,
            slip_stiffness,
            friction_mu,
            contact_grip_stiffness,
            drive_force_per_normal,
            steer_axis,
            max_drive_omega,
            drive_damping,
            stall_torque_gain,
            suspension,
        })
    }

    /// The raycast realisation: a `WheelRaycast` carrying these numbers.
    pub fn to_wheel_raycast(
        &self,
        drive_port: Entity,
        steer_port: Entity,
        visual_entity: Option<Entity>,
    ) -> WheelRaycast {
        let mut wheel = WheelRaycast {
            wheel_radius: self.radius,
            visual_entity,
            drive_port,
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
        wheel.drive_torque_max = self.peak_torque;
        wheel.bearing_damping = self.bearing_damping;
        wheel.friction_mu = self.friction_mu;
        wheel.slip_stiffness = self.slip_stiffness;
        wheel.contact_grip_stiffness = self.contact_grip_stiffness;
        wheel.brake_torque_max = self.brake_torque_max;
        wheel.drive_force_per_normal = self.drive_force_per_normal;
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

    /// The ONE definition of the physical wheel's axle drive: a
    /// velocity-controlled motor (stiffness 0 — pure velocity control,
    /// mass-auto-scaled) whose stall torque is `peakTorque × stallTorqueGain`.
    /// Stall torque sits well above the steady traction figure so a skid turn
    /// can enforce its left/right speed split; velocity control self-caps the
    /// spin, so the high stall torque can't run away.
    pub fn drive_motor(&self) -> AngularMotor {
        AngularMotor::new(MotorModel::AccelerationBased {
            stiffness: 0.0,
            damping: self.drive_damping,
        })
        .with_max_torque(self.peak_torque * self.stall_torque_gain)
    }

    /// Collider density realising `physics:mass` on the physical wheel's
    /// cylinder collider (`cylinder(r, h = r/2)` ⇒ volume = π·r²·(r/2)).
    ///
    /// Mass goes in via DENSITY, not a forced `Mass`: avian derives
    /// `AngularInertia` from the collider at `ColliderDensity` even when `Mass`
    /// is set, and a forced mass desyncs mass from angular inertia — the
    /// contact+joint solver then can't build enough support impulse and the
    /// rover sinks through the one-sided terrain heightfield.
    pub fn wheel_density(&self) -> f32 {
        let volume = std::f64::consts::PI * self.radius.powi(2) * (self.radius * 0.5);
        (self.mass / volume.max(1e-6)) as f32
    }
}

/// Resolve a wheel's ATTACHMENT suspension prim via the canonical two-step path
/// (doc 53 §3.2):
///
/// 1. **Canonical (relationship):** if a `PhysxVehicleWheelAttachmentAPI` prim
///    targets this wheel, the Pass-1 scan recorded the suspension prim it binds —
///    return that path. Keyed by (stage, wheel path) like `joint_targets`: prim
///    paths are only unique WITHIN a stage, so the same rover loaded twice
///    repeats `/Rover/Wheel_FL`, and matching on the path alone would let one
///    instance resolve another instance's suspension.
/// 2. **Flat (fallback):** `None` — [`WheelParams::read`] then reads the attrs
///    directly off the wheel prim (LunCo's compact composition, where the wheel
///    references the suspension and the attrs compose onto the wheel itself).
pub(crate) fn attachment_suspension_path(
    wheel_prim: &UsdPrimPath,
    wheel_attachment_targets: &HashMap<(Handle<UsdStageAsset>, String), String>,
) -> Option<SdfPath> {
    wheel_attachment_targets
        .get(&(wheel_prim.stage_handle.clone(), wheel_prim.path.clone()))
        .and_then(|s| SdfPath::new(s).ok())
}

/// Read the three suspension attrs off one prim. `None` unless all three are
/// authored — partial authoring is treated as missing (no per-field defaults).
fn read_suspension_attrs<R: UsdRead>(reader: &R, prim: &SdfPath) -> Option<SuspensionParams> {
    Some(SuspensionParams {
        rest_length: reader.real(prim, "lunco:suspension:restLength")?,
        spring_k: reader.real(prim, "physxVehicleSuspension:springStrength")?,
        damping_c: reader.real(prim, "physxVehicleSuspension:springDamperRate")?,
    })
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
// `refresh_prim_subtree`/`reinstantiate_entity` fallback. That fallback is
// actively destructive for wheels: it despawns the wheel's synthesized
// `PhysicalPort` children and visual child while `UsdSimProcessed` survives, so
// the sim params are never re-derived, the `MotorActuator` points at a dead
// port, and the chassis-owned joint dangles. The resync mutates the spawned
// components in place — entity ids, joints, `JointCollisionDisabled`, ports and
// `UsdSimProcessed` are never touched.
// ---------------------------------------------------------------------------

/// Attribute families [`resync_wheels_for_stage`] claims from the generic
/// refresh fallback. Prim-scoped where a name is not wheel-specific:
/// `physics:mass` is claimed only on a wheel prim — on a chassis it must keep
/// the normal refresh path (mass overrides are rebuilt by `lunco-usd-avian`).
pub fn claims_edit<R: UsdRead>(reader: &R, prim: &SdfPath, attr: &str) -> bool {
    const WHEEL_ONLY_PREFIXES: [&str; 7] = [
        "lunco:wheel:",
        "lunco:suspension:",
        "lunco:tire:",
        "physxVehicleWheel:",
        "physxVehicleEngine:",
        "physxVehicleTire:",
        "physxVehicleSuspension:",
    ];
    if WHEEL_ONLY_PREFIXES.iter().any(|p| attr.starts_with(p)) {
        return true;
    }
    // Vehicle-root knobs: steering lock and drive-kernel selection re-derive in
    // place; a subtree refresh of the whole rover root would tear down live
    // physics bodies.
    if attr == "physxVehicleAckermannSteering:maxSteerAngle"
        || attr == "lunco:driveKernel"
        || attr == "lunco:driveMix"
    {
        return true;
    }
    if attr == "physics:mass" {
        return reader.has_api_schema(prim, "PhysxVehicleWheelAPI");
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
}

/// Re-derive every spawned wheel (and vehicle-root drive mix) of `stage` from
/// the live composed stage, IN PLACE. Resyncs ALL wheels of the stage rather
/// than only the edited prim: suspension/tire attrs may be authored on a
/// separate referenced prim (attachment topology), vehicle-level attrs fan out
/// to every wheel, and a rover has ≤6 wheels — re-reading them all is cheap and
/// makes the resync a fixed point (double-firing from both funnels is
/// harmless).
///
/// A wheel whose re-read now FAILS (a half-authored edit removed a required
/// attr) keeps its old values — never break a running wheel; the collected
/// missing-attr warning names what to restore.
pub fn resync_wheels_for_stage(world: &mut World, id: AssetId<UsdStageAsset>) {
    // 1. Collect this stage's spawned wheels + vehicle roots (plain data out).
    let mut rows: Vec<(Entity, String, bool)> = Vec::new();
    let mut stage_handle: Option<Handle<UsdStageAsset>> = None;
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
            stage_handle.get_or_insert_with(|| prim.stage_handle.clone());
            rows.push((e, prim.path.clone(), pw.is_some()));
        }
    }
    let mut vehicles: Vec<(Entity, String)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &UsdPrimPath, &lunco_fsw::FlightSoftware)>();
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
    let mut mixes: Vec<(Entity, lunco_mobility::kernels::DriveMix)> = Vec::new();
    {
        let Some(stages) = world.get_non_send::<CanonicalStages>() else {
            return;
        };
        let Some(cs) = stages.get(id) else { return };
        let view = cs.view();
        // Rebuild the canonical attachment map for suspension resolution — the
        // same Pass-1 scan the spawn path runs.
        let mut attach: HashMap<(Handle<UsdStageAsset>, String), String> = HashMap::new();
        if let Some(handle) = &stage_handle {
            let mut joints = HashMap::new();
            let mut roots = std::collections::HashSet::new();
            crate::collect_joint_scan_read(&view, handle, &mut joints, &mut roots, &mut attach);
        }
        for (entity, path, physical) in &rows {
            let Ok(sp) = SdfPath::new(path) else { continue };
            let susp = stage_handle
                .as_ref()
                .and_then(|h| attach.get(&(h.clone(), path.clone())))
                .and_then(|s| SdfPath::new(s).ok());
            match WheelParams::read(&view, &sp, susp.as_ref()) {
                Ok(params) => {
                    let max_steer_angle = crate::steering_vehicle_of(&view, path).and_then(|v| {
                        view.real(&v, "physxVehicleAckermannSteering:maxSteerAngle")
                    });
                    updates.push(WheelUpdate {
                        entity: *entity,
                        physical: *physical,
                        params,
                        max_steer_angle,
                    });
                }
                Err(missing) => warn!(
                    "[wheel resync] {} now missing required attrs {:?} — keeping \
                     the spawned values (restore the attrs to re-derive)",
                    path, missing
                ),
            }
        }
        for (e, path) in &vehicles {
            let Ok(sp) = SdfPath::new(path) else { continue };
            if let Some(mix) = crate::derive_drive_mix(&view, &sp, path) {
                mixes.push((*e, mix));
            }
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
            if let (Some(lock), Some(mut steer)) =
                (u.max_steer_angle, world.get_mut::<SteeringActuator>(u.entity))
            {
                steer.max_steer_angle = lock;
            }
            continue;
        }

        // Physical wheel: body-side numbers…
        let (old_radius, axis_rot) = match world.get::<crate::PhysicalWheel>(u.entity) {
            Some(pw) => (pw.wheel_radius, pw.axis_rot),
            None => continue,
        };
        if let Some(mut pw) = world.get_mut::<crate::PhysicalWheel>(u.entity) {
            pw.wheel_radius = u.params.radius as f32;
        }
        if let Some(mut density) = world.get_mut::<ColliderDensity>(u.entity) {
            density.0 = u.params.wheel_density();
        }
        // …the collider only when the radius actually moved (a swap mid-contact
        // can pop the rover; accept as an editing-time artifact, don't pay it
        // for unrelated edits).
        if (old_radius as f64 - u.params.radius).abs() > 1e-6 {
            let radius = u.params.radius;
            let cyl = Collider::cylinder(radius, radius * 0.5);
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
        // …and the joint-side numbers, on the synthesized joint whose `body2`
        // is this wheel. The motor is REBUILT from the one definition
        // (`drive_motor`) with its live command preserved —
        // `motor_actuator_system` rewrites `target_velocity` next tick anyway.
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
        if let Some(mut joint) = world.get_mut::<RevoluteJoint>(je) {
            let target_velocity = joint.motor.target_velocity;
            let mut motor = u.params.drive_motor();
            motor.target_velocity = target_velocity;
            joint.motor = motor;
        }
        if let Some(mut motor) = world.get_mut::<MotorActuator>(je) {
            motor.max_omega = u.params.max_drive_omega;
        }
        if let (Some(lock), Some(mut steer)) =
            (u.max_steer_angle, world.get_mut::<SteeringActuator>(je))
        {
            steer.max_steer_angle = lock;
        }
    }
    for (e, mix) in mixes {
        world.entity_mut(e).insert(mix);
    }
    info!(
        "[wheel resync] stage {:?}: re-derived {} wheel(s), {} vehicle root(s) in place",
        id, wheel_count, vehicles.len()
    );
}
