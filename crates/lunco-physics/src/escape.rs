//! "This body left the world" — a one-shot diagnostic for bodies that fall out
//! of the simulation.
//!
//! # Why this exists
//!
//! Two separate failures were diagnosed this session by EYEBALLING RENDERED
//! FRAMES: a rigid body sitting at `y = -1510`, and another at `elev = -737`.
//! Nothing in the engine said a word. A body that tunnels through a heightfield
//! that had not finished baking (the exact window [`PhysicsHolds`] exists to
//! protect) does not crash, does not warn, and does not stop — it simply
//! accelerates away for the rest of the run while the scene above it looks
//! plausible. The cost of finding that by eye is hours; the cost of logging it
//! is one comparison per dynamic body per tick.
//!
//! # Choosing a bound that works for BOTH lunar surface and orbital scenes
//!
//! This engine runs lunar-surface work AND orbital work, so the obvious bound is
//! wrong. `y < -100` flags nothing in a heliocentric scene (where the bridge's
//! own round-trip test exercises positions at `1.5e11` m) and false-positives on
//! any scene whose origin is not the ground. A large absolute radius has the
//! opposite failure: it is the only bound that cannot false-positive on an
//! orbital scene, and it would NOT have caught either motivating failure —
//! `y = -1510` is utterly unremarkable at astronomical scale. A bound that
//! cannot see the bug it was written for is not worth having.
//!
//! So the bound is **derived from the scene**, not assumed: the union AABB of
//! every STATIC collider is what "the world" physically is. Terrain, ground
//! planes and fixed structures are static; they are the things a body can fall
//! through and the things that define where the simulation has floor. A dynamic
//! body far outside that volume has, by construction, nothing left to land on.
//!
//! This gives the right answer in both regimes for the same reason:
//!
//! - **Lunar surface**: the terrain collider spans the site, so the union AABB
//!   is site-sized (hundreds of metres to kilometres). `y = -1510` beneath a
//!   site whose terrain bottoms out near zero is far outside it → flagged.
//! - **Orbital**: there are no static colliders at all, so there is no union
//!   AABB, and the diagnostic **disables itself** ([`WorldBounds::None`]). It
//!   cannot false-positive on a scene it has no opinion about. Silence here is
//!   deliberate and correct: absent a floor, "below the world" is meaningless.
//!
//! # Margins
//!
//! The AABB is expanded before it is used, because legitimate motion leaves the
//! static volume all the time:
//!
//! - **Laterally and downward** by [`ESCAPE_MARGIN_FRACTION`] of the AABB's
//!   largest extent, floored at [`ESCAPE_MARGIN_MIN`]. A rover driving off the
//!   edge of a loaded terrain tile is a paging problem, not an escape, and the
//!   floor keeps a small scene (a test rig with a 2 m ground plane) from
//!   flagging a body that hopped a metre sideways.
//! - **Upward: bounded for local worlds.** The ceiling is derived from the
//!   static extent, with [`ESCAPE_VERTICAL_FACTOR`] scene-scale slack. Orbital
//!   scenes have no static colliders and therefore remain `None`.
//!
//! Non-finite positions and velocities (NaN/±inf) are detected regardless of
//! world bounds and use the same required Rhai containment policy. The default
//! stops only the affected dynamic object; an unavailable or invalid policy
//! still fails closed with a physics hold.
//!
//! # Known limitations (stated, not hidden)
//!
//! - A scene whose only floor is a DYNAMIC or KINEMATIC body (a moving platform
//!   with nothing static beneath it) contributes no bounds, so escapes off it go
//!   unreported. Static geometry is the discriminator; there is no cheap
//!   substitute.
//! - The bounds track the static set as it pages in, so a body already outside
//!   the bounds when new terrain loads AROUND it will be reported once and not
//!   re-evaluated. Once-per-entity is a deliberate anti-spam choice
//!   (see below), and a false positive costs one log line.
//! - Bounds are in avian's `Position` frame (the BigSpace root frame), the same
//!   frame the solver works in, so no coordinate conversion is involved and the
//!   logged numbers are directly comparable to anything else avian prints.
//!
//! # Cost
//!
//! One `Vector` comparison per body whose `Position` was written this tick
//! (awake dynamics; statics and sleepers are change-tick-filtered out of the
//! query), and a [`HashSet`] insert on the first (and only) report per entity.
//! The bounds themselves are recomputed only when a static collider's AABB
//! actually changes, which after terrain settles is never. Nothing allocates
//! per frame.
//!
//! An escaped or non-finite state invokes the required `physics.body_escape`
//! Rhai policy. The application default disables only its dynamic
//! joint-connected object, including joints and colliders, and leaves the rest
//! of physics running.
//! Missing or invalid policy results fail closed with a physics hold. The same
//! policy receives non-finite position or velocity as `non_finite_state`, so
//! the default also isolates that object without holding unrelated physics.
//! Every first report emits the shared `TelemetryEvent` named
//! `physics-body-escaped`; the logger and workbench Recent Events consume this
//! one observation without log parsing or UI-specific dependencies.

