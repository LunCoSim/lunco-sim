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
//! | moment of inertia | `physxVehicleWheel:moi` | no (0 ⇒ derived ½·m·r² from authored mass+radius) |
//! | peak axle torque | MOTOR `lunco:motor:stallTorque` x gearbox `ratio` x `efficiency` | via motor |
//! | no-load axle speed | MOTOR `lunco:motor:noLoadSpeed` / gearbox `ratio` | via motor |
//! | bearing damping | `physxVehicleWheel:dampingRate` | yes |
//! | brake torque | `physxVehicleWheel:maxBrakeTorque` | yes |
//! | slip stiffness (longitudinal) | `physxVehicleTire:longitudinalStiffness` | yes |
//! | cornering stiffness (lateral) | `physxVehicleTire:lateralStiffness` | no (schema fallback 0.0) |
//! | Coulomb μ (analytic) | `lunco:tire:frictionCoefficient` | yes |
//! | contact μ (solver) | `physics:dynamicFriction` | yes |
//! | steer axis | `lunco:wheel:steerAxis` | yes |
//! | motor damping | `lunco:wheel:driveDamping` | yes |
//! | suspension | `lunco:suspension:restLength` + `physxVehicleSuspension:springStrength`/`:springDamperRate` | raycast only |
//!
//! ## One no-load speed for both realizations
//!
//! `physxVehicleEngine:maxRotationSpeed` is THE no-load axle speed, and both
//! kinds obey it: the joint wheel's velocity motor targets it
//! (`MotorActuator::max_omega`), and the raycast wheel rolls its drive force
//! off toward it (`lunco_mobility::drive_force_mag`), so both self-limit at
//! `ω_max · r`. There used to be a second name for the same quantity —
//! `lunco:wheel:maxDriveOmega`, read only by the joint path — and the two were
//! authored 60 vs 12, which is why raycast rovers drove ~5× too fast. The
//! second name is GONE; there is no alias and no fallback.
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
use bevy::log::{info, warn};
use bevy::math::DVec3;
use bevy::prelude::{Entity, Quat, World};
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
    /// Explicit axle moment of inertia, kg·m² (`physxVehicleWheel:moi`).
    /// 0 ⇒ DERIVED as the solid-cylinder ½·m·r² from the authored `physics:mass`
    /// and `physxVehicleWheel:radius`. That is a derivation from authored
    /// physics, not an invented default — no number enters that nothing authored.
    pub moment_of_inertia: f64,
    /// Engine peak drive torque, N·m (`physxVehicleEngine:peakTorque`).
    pub peak_torque: f64,
    /// No-load axle speed, rad/s (`physxVehicleEngine:maxRotationSpeed`). THE
    /// top-speed parameter for BOTH realizations: the joint motor targets it,
    /// the raycast drive force rolls off toward it, so both cap at `ω·r`.
    pub max_rotation_speed: f64,
    /// Bearing + rolling drag, N·m·s (`physxVehicleWheel:dampingRate`). A
    /// physical property of the hub in its own right — REQUIRED, never inferred
    /// from the drive torque.
    pub bearing_damping: f64,
    /// Lock-up authority, N·m (`physxVehicleWheel:maxBrakeTorque`).
    pub brake_torque_max: f64,
    /// Tire longitudinal stiffness (`physxVehicleTire:longitudinalStiffness`).
    pub slip_stiffness: f64,
    /// Tire CORNERING stiffness (`physxVehicleTire:lateralStiffness`) — side
    /// force per m/s of lateral slip, before the Coulomb cone.
    ///
    /// The PhysX schema's own companion to `longitudinalStiffness`, and read on
    /// the schema's terms: it declares a `0.0` fallback, so an unauthored tire
    /// resolves to zero here rather than raising a missing-attribute error. Zero
    /// is a legal (if unhelpful) tire — no cornering grip at all — and the place
    /// to state a real value is the tire asset, which every shipped tire does.
    pub lateral_stiffness: f64,
    /// Coulomb μ from the wheel's TIRE (`lunco:tire:frictionCoefficient`,
    /// composed through the `tire` variant). The ANALYTIC cone: the raycast
    /// tyre model saturates its patch force at exactly `friction_mu · N`.
    pub friction_mu: f64,
    /// Contact μ for the SOLVER (`physics:dynamicFriction`, `UsdPhysicsMaterialAPI`
    /// on the same tyre). The coefficient the physical wheel's avian contact is
    /// given so that it reproduces `friction_mu`'s cone — see the sweep recorded
    /// in `components/mobility/tires/regolith.usda`. A second number because
    /// avian's XPBD contact friction under sustained sliding delivers well under
    /// the nominal cone, and the tyre's real μ must not be bent to hide that.
    pub contact_friction: f64,
    /// Raked steering-head axis, wheel-local (`lunco:wheel:steerAxis`).
    pub steer_axis: DVec3,
    /// Velocity-tracking aggressiveness, 1/s (`lunco:wheel:driveDamping`).
    pub drive_damping: f64,
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
    ///
    /// `powertrain` is the motor (and optional gearbox) that turns this wheel, found
    /// by the caller via `lunco:motor:drivenWheel`. Torque and no-load speed come from
    /// it, NOT from the wheel: those used to be `physxVehicleEngine:peakTorque` and
    /// `:maxRotationSpeed` authored on the wheel prim, which is a vehicle-level PhysX
    /// attribute misapplied to a part — and with no motor to own them, the same
    /// quantity ended up authored twice under two names and rovers drove 5× too fast in
    /// one realization. `None` means an undriven wheel (a castor, a trailer wheel):
    /// zero torque, and legitimate to author.
    pub fn read(
        reader: &lunco_usd_bevy::StageView<'_>,
        wheel: &SdfPath,
        attachment_suspension: Option<&SdfPath>,
        powertrain: Option<&crate::powertrain::PowertrainParams>,
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
        // From the MOTOR behind the wheel, geared. An undriven wheel has no motor and
        // therefore no torque — that is a castor, not a wheel with a default torque.
        // `max(1e-3)` on the speed keeps the raycast rolloff's divisor finite; it is a
        // numerical guard, not a fallback value.
        let peak_torque = powertrain.map_or(0.0, |p| p.axle_peak_torque());
        let max_rotation_speed = powertrain.map_or(1e-3, |p| p.axle_no_load_speed().max(1e-3));
        let bearing_damping = req("physxVehicleWheel:dampingRate");
        let brake_torque_max = req("physxVehicleWheel:maxBrakeTorque");
        let slip_stiffness = req("physxVehicleTire:longitudinalStiffness");
        // NOT `req`: the PhysX schema declares this one with a `0.0` fallback, so
        // it always composes to a value and there is nothing to report missing.
        // Reading it any other way would invent a required-ness the schema does
        // not state — see the field doc.
        let lateral_stiffness = reader
            .real(wheel, "physxVehicleTire:lateralStiffness")
            .unwrap_or(0.0);
        let friction_mu = req("lunco:tire:frictionCoefficient");
        let contact_friction = req("physics:dynamicFriction");
        let drive_damping = req("lunco:wheel:driveDamping");

        // The ONE non-required number, and it is a DERIVATION, not a default:
        // 0/unauthored means "solid cylinder", i.e. ½·m·r² computed downstream
        // from the authored mass and radius. Nothing is invented.
        let moment_of_inertia = reader.real(wheel, "physxVehicleWheel:moi").unwrap_or(0.0);

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
            lateral_stiffness,
            friction_mu,
            contact_friction,
            steer_axis,
            drive_damping,
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
        wheel.max_rotation_speed = self.max_rotation_speed;
        wheel.bearing_damping = self.bearing_damping;
        wheel.friction_mu = self.friction_mu;
        wheel.slip_stiffness = self.slip_stiffness;
        wheel.lateral_grip_stiffness = self.lateral_stiffness;
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

    /// The physical wheel's axle motor model. The
    /// [`lunco_hardware::MotorActuator`] enables it only for a non-zero command
    /// and writes the live motor/gearbox torque curve every tick.
    ///
    /// The torque cap is deliberately NOT fixed here. It is the authored motor
    /// curve `τ(ω) = τ_stall·(1 − ω/ω_noload)`, which depends on this tick's axle
    /// rate, so the actuator writes `motor.max_torque` every tick; a constant cap
    /// authored at build time would turn the physical wheel into a speed source
    /// disconnected from its motor power.
    ///
    /// It is built DISABLED, not left at `AngularMotor::new`'s `Scalar::MAX`
    /// torque. A motor born with unbounded torque and a target velocity of zero is
    /// an infinitely strong brake, and the physics steps between the joint being
    /// spawned and the actuator's first write were enough to fire an unbounded
    /// impulse and throw every body in the rig out of the world
    /// (`[physics] body left the world`, velocities of ~1e6 m/s). Note that
    /// `with_max_torque(0.0)` does NOT express "no torque" in avian — zero is a
    /// sentinel for UNLIMITED, see the warning in `MotorActuator`. Disabled is the
    /// honest starting point in any case: an axle exerts no torque until something
    /// commands it, and the actuator enables the motor on the first commanded tick.
    ///
    /// `lunco:wheel:driveDamping` is what remains meaningful: with the torque
    /// capped by the curve the motor is saturated for nearly the whole speed
    /// range, so the damping only shapes the last approach to no-load, where the
    /// curve has already fallen to near zero. That is why sweeping it 30 → 150
    /// produced byte-identical traces — not a broken parameter, a parameter whose
    /// regime the rover never entered.
    pub fn drive_motor(&self) -> AngularMotor {
        AngularMotor::new_disabled(MotorModel::AccelerationBased {
            stiffness: 0.0,
            damping: self.drive_damping,
        })
        // `0` is Avian's unlimited sentinel, but the motor is disabled until
        // MotorActuator writes a positive live torque cap in the first command.
        .with_max_torque(0.0)
    }

    /// Axle moment of inertia, kg·m² — authored `physxVehicleWheel:moi` when it
    /// is stated, otherwise the solid-disk derivation `½·m·r²` from the authored
    /// mass and radius.
    ///
    /// The SAME derivation `WheelRaycast::axle_inertia` applies on the raycast
    /// side (it cannot be shared as code — `lunco-mobility` does not depend on
    /// this crate — so it is shared as a rule, and both are fed by this reader).
    /// The physical wheel's real inertia comes from its cylinder collider at
    /// `wheel_density()`, which is ½·m·r² about the axle by construction, so this
    /// is that same number and not a parallel one.
    pub fn axle_inertia(&self) -> f64 {
        if self.moment_of_inertia > 0.0 {
            self.moment_of_inertia
        } else {
            0.5 * self.mass * self.radius * self.radius
        }
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
fn read_suspension_attrs(
    reader: &lunco_usd_bevy::StageView<'_>,
    prim: &SdfPath,
) -> Option<SuspensionParams> {
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
// `Port` children and visual child while `UsdSimProcessed` survives, so
// the sim params are never re-derived, the `MotorActuator` points at a dead
// port, and the chassis-owned joint dangles. The resync mutates the spawned
// components in place — entity ids, joints, `JointCollisionDisabled`, ports and
// `UsdSimProcessed` are never touched.
// ---------------------------------------------------------------------------

/// Attribute families [`resync_wheels_for_stage`] claims from the generic
/// refresh fallback. Prim-scoped where a name is not wheel-specific:
/// `physics:mass` is claimed only on a wheel prim — on a chassis it must keep
/// the normal refresh path (mass overrides are rebuilt by `lunco-usd-avian`).
pub fn claims_edit(reader: &lunco_usd_bevy::StageView<'_>, prim: &SdfPath, attr: &str) -> bool {
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
    if attr == "physxVehicleAckermannSteering:maxSteerAngle" || attr == "lunco:driveKernel" {
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
        // `ActuatorPorts` identifies a VEHICLE ROOT here (only a rover root carries
        // one). Deliberately not `DriveMix`: a root whose mix failed to derive still
        // needs to appear in this list, because the re-derive below is exactly what
        // can give it one.
        let mut q = world.query::<(Entity, &UsdPrimPath, &lunco_core::ActuatorPorts)>();
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
            let powertrain = crate::powertrain::find_for_wheel(&view, &sp);
            match WheelParams::read(&view, &sp, susp.as_ref(), powertrain.as_ref()) {
                Ok(params) => {
                    let max_steer_angle = crate::steering_vehicle_of(&view, path)
                        .and_then(|v| view.real(&v, "physxVehicleAckermannSteering:maxSteerAngle"));
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
            if let (Some(lock), Some(mut steer)) = (
                u.max_steer_angle,
                world.get_mut::<SteeringActuator>(u.entity),
            ) {
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
            motor.max_omega = u.params.max_rotation_speed;
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
        id,
        wheel_count,
        vehicles.len()
    );
}
