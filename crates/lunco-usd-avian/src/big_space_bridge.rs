//! Phase 5 — avian ↔ big_space physics transform domain.
//!
//! Physics must not share `GlobalTransform` with the render world. Doc 45's
//! addendum (2026-07-11) identified avian's `propagate_before_physics` as the
//! third plain-f32 whole-tree `GlobalTransform` writer — it re-propagates the
//! entire hierarchy in absolute convention inside `PhysicsSchedule` on every
//! physics tick, unordered (and unorderable) against big_space's PostUpdate
//! high-precision pass. That is the measured 1-in-5–9 render strobe the
//! `touch_celestial_transforms` list papers over.
//!
//! This bridge turns ALL of avian's f32 transform sync off
//! (`propagate_before_physics`, `transform_to_position`,
//! `position_to_transform`) and owns the sync itself in the f64 cell-chain
//! domain (`world_pose_seeded`): render `GlobalTransform`s are big_space's
//! alone; physics `Position`/`Rotation` are fed from (and written back to)
//! `CellCoord` + `Transform` truth. The `Position` frame is the BigSpace root
//! frame — for site content under the site-anchored `WorldGrid` this is the
//! same small-magnitude frame avian solved in before, now with cell offsets
//! honoured (a body more than one cell from the site no longer collapses).
//!
//! ## Sync rules (per body, per physics tick)
//!
//! READ (`pose_to_position`, Prepare): a body's `Position`/`Rotation` are
//! recomputed from the cell chain ONLY when its own `(CellCoord, Transform)`
//! differs from the [`BridgeShadow`] copy taken at the bridge's last write —
//! i.e. when an EXTERNAL writer (spawn, teleport command, gizmo, USD
//! animation, anchor system, big_space recentring) touched it. A fired body
//! also re-reads every descendant body, so teleporting a chassis carries its
//! jointed wheels. Plain chain nodes (no body, no collider) carry no shadow;
//! their motion is probed via `Changed<Transform>`/`Changed<CellCoord>`
//! instead, so moving a group Xform re-reads the bodies beneath it too. Static bodies at rest are never touched — the previous
//! bridge dirtied every static's `Position` each tick, and the resulting
//! whole-world contact churn is what corrupted avian's island bookkeeping
//! (`islands/mod.rs:547` unwrap on a stale contact edge, reached from
//! `update_narrow_phase`).
//!
//! Standalone colliders (a `Collider` with no rigid-body ancestor, e.g. a
//! world-fixed sensor zone) previously got their `Position` from
//! `transform_to_position` too, so the READ pass covers them as well.
//! Body-attached child colliders keep avian's own `ColliderTransform` path
//! (`update_child_collider_position` — `Position`-based, unaffected).
//!
//! WRITEBACK (`position_to_pose`, Writeback): Dynamic bodies only — the
//! solver owns their `Position`. The world pose is converted to the parent
//! frame (nearest ancestor body's fresh `Position`, else the ancestor grid's
//! cell-chain pose) and written to `Transform` RELATIVE TO THE CURRENT CELL;
//! the cell itself is never written here — big_space's
//! `recenter_large_transforms` re-splits when the remainder exceeds the
//! grid's threshold, and the resulting external `(cell, Transform)` change
//! round-trips through the READ rule to an identical world pose. Jointed
//! sub-bodies without a `CellCoord` (rover wheels are plain `Transform`
//! children of the chassis) get their local transform relative to the
//! chassis' solved pose — the case avian's `position_to_transform` used to
//! handle via `GlobalTransform` math.

use avian3d::math::Vector;
use avian3d::physics_transform::{PhysicsTransformConfig, PhysicsTransformSystems, Position, Rotation};
use avian3d::prelude::*;
use avian3d::schedule::{PhysicsSchedule, PhysicsStepSystems, PhysicsSystems};
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::coords::world_pose_seeded;