use avian3d::math::{Scalar, Vector};
use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use lunco_core::GlobalEntityId;
use lunco_hooks::HookValue;
use lunco_telemetry_core::{Severity, TelemetryEvent, TelemetryValue};

/// Policy seam for a changed rigid body that left the bounds or became invalid.
/// `ctx` includes kind, optional path and global_id, position_m, velocity_mps,
/// world_min_m, and world_max_m; the returned string is a containment action.
pub const BODY_ESCAPE_POLICY_HOOK: &str = "physics.body_escape";

lunco_hooks::declare_hook! {
    id: BODY_ESCAPE_POLICY_HOOK,
    owner: "lunco-physics",
    description: "Choose how to contain an escaped or non-finite rigid-body state.",
    signature: [ctx: Map],
    output: String,
    deterministic: true,
    required: true,
    installable: true,
}

/// Fraction of the static world's largest extent added as lateral/downward
/// slack. Ten percent is comfortably more than terrain-tile paging jitter and
/// far less than the scale of a genuine escape.
pub const ESCAPE_MARGIN_FRACTION: Scalar = 0.1;

/// Floor for the computed margin, in metres. Keeps a small scene (a unit-test
/// rig with a 2 m ground plane) from flagging a body that hopped a metre.
pub const ESCAPE_MARGIN_MIN: Scalar = 100.0;

/// Additional local-world height, in multiples of the static world's largest
/// extent. This catches runaway integration without rewriting physics state.
pub const ESCAPE_VERTICAL_FACTOR: Scalar = 10.0;

/// Marks an entity in an articulated object stopped by the escape policy.
/// Readiness release leaves Avian's disable in place while this marker remains.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct PhysicsEscapePaused;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EscapePolicyAction {
    PauseObject,
    PauseWorld,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EscapeKind {
    FiniteWorldExit,
    NonFiniteState,
}

impl EscapeKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::FiniteWorldExit => "finite_world_exit",
            Self::NonFiniteState => "non_finite_state",
        }
    }
}

impl EscapePolicyAction {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "pause_object" => Some(Self::PauseObject),
            "pause_world" => Some(Self::PauseWorld),
            _ => None,
        }
    }
}

fn hook_vector(value: Vector) -> HookValue {
    HookValue::Array(vec![
        HookValue::Float(value.x as f64),
        HookValue::Float(value.y as f64),
        HookValue::Float(value.z as f64),
    ])
}

fn escape_policy_action(
    kind: EscapeKind,
    path: Option<&Name>,
    global_id: Option<u64>,
    position: Vector,
    velocity: Vector,
    bounds: WorldBounds,
) -> Result<EscapePolicyAction, String> {
    let (world_min, world_max) = match bounds {
        WorldBounds::Some { min, max } => (hook_vector(min), hook_vector(max)),
        WorldBounds::None if kind == EscapeKind::NonFiniteState => {
            (HookValue::Unit, HookValue::Unit)
        }
        WorldBounds::None => {
            return Err("a finite world exit was reported without static-world bounds".to_owned());
        }
    };
    let context = HookValue::map([
        ("kind", HookValue::str(kind.as_str())),
        (
            "path",
            path.map_or(HookValue::Unit, |name| HookValue::str(name.as_str())),
        ),
        (
            "global_id",
            global_id.map_or(HookValue::Unit, |id| HookValue::str(id.to_string())),
        ),
        ("position_m", hook_vector(position)),
        ("velocity_mps", hook_vector(velocity)),
        ("world_min_m", world_min),
        ("world_max_m", world_max),
    ]);
    let Some(result) = lunco_hooks::invoke(BODY_ESCAPE_POLICY_HOOK, &[context]) else {
        return Err(format!(
            "required `{BODY_ESCAPE_POLICY_HOOK}` policy is not installed"
        ));
    };
    let value =
        result.map_err(|error| format!("`{BODY_ESCAPE_POLICY_HOOK}` policy failed: {error}"))?;
    match value {
        HookValue::Str(value) => EscapePolicyAction::parse(&value).ok_or_else(|| {
            format!(
                "`{BODY_ESCAPE_POLICY_HOOK}` returned unsupported action `{value}`; expected `pause_object` or `pause_world`"
            )
        }),
        other => Err(format!(
            "`{BODY_ESCAPE_POLICY_HOOK}` returned {other:?}; expected a string action"
        )),
    }
}

