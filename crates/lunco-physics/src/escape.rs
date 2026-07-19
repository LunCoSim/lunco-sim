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
//! - **Upward: not at all — the ceiling is removed entirely.** A lander under
//!   thrust, a suborbital hop and a spacecraft on approach are all legitimately
//!   unbounded above the terrain, and there is no altitude at which "too high"
//!   is a defensible verdict for this engine. Escapes fall; they do not rise.
//!
//! Non-finite positions and velocities (NaN/±inf) are flagged unconditionally,
//! bounds or no bounds. A NaN in the solver is never legitimate in any regime,
//! and it is the one signal that needs no scene context at all.
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
//! One `Vector` comparison per dynamic body per tick, and a [`HashSet`] insert
//! on the first (and only) report per entity. The bounds themselves are
//! recomputed only when a static collider's AABB actually changes, which after
//! terrain settles is never. Nothing allocates per frame.

use avian3d::math::{Scalar, Vector};
use avian3d::prelude::*;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;

/// Fraction of the static world's largest extent added as slack on every side
/// except up. Ten percent is comfortably more than terrain-tile paging jitter
/// and far less than the scale of a genuine escape.
pub const ESCAPE_MARGIN_FRACTION: Scalar = 0.1;

/// Floor for the computed margin, in metres. Keeps a small scene (a unit-test
/// rig with a 2 m ground plane) from flagging a body that hopped a metre.
pub const ESCAPE_MARGIN_MIN: Scalar = 100.0;

/// The volume the simulation has static geometry in, expanded by the margins
/// described in the module docs — or [`WorldBounds::None`] when the scene has no
/// static colliders at all, in which case the diagnostic is inert.
///
/// `max.y` is deliberately `INFINITY`: see the module docs on why there is no
/// defensible ceiling for an engine that also does orbital work.
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

/// Entities already reported. Membership is the anti-spam rule: a body that has
/// left the world stays left, and re-logging it every tick would bury the very
/// first report — the one that names when it happened.
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
    q_removed: RemovedComponents<ColliderAabb>,
    q_static: Query<(&ColliderAabb, &ColliderOf)>,
    q_bodies: Query<&RigidBody>,
    mut bounds: ResMut<WorldBounds>,
) {
    let static_changed = q_changed
        .iter()
        .any(|collider_of| matches!(q_bodies.get(collider_of.body), Ok(RigidBody::Static)));
    if !static_changed && q_removed.is_empty() {
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
            // No ceiling: thrust and orbit are legitimately unbounded upward.
            max: Vector::new(max.x + margin, Scalar::INFINITY, max.z + margin),
        }
    } else {
        WorldBounds::None
    };

    if *bounds != next {
        *bounds = next;
    }
}

/// Log — once per entity, at `error!` — any dynamic body that has left
/// [`WorldBounds`], with everything needed to act on it without a re-run.
///
/// `Name` carries the USD prim path for USD-spawned bodies: the loader spawns
/// each prim with `Name::new(prim_path)` (`lunco-usd-bevy/src/lib.rs:1224`).
/// This crate deliberately does not depend on `lunco-usd` — it is substrate for
/// headless and wasm generators — so `Name` is both the reachable identifier and,
/// in practice, the prim path itself.
fn report_escaped_bodies(
    bounds: Res<WorldBounds>,
    mut reported: ResMut<ReportedEscapes>,
    q: Query<
        (
            Entity,
            &Position,
            &LinearVelocity,
            Option<&Name>,
            &RigidBody,
        ),
        With<RigidBody>,
    >,
) {
    for (entity, pos, vel, name, rb) in &q {
        // Static bodies do not move, and a kinematic body is wherever its driver
        // put it — neither can "escape", and flagging them would report the
        // driver's intent as an engine fault.
        if !matches!(rb, RigidBody::Dynamic) {
            continue;
        }
        if !bounds.escaped(pos.0) {
            continue;
        }
        // `insert` returns false if already present: the whole anti-spam rule.
        if !reported.0.insert(entity) {
            continue;
        }
        error!(
            "[physics] body left the world: {} ({entity}) at {:?}, velocity {:?} \
             — outside {:?}. A dynamic body outside the static geometry has nothing \
             left to collide with and will keep accelerating. Usual cause: physics \
             stepped while its collider was absent (see `PhysicsHolds`), or the body \
             was spawned below the terrain.",
            name.map(Name::as_str).unwrap_or("<unnamed>"),
            pos.0,
            vel.0,
            *bounds,
        );
    }
}

/// Drop despawned bodies from the once-per-entity report set, so a reloaded
/// scene can report the same failure again. Entity ids are recycled, and a stale
/// id in the set would silence a genuinely new escape.
pub fn clear_reported_escapes(
    mut removed: RemovedComponents<RigidBody>,
    mut reported: ResMut<ReportedEscapes>,
) {
    for entity in removed.read() {
        reported.0.remove(&entity);
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
                (update_world_bounds, clear_reported_escapes, report_escaped_bodies)
                    .chain()
                    .in_set(PhysicsSystems::Writeback)
                    .after(avian3d::schedule::PhysicsStepSystems::Last)
                    .before(PhysicsSystems::Last),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            max: Vector::new(600.0, Scalar::INFINITY, 600.0),
        };
        assert!(b.escaped(Vector::new(0.0, -1510.0, 0.0)), "y=-1510 must flag");
        assert!(b.escaped(Vector::new(0.0, -737.0, 0.0)), "elev=-737 must flag");
        // A body resting on the terrain is fine.
        assert!(!b.escaped(Vector::new(10.0, 0.5, -20.0)));
        // A body a metre under the surface — settling, not escaping — is fine.
        assert!(!b.escaped(Vector::new(10.0, -1.0, -20.0)));
    }

    /// There is no ceiling: a lander under thrust and a spacecraft on approach
    /// are legitimately unbounded above the terrain.
    #[test]
    fn altitude_is_never_an_escape() {
        let b = WorldBounds::Some {
            min: Vector::new(-600.0, -100.0, -600.0),
            max: Vector::new(600.0, Scalar::INFINITY, 600.0),
        };
        assert!(!b.escaped(Vector::new(0.0, 1.0e9, 0.0)));
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
        assert!(
            bounds.escaped(Vector::new(0.0, -1510.0, 0.0)),
            "a body 1.5 km below a 1 km ground plane must read as escaped: {bounds:?}"
        );
        assert!(
            app.world().resource::<ReportedEscapes>().0.len() == 1,
            "exactly one body should have been reported"
        );
    }

    /// Once per entity, never per frame — the anti-spam contract.
    #[test]
    fn each_entity_is_reported_at_most_once() {
        let mut reported = ReportedEscapes::default();
        let e = Entity::from_raw_u32(7).unwrap();
        assert!(reported.0.insert(e), "first sighting reports");
        assert!(!reported.0.insert(e), "every later tick is silent");
        reported.0.remove(&e);
        assert!(reported.0.insert(e), "a despawned body's id reports afresh");
    }
}