/// The bridge's two passes, as orderable sets.
///
/// These exist because the bridge OWNS `Position` initialisation in this app —
/// avian's `transform_to_position` is switched off below, so
/// `PhysicsTransformSystems::TransformToPosition` is an empty set and ordering
/// against it is silently vacuous. Anything that must read a real `Position`
/// (the authored-joint seat in `build_usd_physics_joints`) has to say
/// `.after(PhysicsBridgeSystems::Read)` and mean it.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PhysicsBridgeSystems {
    /// READ: `(cell, Transform)` → `Position`/`Rotation`. After this has run,
    /// a body's `Position` is its authored world pose rather than
    /// `RigidBody`'s required-component default of zero.
    Read,
    /// WRITEBACK: solved `Position`/`Rotation` → `Transform`.
    Writeback,
}

/// Decouple avian from the f32 render transforms entirely; own the f64
/// `Position` ↔ (cell, `Transform`) sync.
pub struct BigSpacePhysicsBridgePlugin;

impl Plugin for BigSpacePhysicsBridgePlugin {
    fn build(&self, app: &mut App) {
        // Runtime-gated in avian (`run_if` on the resource), so overriding the
        // resource after `PhysicsPlugins` disables all three systems.
        // `transform_to_collider_scale` stays on: collider scale changes are
        // spawn-shaped and scale is big_space-preserved.
        app.insert_resource(PhysicsTransformConfig {
            propagate_before_physics: false,
            transform_to_position: false,
            position_to_transform: false,
            ..default()
        });
        // Every body (and standalone collider) carries the bridge's shadow
        // copy from spawn; the NaN sentinel makes the first READ always fire,
        // which is also what initialises `Position` (avian's own spawn init
        // lived inside the disabled `transform_to_position`).
        app.register_required_components::<RigidBody, BridgeShadow>();
        app.register_required_components::<Collider, BridgeShadow>();
        app.add_systems(
            PhysicsSchedule,
            pose_to_position
                .in_set(PhysicsBridgeSystems::Read)
                .in_set(PhysicsSystems::Prepare)
                // Before the physics STEP consumes Position/Rotation. Pinning
                // against PhysicsStepSystems::First (not PhysicsSystems::
                // StepSimulation) is what resolves the schedule-ambiguity
                // panic: the solver systems live in PhysicsStepSystems, a
                // parallel chain.
                .after(PhysicsSystems::First)
                .before(PhysicsStepSystems::First)
                .before(PhysicsTransformSystems::TransformToPosition),
        );
        app.add_systems(
            PhysicsSchedule,
            position_to_pose
                .in_set(PhysicsBridgeSystems::Writeback)
                .in_set(PhysicsSystems::Writeback)
                .after(PhysicsStepSystems::Last)
                .after(PhysicsTransformSystems::PositionToTransform)
                .before(PhysicsSystems::Last),
        );
        // Bridge-owned ColliderTransform propagation. avian's own
        // `propagate_collider_transforms` only descends from tree roots that
        // carry a `Transform` — with the canonical (Transform-free) BigSpace
        // root it is a silent no-op, and `ColliderTransform` (offset AND
        // scale — `update_collider_scale`'s child branch reads it) would
        // freeze at spawn values: measured 2026-07-11 as the 4000×-scaled
        // sandbox Ground collapsing to ~1 m. This system computes every
        // collider's transform directly from its `ColliderOf` chain instead,
        // no tree root involved. Same set as avian's pass (which no-ops).
        app.add_systems(
            FixedPostUpdate,
            propagate_collider_transforms_rootless.in_set(PhysicsTransformSystems::Propagate),
        );
    }
}

/// The bridge's copy of the `(CellCoord, Transform)` it last synced for this
/// entity. A mismatch on the READ pass means an external writer moved the
/// entity since — the one signal the bridge acts on. Default is a NaN
/// sentinel so a fresh spawn always mismatches.
#[derive(Component, Clone, Copy, Debug)]
pub struct BridgeShadow {
    cell: CellCoord,
    translation: Vec3,
    rotation: Quat,
}

impl Default for BridgeShadow {
    fn default() -> Self {
        Self {
            cell: CellCoord::ZERO,
            translation: Vec3::NAN,
            rotation: Quat::NAN,
        }
    }
}