/// Find the live dynamic bodies joined to `seed`. Static and kinematic anchors
/// terminate traversal so independent mechanisms attached to the same world
/// frame remain independent objects.
fn dynamic_object_island(
    seed: Entity,
    body_modes: &Query<&RigidBody>,
    joint_links: &Query<(Entity, &crate::PhysicsJointLink), Without<JointDisabled>>,
) -> EntityHashSet {
    let mut island = EntityHashSet::default();
    island.insert(seed);
    loop {
        let mut added = false;
        for (_, link) in joint_links.iter() {
            let body0_dynamic = body_modes
                .get(link.body0)
                .is_ok_and(|mode| matches!(mode, RigidBody::Dynamic));
            let body1_dynamic = body_modes
                .get(link.body1)
                .is_ok_and(|mode| matches!(mode, RigidBody::Dynamic));
            if !body0_dynamic || !body1_dynamic {
                continue;
            }
            if island.contains(&link.body0) && island.insert(link.body1) {
                added = true;
            }
            if island.contains(&link.body1) && island.insert(link.body0) {
                added = true;
            }
        }
        if !added {
            return island;
        }
    }
}

fn pause_dynamic_object(
    seed: Entity,
    body_modes: &Query<&RigidBody>,
    joint_links: &Query<(Entity, &crate::PhysicsJointLink), Without<JointDisabled>>,
    colliders: &Query<(Entity, &ColliderOf)>,
    reported: &mut ReportedEscapes,
    commands: &mut Commands,
) -> usize {
    let mut island: Vec<_> = dynamic_object_island(seed, body_modes, joint_links)
        .into_iter()
        .collect();
    island.sort_unstable_by_key(|entity| entity.to_bits());
    let mut island_set = EntityHashSet::default();
    for entity in &island {
        island_set.insert(*entity);
    }
    for (joint, link) in joint_links.iter() {
        if island_set.contains(&link.body0) || island_set.contains(&link.body1) {
            commands.entity(joint).try_insert(JointDisabled);
        }
    }
    for (collider, collider_of) in colliders.iter() {
        if island_set.contains(&collider_of.body) {
            commands
                .entity(collider)
                .try_insert((ColliderDisabled, PhysicsEscapePaused));
        }
    }
    for entity in &island {
        commands
            .entity(*entity)
            .try_insert((RigidBodyDisabled, PhysicsEscapePaused));
        // One event and one policy decision describe the whole articulated
        // object even if several of its bodies crossed the bounds this step.
        reported.0.insert(*entity);
    }
    island.len()
}

/// The volume the simulation has static geometry in, expanded by the margins
/// described in the module docs — or [`WorldBounds::None`] when the scene has no
/// static colliders at all, in which case the diagnostic is inert.
///
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub enum WorldBounds {
    /// No static geometry ⇒ no opinion ⇒ no reports.
    #[default]
    None,
    /// Inclusive bounds in avian's `Position` frame.
    Some { min: Vector, max: Vector },
}

impl WorldBounds {
    /// Is `p` outside the world? Always true for a non-finite position.
    #[inline]
    pub fn escaped(&self, p: Vector) -> bool {
        if !p.is_finite() {
            return true;
        }
        match *self {
            WorldBounds::None => false,
            WorldBounds::Some { min, max } => p.cmplt(min).any() || p.cmpgt(max).any(),
        }
    }
}

/// Entities already handled by the escape boundary. Object-policy membership
/// covers the full dynamic joint island, so one escape produces one policy
/// decision and telemetry event instead of one per wheel or joint endpoint.
#[derive(Resource, Debug, Default)]
pub struct ReportedEscapes(EntityHashSet);

/// Recompute [`WorldBounds`] from the union of every static collider's AABB.
///
/// Change-driven, filtered to STATIC bodies' colliders: avian rewrites
/// `ColliderAabb` for every awake body's colliders each step, so a bare
/// `Changed<ColliderAabb>` probe would fire whenever anything moves — only a
/// static collider's AABB changing (or any removal) says the world changed.
/// Once terrain has settled the probe matches nothing and this system returns
/// immediately. The union itself is recomputed over the full static set when it
/// does fire, which is correct under REMOVAL as well (a shrinking world must
/// shrink its bounds, and an incremental union cannot).
fn update_world_bounds(
    q_changed: Query<&ColliderOf, Changed<ColliderAabb>>,
    mut q_removed: RemovedComponents<ColliderAabb>,
    q_static: Query<(&ColliderAabb, &ColliderOf)>,
    q_bodies: Query<&RigidBody>,
    mut bounds: ResMut<WorldBounds>,
) {
    // DRAIN, don't peek: `is_empty()` leaves the events queued for their whole
    // retention window, so every removal re-fired this full static union for
    // several extra frames. Drained FIRST and unconditionally — folding it into
    // the `||` below would let a static change short-circuit the drain and leak
    // the events back into the next frames.
    let removed_any = q_removed.read().count() > 0;
    // Only a STATIC collider can move the union the loop below computes, so a
    // changed DYNAMIC collider must not re-fire it. Gating on any `ColliderOf`
    // change instead meant a single rover in motion recomputed the full static
    // union every frame — a whole pass over every static collider in the scene.
    let static_changed = q_changed
        .iter()
        .any(|collider_of| matches!(q_bodies.get(collider_of.body), Ok(RigidBody::Static)));
    if !static_changed && !removed_any {
        return;
    }

    let mut min = Vector::INFINITY;
    let mut max = Vector::NEG_INFINITY;
    let mut any = false;
    for (aabb, collider_of) in &q_static {
        // Only STATIC geometry defines the world — see the module docs.
        if !matches!(q_bodies.get(collider_of.body), Ok(RigidBody::Static)) {
            continue;
        }
        // An unbuilt collider carries `ColliderAabb::INVALID` (min = +inf,
        // max = -inf); folding it in would poison the union with infinities.
        if !aabb.min.is_finite() || !aabb.max.is_finite() {
            continue;
        }
        min = min.min(aabb.min);
        max = max.max(aabb.max);
        any = true;
    }

    let next = if any {
        let extent = (max - min).max_element();
        let margin = (extent * ESCAPE_MARGIN_FRACTION).max(ESCAPE_MARGIN_MIN);
        WorldBounds::Some {
            min: min - Vector::splat(margin),
            // Local static geometry defines the scale. Orbital scenes have no
            // static union and remain unbounded through `WorldBounds::None`.
            max: Vector::new(
                max.x + margin,
                max.y + extent * ESCAPE_VERTICAL_FACTOR + margin,
                max.z + margin,
            ),
        }
    } else {
        WorldBounds::None
    };

    if *bounds != next {
        // The bounds ARE the diagnostic's whole opinion, and `None` silently
        // disables it. Logging the transition is what distinguishes "nothing
        // escaped" from "this build had no idea where the world was".
        debug!("[physics] world bounds: {:?} -> {:?}", *bounds, next);
        *bounds = next;
    }
}