impl BridgeShadow {
    fn matches(&self, cell: Option<&CellCoord>, tf: &Transform) -> bool {
        self.cell == cell.copied().unwrap_or_default()
            && self.translation == tf.translation
            && self.rotation == tf.rotation
    }

    fn capture(&mut self, cell: Option<&CellCoord>, tf: &Transform) {
        self.cell = cell.copied().unwrap_or_default();
        self.translation = tf.translation;
        self.rotation = tf.rotation;
    }

    /// Has [`pose_to_position`] written a real world pose for this entity yet?
    ///
    /// The bridge owns `Position` initialisation in this app (avian's own
    /// `transform_to_position` is switched OFF above), and the default shadow is
    /// the NaN sentinel that forces the first READ. "No longer NaN" is therefore
    /// exactly the signal that the READ pass has run at least once and `Position`
    /// holds the authored world pose — as opposed to `RigidBody`'s required-
    /// component default of `(0,0,0)`, which is present from the instant the body
    /// spawns and is indistinguishable from a real pose at the origin.
    ///
    /// This exists because consumers that seat against `Position` (the authored
    /// joint path in `build_usd_physics_joints`) have no other way to tell an
    /// uninitialised body from a placed one. `With<Position>` proves only that the
    /// body was admitted to the island graph, never that its pose is real.
    pub fn is_seeded(&self) -> bool {
        self.translation.is_finite() && !self.rotation.is_nan()
    }
}

/// Bodies and standalone colliders the bridge syncs. Child colliders of a
/// body (`ColliderOf` present, no own `RigidBody`) are excluded — avian's
/// `update_child_collider_position` derives their pose from the body.
type BridgeSynced = Or<(With<RigidBody>, Without<ColliderOf>)>;

/// READ: externally-moved `(cell, Transform)` → f64 `Position`/`Rotation`,
/// carrying the change to descendant bodies (chassis teleport moves wheels).
///
/// Order against this via [`PhysicsBridgeSystems::Read`], not by name — it is the
/// system that makes `Position` real, and anything seating against `Position`
/// before it has run reads zeros for every body.
#[allow(clippy::type_complexity)]
fn pose_to_position(
    mut commands: Commands,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    q_spatial: Query<(Option<&CellCoord>, &Transform)>,
    q_sleeping: Query<(), (With<Sleeping>, With<RigidBody>)>,
    // Plain chain nodes (no RigidBody, no Collider) carry no `BridgeShadow`,
    // so their motion is probed via change detection instead: a gizmo-dragged
    // or USD-animated group Xform must still re-read every descendant body.
    q_moved_plain: Query<
        Entity,
        (
            Or<(Changed<Transform>, Changed<CellCoord>)>,
            Without<RigidBody>,
            Without<Collider>,
        ),
    >,
    mut q_bodies: Query<
        (
            Entity,
            Option<&CellCoord>,
            &Transform,
            &mut Position,
            &mut Rotation,
            &mut BridgeShadow,
        ),
        BridgeSynced,
    >,
) {
    // Pass 1 (read-only): which entities did an external writer touch?
    let mut moved = EntityHashSet::default();
    for (e, cell, tf, _, _, shadow) in q_bodies.iter() {
        if !shadow.matches(cell, tf) {
            moved.insert(e);
        }
    }
    moved.extend(q_moved_plain.iter());
    if moved.is_empty() {
        return;
    }

    // Pass 2: re-read a body if it moved OR any ancestor moved (the ancestor's
    // new Transform is already in place, so the chain walk composes the
    // carried pose).
    for (e, cell, tf, mut pos, mut rot, mut shadow) in &mut q_bodies {
        let fired = moved.contains(&e) || {
            let mut cur = e;
            let mut hit = false;
            for _ in 0..32 {
                let Ok(co) = q_parents.get(cur) else { break };
                cur = co.parent();
                if moved.contains(&cur) {
                    hit = true;
                    break;
                }
            }
            hit
        };
        if !fired {
            continue;
        }
        let seed_cell = cell.copied().unwrap_or_default();
        let (p, r) = world_pose_seeded(e, &seed_cell, tf, &q_parents, &q_grids, &q_spatial);
        pos.0 = p;
        rot.0 = r;
        shadow.capture(cell, tf);
        // avian's `wake_on_changed` only sees Position writes made OUTSIDE
        // the physics schedule (it compares against `LastPhysicsTick`), so an
        // external Transform teleport applied here would leave a sleeping
        // body hovering. Removing `Sleeping` goes through avian's
        // `wake_on_remove_sleeping` hook — the sanctioned island wake path.
        if q_sleeping.contains(e) {
            commands.entity(e).remove::<Sleeping>();
        }
    }
}

/// WRITEBACK: solver f64 `Position`/`Rotation` → `Transform` relative to the
/// parent frame and the CURRENT cell, for Dynamic bodies. Cells are never
/// written; big_space's recentring owns the re-split.
#[allow(clippy::type_complexity)]
fn position_to_pose(
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    // Chain nodes that are not bodies or colliders (grids, plain group
    // nodes). Disjoint from `q_dyn`'s `&mut Transform` via the filters.
    q_plain: Query<(Option<&CellCoord>, &Transform), (Without<RigidBody>, Without<Collider>)>,
    q_poses: Query<(Entity, &Position, &Rotation), With<RigidBody>>,
    mut q_dyn: Query<(
        Entity,
        &Position,
        &Rotation,
        Option<&CellCoord>,
        &mut Transform,
        &mut BridgeShadow,
        &RigidBody,
    )>,
    // Scratch reused across ticks. This runs every physics tick over every body,
    // so building a fresh map per tick and a fresh chain Vec per BODY was a
    // steady-state allocation cost even when nothing moved. Both are cleared
    // where they were previously constructed — same contents, same order.
    mut body_poses: Local<EntityHashMap<(DVec3, DQuat)>>,
    mut chain: Local<Vec<(DVec3, Quat)>>,
) {
    // Pass A: solved world poses of every body — the parent frames for
    // jointed sub-bodies, fresher than any Transform this tick.
    body_poses.clear();
    for (e, p, r) in &q_poses {
        body_poses.insert(e, (p.0, r.0));
    }

    'bodies: for (e, pos, rot, cell, mut tf, mut shadow, rb) in &mut q_dyn {
        // Sync Position → Transform for every body avian moves via `Position`:
        // `Dynamic` (solver-integrated) AND `Kinematic` (externally seated — the
        // networked client pins replicated proxies `Kinematic` and drives their
        // `Position` in `drive_kinematic_proxies`; a host-side animated platform
        // is the same shape). Only `Static` is skipped — it never moves via
        // `Position`, so recomputing its Transform is pure churn. This replaces
        // avian's disabled `position_to_transform`, which likewise ran for all
        // non-static bodies; restricting to `Dynamic` froze every kinematic proxy
        // (Transform stuck at spawn) — visible only on a networked client, where
        // kinematic bodies exist.
        if matches!(rb, RigidBody::Static) {
            continue;
        }

        // Walk up from the direct parent to the nearest anchor: another body
        // (use its solved pose), a Grid (use its cell-chain pose), or the
        // root. Intermediate plain nodes accumulate bottom-up. An
        // inaccessible intermediate (a chain node the disjoint query cannot
        // see) skips the body — writing a pose composed against the wrong
        // frame is worse than leaving last tick's Transform.
        enum Anchor {
            Body(DVec3, DQuat),
            GridEntity(Entity),
            Root,
        }
        chain.clear();
        let mut anchor = Anchor::Root;
        let mut cur = e;
        for _ in 0..32 {
            let Ok(co) = q_parents.get(cur) else { break };
            let parent = co.parent();
            if let Some(&(bp, br)) = body_poses.get(&parent) {
                anchor = Anchor::Body(bp, br);
                break;
            }
            if q_grids.contains(parent) {
                anchor = Anchor::GridEntity(parent);
                break;
            }
            // Plain intermediate node: local offset in ITS parent's frame.
            let Ok((p_cell, p_tf)) = q_plain.get(parent) else { continue 'bodies };
            let edge = q_parents
                .get(parent)
                .ok()
                .and_then(|co2| q_grids.get(co2.parent()).ok())
                .map(|g| g.cell_edge_length() as f64);
            let cell_off = match (p_cell, edge) {
                (Some(c), Some(edge)) => {
                    DVec3::new(c.x as f64 * edge, c.y as f64 * edge, c.z as f64 * edge)
                }
                _ => DVec3::ZERO,
            };
            chain.push((cell_off + p_tf.translation.as_dvec3(), p_tf.rotation));
            cur = parent;
        }

        let (mut fp, mut fr) = match anchor {
            Anchor::Body(p, r) => (p, r),
            Anchor::GridEntity(g) => {
                let Ok((g_cell, g_tf)) = q_plain.get(g) else { continue };
                world_pose_seeded(
                    g,
                    &g_cell.copied().unwrap_or_default(),
                    g_tf,
                    &q_parents,
                    &q_grids,
                    &q_plain,
                )
            }
            Anchor::Root => (DVec3::ZERO, DQuat::IDENTITY),
        };
        // Compose accumulated intermediates top-down.
        for (off, local_rot) in chain.iter().rev() {
            fp += fr * *off;
            fr *= local_rot.as_dquat();
        }

        let inv = fr.inverse();
        let local = inv * (pos.0 - fp);
        let local_rot = (inv * rot.0).normalize().as_quat();

        // Subtract the current cell only when the direct parent is a Grid —
        // the same convention `world_pose` reads with.
        let direct_edge = q_parents
            .get(e)
            .ok()
            .and_then(|co| q_grids.get(co.parent()).ok())
            .map(|g| g.cell_edge_length() as f64);
        let rem = match (cell, direct_edge) {
            (Some(c), Some(edge)) => {
                local - DVec3::new(c.x as f64 * edge, c.y as f64 * edge, c.z as f64 * edge)
            }
            _ => local,
        };
        let new_t = rem.as_vec3();

        // Change-gate: a sleeping body recomputes to identical values — do
        // not dirty `Transform` (that churn is what big_space and the
        // renderer would pay for every tick).
        if tf.translation != new_t || tf.rotation != local_rot {
            tf.translation = new_t;
            tf.rotation = local_rot;
        }
        shadow.capture(cell, &tf);
    }
}