/// Observe dynamic bodies outside [`WorldBounds`], invoke the authored policy
/// once per escaped object, and keep missing or invalid policy results fail-closed.
///
/// `Name` carries the USD prim path for USD-spawned bodies: the loader spawns
/// each prim with `Name::new(prim_path)` (`lunco-usd-bevy` visual projection).
/// This crate deliberately does not depend on `lunco-usd-commands` — it is substrate for
/// headless and wasm generators — so `Name` is both the reachable identifier and,
/// in practice, the prim path itself.
fn report_escaped_bodies(
    bounds: Res<WorldBounds>,
    mut reported: ResMut<ReportedEscapes>,
    mut holds: ResMut<crate::PhysicsHolds>,
    mut faults: Option<ResMut<lunco_core::RuntimeFaults>>,
    world_time: Option<Res<lunco_time::WorldTime>>,
    mut commands: Commands,
    body_modes: Query<&RigidBody>,
    joint_links: Query<(Entity, &crate::PhysicsJointLink), Without<JointDisabled>>,
    colliders: Query<(Entity, &ColliderOf)>,
    // Changed position or velocity is the query-level activity filter: avian
    // has no per-variant marker component (`RigidBody` is one enum component),
    // so "dynamic only" cannot be a `With<>` filter. The solver writes every
    // awake dynamic body's position each step; watching velocity changes also
    // catches a bad externally-authored velocity before it produces a position.
    // Statics (terrain tiles — the bulk of the body count) therefore cost one
    // change-tick compare instead of a full fetch + enum match per tick. A
    // sleeping body is skipped too, correctly: escapes accelerate, they never
    // sleep. (A NaN blow-up is still a write, so it still fires.)
    q: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            Option<&Name>,
            Option<&GlobalEntityId>,
            &RigidBody,
        ),
        Or<(Changed<Position>, Changed<LinearVelocity>)>,
    >,
) {
    for (entity, pos, vel, name, global_id, rb) in &q {
        // Kinematic bodies pass the change gate whenever their driver moves
        // them, and an externally teleported static passes it once — but a
        // kinematic body is wherever its driver put it and a static does not
        // fall: neither can "escape", and flagging them would report the
        // driver's intent as an engine fault.
        if !matches!(rb, RigidBody::Dynamic) {
            continue;
        }
        let non_finite_state = !pos.0.is_finite() || !vel.0.is_finite();
        if !non_finite_state && !bounds.escaped(pos.0) {
            continue;
        }
        // `insert` returns false if already present: the whole anti-spam rule.
        if !reported.0.insert(entity) {
            continue;
        }
        let label = name.map(Name::as_str).unwrap_or("<unnamed>");
        let global_id_value = global_id.map(GlobalEntityId::get);
        let global_id_detail =
            global_id_value.map_or_else(|| "none".to_owned(), |id| id.to_string());
        let telemetry_source = global_id_value.unwrap_or(0);
        let timestamp = world_time.as_ref().map(|time| time.epoch_jd).unwrap_or(0.0);
        let sim_secs = world_time.as_ref().map(|time| time.sim_secs).unwrap_or(0.0);
        let escape_kind = if non_finite_state {
            EscapeKind::NonFiniteState
        } else {
            EscapeKind::FiniteWorldExit
        };
        let condition = match escape_kind {
            EscapeKind::FiniteWorldExit => "body left the world",
            EscapeKind::NonFiniteState => "body has non-finite state",
        };
        let mut detail = format!(
            "kind={}; body={label}; entity_bits={}; global_id={global_id_detail}; position={:?}; velocity={:?}; bounds={:?}; sim_secs={sim_secs:.6}",
            escape_kind.as_str(),
            entity.to_bits(),
            pos.0,
            vel.0,
            *bounds,
        );
        let action_name;
        let mut paused_bodies = 0;
        match escape_policy_action(escape_kind, name, global_id_value, pos.0, vel.0, *bounds) {
            Ok(EscapePolicyAction::PauseObject) => {
                paused_bodies = pause_dynamic_object(
                    entity,
                    &body_modes,
                    &joint_links,
                    &colliders,
                    &mut reported,
                    &mut commands,
                );
                action_name = "pause_object";
            }
            Ok(EscapePolicyAction::PauseWorld) => {
                action_name = "pause_world";
                holds.set(crate::PhysicsHolds::SAFETY_FAILURE, true);
                if let Some(faults) = faults.as_deref_mut() {
                    let fault_kind = match escape_kind {
                        EscapeKind::FiniteWorldExit => "physics-body-escaped",
                        EscapeKind::NonFiniteState => "physics-body-nonfinite",
                    };
                    if faults.raise(fault_kind, Some(entity), label, detail.clone()) {
                        error!(
                            "[physics] Rhai policy selected a world hold for {condition}: {label} ({entity})"
                        );
                    }
                }
            }
            Err(error) => {
                action_name = "policy_failed";
                detail.push_str("; policy_error=");
                detail.push_str(&error);
                holds.set(crate::PhysicsHolds::SAFETY_FAILURE, true);
                if let Some(faults) = faults.as_deref_mut() {
                    if faults.raise(
                        "physics-escape-policy-failed",
                        Some(entity),
                        label,
                        detail.clone(),
                    ) {
                        error!(
                            "[physics] terminal runtime failure: {BODY_ESCAPE_POLICY_HOOK} failed for {label} ({entity}): {error}"
                        );
                    }
                }
            }
        }
        detail.push_str(&format!(
            "; policy_action={action_name}; paused_dynamic_bodies={paused_bodies}"
        ));
        commands.trigger(TelemetryEvent {
            name: "physics-body-escaped".to_string(),
            // The telemetry contract reserves 0 for an event without an
            // attached global entity identity; the detail string says `none`.
            source: telemetry_source,
            severity: Severity::Error,
            data: TelemetryValue::String(detail),
            timestamp,
        });
        match action_name {
            "pause_object" => warn!(
                "[physics] {condition}: {} ({entity}) at {:?}, velocity {:?} — Rhai policy paused its articulated object ({paused_bodies} dynamic bodies); the rest of the simulation continues",
                label, pos.0, vel.0,
            ),
            "pause_world" => error!(
                "[physics] {condition}: {} ({entity}) at {:?}, velocity {:?} — Rhai policy paused physics for the whole scene",
                label, pos.0, vel.0,
            ),
            _ => error!(
                "[physics] {condition}: {} ({entity}) at {:?}, velocity {:?} — physics held because its escape policy is unsafe",
                label, pos.0, vel.0,
            ),
        }
    }
}

/// The escape guard judges the result of an admitted solver step, never loading
/// state that merely happens to carry a changed [`Position`].
///
/// Avian intentionally leaves the previous non-zero physics delta in place for
/// the first fixed tick after [`Time<Physics>`] is paused, then clears it after
/// entering `PhysicsSchedule`. Consequently the schedule can execute once while
/// the authoritative physics clock is already paused. Scene readiness relies on
/// that pause while USD entities are still being mounted into their final
/// BigSpace frame. Looking only at schedule placement therefore mistakes the
/// authored loading pose for a post-solver escape.
///
/// A cinematic single-step is admitted by temporarily unpausing this same
/// clock, so the clock state is the complete contract for both normal and
/// explicitly stepped simulation; no scene-specific exception is required.
fn physics_step_admitted(physics_time: Res<Time<Physics>>) -> bool {
    !physics_time.is_paused()
}