/// Recompute every collider's [`ColliderTransform`] from its entity path,
/// without needing a `Transform` on the tree root (avian's version does).
///
/// Semantics mirror avian's `propagate_collider_transforms` recursion exactly:
/// walking the path top-down, a plain node composes translation
/// (`transform_point`), rotation, and scale; a RIGID-BODY node resets
/// translation/rotation and keeps the running scale (the body defines the
/// collider frame; only ancestor scale survives into it). The tree root
/// contributes nothing when it has no `Transform` — identity, exactly what
/// the canonical BigSpace root is. Cell offsets are irrelevant here: nodes
/// between root and body only ever contribute SCALE, and cells do not scale.
///
/// Compare-gated writes: values derive from `Transform`s deterministically,
/// so an unchanged chain recomputes bit-identical and dirties nothing.
#[allow(clippy::type_complexity)]
fn propagate_collider_transforms_rootless(
    q_parents: Query<&ChildOf>,
    q_transforms: Query<&Transform>,
    q_rb: Query<(), With<RigidBody>>,
    mut q_colliders: Query<(Entity, &mut ColliderTransform)>,
    mut path: Local<Vec<Entity>>,
) {
    for (e, mut ct) in &mut q_colliders {
        // Path root → collider (inclusive).
        ancestor_path(e, &q_parents, &mut path);

        let mut acc = ColliderTransform::default();
        for &n in path.iter().rev() {
            let is_rb = q_rb.contains(n);
            match q_transforms.get(n) {
                Ok(tf) => {
                    let nt = ColliderTransform::from(*tf);
                    acc = if is_rb {
                        ColliderTransform {
                            translation: Vector::ZERO,
                            rotation: default(),
                            scale: acc.scale * nt.scale,
                        }
                    } else {
                        ColliderTransform {
                            translation: acc.transform_point(nt.translation),
                            rotation: Rotation(acc.rotation.0 * nt.rotation.0),
                            scale: acc.scale * nt.scale,
                        }
                    };
                }
                // No Transform (the canonical root): contributes identity,
                // but a body still resets the frame.
                Err(_) if is_rb => {
                    acc = ColliderTransform {
                        translation: Vector::ZERO,
                        rotation: default(),
                        scale: acc.scale,
                    };
                }
                Err(_) => {}
            }
        }
        if *ct != acc {
            *ct = acc;
        }
    }
}

/// Write `e`'s ancestor path into `path` as `[e, parent, …, root]` (walk capped
/// at 32). Caller-owned buffer: this runs per collider per tick, so the Vec is
/// reused rather than reallocated.
fn ancestor_path(e: Entity, q_parents: &Query<&ChildOf>, path: &mut Vec<Entity>) {
    path.clear();
    let mut cur = e;
    path.push(cur);
    for _ in 0..32 {
        match q_parents.get(cur) {
            Ok(co) => {
                cur = co.parent();
                path.push(cur);
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Round-trip proof of the bridge math at astronomical magnitude + with a
    //! rotating ancestor grid: `world_pose(body)` → world (pos, rot); the
    //! writeback conversion (world → parent-grid-local → cell remainder)
    //! must reproduce the original translation.
    use super::*;
    use bevy::ecs::system::SystemState;
    use lunco_core::coords::world_pose;

    #[test]
    fn world_pose_round_trips_through_cell_remainder() {
        let mut world = World::new();
        // Parent grid at a large heliocentric offset (cell (150_000_000, 0, 0)
        // on a 1 km edge ≈ 1.5e11 m — the 16 km f32 ULP regime), rotated 37°
        // about Y (a non-trivial ancestor rotation, the "spinning grid" case).
        let edge = 1_000.0_f32;
        let grid = Grid::new(edge, 0.0);
        let grid_cell = CellCoord::new(150_000_000, 0, 0);
        let grid_rot = Quat::from_rotation_y(0.6435); // ~37°
        let root_grid = world
            .spawn((Grid::new(edge, 0.0), CellCoord::ZERO, Transform::default(), GlobalTransform::default()))
            .id();
        let grid_e = world
            .spawn((grid, grid_cell, Transform::from_rotation(grid_rot), GlobalTransform::default(), ChildOf(root_grid)))
            .id();
        let b_cell = CellCoord::new(3, -1, 2);
        let b_tf = Transform::from_xyz(120.0, -40.0, 80.0)
            .with_rotation(Quat::from_rotation_y(0.1745));
        let body = world
            .spawn((b_cell, b_tf, GlobalTransform::default(), ChildOf(grid_e)))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_spatial) =
            state.get(&world).expect("read-only queries always validate");

        // READ direction: body world pose.
        let (p, r) = world_pose(body, &q_parents, &q_grids, &q_spatial).unwrap();
        assert!(p.length() > 1.0e11, "world pose {p:?} not at astronomical scale");

        // WRITEBACK direction: world → parent-grid-local → remainder against
        // the CURRENT cell (the bridge never rewrites the cell itself).
        let (gp, grot) = world_pose(grid_e, &q_parents, &q_grids, &q_spatial).unwrap();
        let inv = grot.inverse();
        let local = inv * (p - gp);
        let local_rot = inv * r;
        let e64 = edge as f64;
        let rem = local
            - DVec3::new(
                b_cell.x as f64 * e64,
                b_cell.y as f64 * e64,
                b_cell.z as f64 * e64,
            );

        assert!(
            (rem.as_vec3() - Vec3::new(120.0, -40.0, 80.0)).length() < 1e-2,
            "remainder {rem:?}"
        );
        let rot_err = local_rot.angle_between(b_tf.rotation.as_dquat()).abs();
        assert!(rot_err < 1e-4, "rotation error {rot_err}");
    }
}