/// Forget an entity's report the moment its body dies, so a scene reload — or any
/// id reuse — reports afresh.
///
/// Entity ids are RECYCLED. A stale id left in the set silences a genuinely new
/// escape by a different body that happens to inherit the id, which is the same
/// unattributable-silence this module exists to remove.
///
/// This prunes on death rather than clearing on scene teardown. `lunco-physics`
/// deliberately depends on no `lunco-*` crate (see [`report_escaped_bodies`]) so
/// it cannot observe `ClearScene` — but it does not need to: despawning is what
/// teardown *does*, so the death signal covers scene reload, single-body despawn
/// and id reuse alike, with no coupling. Change-driven — empty in steady state.
fn forget_dead_bodies(
    mut dead: RemovedComponents<RigidBody>,
    mut reported: ResMut<ReportedEscapes>,
) {
    for e in dead.read() {
        reported.0.remove(&e);
    }
}

/// Installs the "left the world" diagnostic. Registered by [`PhysicsGatePlugin`]
/// — the diagnostic is only useful where physics actually runs.
///
/// [`PhysicsGatePlugin`]: crate::PhysicsGatePlugin
pub struct EscapeDiagnosticPlugin;

impl Plugin for EscapeDiagnosticPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldBounds>()
            .init_resource::<ReportedEscapes>()
            // The diagnostic raises the safety hold at the owning physics
            // boundary. Keep the plugin self-contained for headless apps and
            // focused integration tests that install it without the broader
            // PhysicsGatePlugin.
            .init_resource::<crate::PhysicsHolds>()
            // In `Writeback`, after the solver has moved bodies this tick, so a
            // body that left the world is reported on the tick it left rather
            // than one tick later. Bounds are refreshed first so newly-paged
            // terrain widens the world before anything is judged against it.
            //
            // `PhysicsStepSystems::Last`/`PhysicsSystems::Last` are the load-bearing
            // part, not decoration. `PhysicsSchedule` runs with
            // `ambiguity_detection: LogLevel::Error`, and the `PhysicsSystems` chain
            // is configured on `FixedPostUpdate` — NOT inside `PhysicsSchedule` — so
            // `in_set(Writeback)` alone leaves these systems unordered against every
            // solver system here and bevy PANICS at schedule init ("8 pairs of
            // systems with conflicting data access", on `Position`, `LinearVelocity`
            // and `ColliderAabb`). Pinning against `PhysicsStepSystems` is what
            // actually places them after the solver; the bridge's own writeback pass
            // resolves the identical problem the identical way.
            .add_systems(
                avian3d::schedule::PhysicsSchedule,
                // `forget_dead_bodies` first: a body despawned this tick must leave
                // the set before anything is judged, or a recycled id starts life
                // already-reported and its first real escape is silent.
                (
                    forget_dead_bodies,
                    update_world_bounds,
                    report_escaped_bodies,
                )
                    .chain()
                    .run_if(physics_step_admitted)
                    .in_set(PhysicsSystems::Writeback)
                    .after(avian3d::schedule::PhysicsStepSystems::Last)
                    .before(PhysicsSystems::Last),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PhysicsHolds;

    #[derive(Resource, Default)]
    struct SeenTelemetry(Vec<TelemetryEvent>);

    /// The orbital guarantee: with no static geometry the diagnostic has no
    /// opinion, so a body at heliocentric distance is NOT flagged. This is the
    /// property a naive `y < -100` bound would violate.
    #[test]
    fn no_static_geometry_means_no_opinion() {
        let b = WorldBounds::None;
        assert!(!b.escaped(Vector::new(1.5e11, -4.0e10, 0.0)));
        // …but NaN is always wrong, in every regime.
        assert!(b.escaped(Vector::new(Scalar::NAN, 0.0, 0.0)));
    }

    /// The motivating failure: `y = -1510` under a site-sized terrain must be
    /// caught. A 1 km terrain gives a 100 m margin (the floor dominates 10% of
    /// 1 km = 100 m here), so the floor sits at -100 and -1510 is well outside.
    #[test]
    fn catches_the_measured_failure_below_a_site_terrain() {
        let b = WorldBounds::Some {
            min: Vector::new(-600.0, -100.0, -600.0),
            max: Vector::new(600.0, 10_100.0, 600.0),
        };
        assert!(
            b.escaped(Vector::new(0.0, -1510.0, 0.0)),
            "y=-1510 must flag"
        );
        assert!(
            b.escaped(Vector::new(0.0, -737.0, 0.0)),
            "elev=-737 must flag"
        );
        // A body resting on the terrain is fine.
        assert!(!b.escaped(Vector::new(10.0, 0.5, -20.0)));
        // A body a metre under the surface — settling, not escaping — is fine.
        assert!(!b.escaped(Vector::new(10.0, -1.0, -20.0)));
    }

    /// A local-world ceiling is finite and derived from scene scale.
    #[test]
    fn a_local_world_has_a_finite_vertical_escape_bound() {
        let b = WorldBounds::Some {
            min: Vector::new(-600.0, -100.0, -600.0),
            max: Vector::new(600.0, 10_100.0, 600.0),
        };
        assert!(b.escaped(Vector::new(0.0, 1.0e9, 0.0)));
    }

    /// End-to-end wiring: a static ground collider must actually produce bounds,
    /// and a dynamic body dropped far below it must actually be reported.
    ///
    /// Without this the live-scene result ("no escape logged") would be vacuous —
    /// it would look identical to a diagnostic that never computed any bounds at
    /// all and therefore had no opinion about anything.
    #[test]
    fn static_ground_produces_bounds_and_a_body_below_it_is_reported() {
        use bevy::time::TimeUpdateStrategy;
        use core::time::Duration;

        let mut app = App::new();
        // AssetPlugin + Mesh: avian's collider cache reads `AssetEvent<Mesh>`
        // messages and panics on the first step if they were never initialised.
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            PhysicsPlugins::default(),
            EscapeDiagnosticPlugin,
        ));
        app.init_asset::<Mesh>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15625,
        )));
        app.finish();
        app.cleanup();

        // A 1 km square of static ground at the origin.
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(1000.0, 1.0, 1000.0),
            Transform::default(),
        ));
        // A dynamic body far beneath it — the shape of the measured failure.
        app.world_mut().spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Name::new("Escapee"),
            Transform::from_xyz(0.0, -1510.0, 0.0),
        ));

        for _ in 0..8 {
            app.update();
        }

        let bounds = *app.world().resource::<WorldBounds>();
        assert!(
            matches!(bounds, WorldBounds::Some { .. }),
            "static ground must define the world, got {bounds:?} — if this is `None` \
             the diagnostic is inert and reports nothing, which is indistinguishable \
             from a healthy scene"
        );
        if let WorldBounds::Some { max, .. } = bounds {
            assert!(
                max.y.is_finite(),
                "local static worlds need a finite ceiling"
            );
        }
        assert!(
            bounds.escaped(Vector::new(0.0, -1510.0, 0.0)),
            "a body 1.5 km below a 1 km ground plane must read as escaped: {bounds:?}"
        );
        assert!(
            app.world().resource::<ReportedEscapes>().0.len() == 1,
            "exactly one body should have been reported"
        );
    }

    /// A readiness pause is an admission boundary, not merely a zero-delta
    /// integration hint. Avian may enter `PhysicsSchedule` once with its prior
    /// delta after the pause edge; that transitional schedule must not inspect
    /// newly projected loading poses. Once the same clock is admitted again,
    /// the real escape is reported normally.
    #[test]
    fn paused_physics_does_not_judge_loading_pose_but_reports_after_admission() {
        use bevy::time::TimeUpdateStrategy;
        use core::time::Duration;

        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            TransformPlugin,
            PhysicsPlugins::default(),
            EscapeDiagnosticPlugin,
        ));
        app.init_asset::<Mesh>();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_micros(
            15625,
        )));
        app.finish();
        app.cleanup();

        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(1000.0, 1.0, 1000.0),
            Transform::default(),
        ));
        // Establish a real previous physics delta and the static-world bounds.
        for _ in 0..4 {
            app.update();
        }
        assert!(matches!(
            *app.world().resource::<WorldBounds>(),
            WorldBounds::Some { .. }
        ));

        app.world_mut().spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            Name::new("loading-pose"),
            Transform::from_xyz(0.0, -1510.0, 0.0),
        ));
        app.world_mut().resource_mut::<Time<Physics>>().pause();

        // This update exercises Avian's pause-edge schedule with the preceding
        // non-zero delta. The body is authored but has not been admitted.
        app.update();
        assert!(
            app.world().resource::<ReportedEscapes>().0.is_empty(),
            "a held loading pose must not become a terminal runtime fault"
        );
        assert!(!app
            .world()
            .resource::<PhysicsHolds>()
            .holds(PhysicsHolds::SAFETY_FAILURE));

        app.world_mut().resource_mut::<Time<Physics>>().unpause();
        app.update();
        assert_eq!(
            app.world().resource::<ReportedEscapes>().0.len(),
            1,
            "the same body must be diagnosed after an admitted solver step"
        );
    }

    /// A finite escape without an installed application policy fails closed.
    /// The production application supplies the required Rhai policy; this
    /// low-level boundary must not invent a Rust action when it is absent.
    #[test]
    fn a_missing_escape_policy_holds_physics_with_a_fault() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(WorldBounds::Some {
            min: Vector::splat(-100.0),
            max: Vector::splat(100.0),
        });
        world.insert_resource(ReportedEscapes::default());
        world.insert_resource(PhysicsHolds::default());
        world.insert_resource(lunco_core::RuntimeFaults::default());

        let position = Vector::new(0.0, -1510.0, 0.0);
        let velocity = Vector::new(0.0, -100.0, 0.0);
        let entity = world
            .spawn((
                RigidBody::Dynamic,
                Position(position),
                LinearVelocity(velocity),
                Name::new("escaped-body"),
            ))
            .id();

        world.run_system_once(report_escaped_bodies).unwrap();

        assert_eq!(world.get::<Position>(entity).unwrap().0, position);
        assert_eq!(world.get::<LinearVelocity>(entity).unwrap().0, velocity);
        assert!(world
            .resource::<PhysicsHolds>()
            .holds(PhysicsHolds::SAFETY_FAILURE));
        let fault = world.resource::<lunco_core::RuntimeFaults>();
        assert_eq!(
            fault.first.as_ref().map(|fault| fault.kind),
            Some("physics-escape-policy-failed")
        );
    }

    /// The escape boundary emits the same one-shot observation that the
    /// workbench status bridge consumes. The payload keeps the body identity,
    /// pose, bounds, and simulation time together so Recent Events is useful
    /// without scraping the logger.
    #[test]
    fn an_escape_emits_one_identified_telemetry_event() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(WorldBounds::Some {
            min: Vector::splat(-100.0),
            max: Vector::splat(100.0),
        });
        app.init_resource::<ReportedEscapes>();
        app.insert_resource(PhysicsHolds::default());
        app.insert_resource(lunco_core::RuntimeFaults::default());
        app.insert_resource(lunco_time::WorldTime {
            epoch_jd: 2_460_000.5,
            sim_secs: 12.5,
            met_secs: 12.5,
        });
        app.init_resource::<SeenTelemetry>();
        app.add_observer(
            |trigger: On<TelemetryEvent>, mut seen: ResMut<SeenTelemetry>| {
                if trigger.event().name == "physics-body-escaped" {
                    seen.0.push(trigger.event().clone());
                }
            },
        );
        app.add_systems(Update, report_escaped_bodies);

        let entity = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(Vector::new(0.0, -1510.0, 0.0)),
                LinearVelocity(Vector::new(0.0, -100.0, 0.0)),
                Name::new("escaped-body"),
                GlobalEntityId::from_raw(42),
            ))
            .id();

        app.update();
        app.update();

        let seen = &app.world().resource::<SeenTelemetry>().0;
        assert_eq!(seen.len(), 1, "the once-only escape must emit one event");
        let event = &seen[0];
        assert_eq!(event.source, 42);
        assert_eq!(event.timestamp, 2_460_000.5);
        assert_eq!(event.severity, Severity::Error);
        let TelemetryValue::String(detail) = &event.data else {
            panic!("escape telemetry should retain its diagnostic detail");
        };
        assert!(detail.contains("body=escaped-body"));
        assert!(detail.contains(&format!("entity_bits={}", entity.to_bits())));
        assert!(detail.contains("global_id=42"));
        assert!(detail.contains("sim_secs=12.500000"));
    }

    /// Once per entity, never per frame — the anti-spam contract.
    #[test]
    fn each_entity_is_reported_at_most_once() {
        let mut reported = ReportedEscapes::default();
        let e = Entity::from_raw_u32(7).unwrap();
        assert!(reported.0.insert(e), "first sighting reports");
        assert!(!reported.0.insert(e), "every later tick is silent");
    }

    /// Despawning a body must clear its report, or a recycled id inherits the
    /// silence and its first genuine escape is never logged.
    ///
    /// Drives the REAL system through a real despawn — an earlier version of this
    /// test called a private copy of the clear logic, so it stayed green while the
    /// production function was never registered at all.
    #[test]
    fn a_despawned_body_is_forgotten() {
        let mut app = App::new();
        app.init_resource::<ReportedEscapes>();
        app.add_systems(Update, forget_dead_bodies);

        let e = app.world_mut().spawn(RigidBody::Dynamic).id();
        app.world_mut()
            .resource_mut::<ReportedEscapes>()
            .0
            .insert(e);

        // No death yet: the report stands.
        app.update();
        assert!(
            app.world().resource::<ReportedEscapes>().0.contains(&e),
            "a live body must keep its once-only report"
        );

        app.world_mut().entity_mut(e).despawn();
        app.update();
        assert!(
            app.world().resource::<ReportedEscapes>().0.is_empty(),
            "a despawned body must be forgotten so a reused id reports afresh"
        );
    }
}
