//! Phase 5 — avian ↔ big_space physics transform domain.
//!
//! Physics must not share `GlobalTransform` with the render world. Doc 45's
//! addendum (2026-07-11) identified avian's `propagate_before_physics` as the
//! third plain-f32 whole-tree `GlobalTransform` writer — it re-propagates the
//! entire hierarchy in absolute convention inside `PhysicsSchedule` on every
//! physics tick, unordered (and unorderable) against big_space's PostUpdate
//! high-precision pass. The bridge removes that competing writer instead of
//! relying on per-frame dirtying to reconcile two transform conventions.
//!
//! This bridge turns ALL of avian's f32 transform sync off
//! (`propagate_before_physics`, `transform_to_position`,
//! `position_to_transform`) and owns the sync itself in the f64 cell-chain
//! domain (`grid_relative_pose` / `pose_in_grid`): render `GlobalTransform`s are big_space's
//! alone; physics `Position`/`Rotation` are fed from (and written back to)
//! `CellCoord` + `Transform` truth. The `Position` frame is the explicit
//! [`lunco_core::ActivePhysicsFrame`] selected for the loaded physical site.
//! Every Avian body and collider uses that one frame; sibling BigSpace branches
//! are converted through their nearest shared grid. A body-fixed surface frame
//! therefore keeps Avian local and stationary while the render hierarchy
//! follows the celestial body's rotation.
//!
//! ## Sync rules (per body, per physics tick)
//!
//! READ (`pose_to_position`, Prepare): a body's `Position`/`Rotation` are
//! recomputed from the cell chain ONLY when its own `(CellCoord, Transform)`
//! differs from the [`BridgeShadow`] copy taken at the bridge's last write —
//! i.e. when an EXTERNAL writer (spawn, teleport command, gizmo, USD
//! animation, anchor system) touched it. BigSpace recentring is identified by
//! reproducing its exact cell re-split from the previous representation, not by
//! guessing from Bevy change flags; a real cross-cell teleport therefore cannot
//! be mistaken for internal maintenance. A fired body
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
use avian3d::physics_transform::{
    PhysicsTransformConfig, PhysicsTransformSystems, Position, Rotation,
};
use avian3d::prelude::*;
use avian3d::schedule::{PhysicsSchedule, PhysicsStepSystems, PhysicsSystems};
use bevy::ecs::entity::{EntityHashMap, EntityHashSet};
use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::coords::{
    cell_local_remainder, compose_cell_local, grid_relative_pose_seeded, pose_in_grid,
    pose_in_grid_seeded, GridPos, GridRot,
};

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
        // Plain Grid/CellCoord chain nodes can carry physical descendants. Keep
        // the same exact previous-representation record on them so a BigSpace
        // re-split is distinguishable from a semantic ancestor teleport.
        app.register_required_components::<CellCoord, SpatialBridgeShadow>();
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
        // The world-readiness hold pauses Avian's inner PhysicsSchedule, but
        // preparation must still be able to seed poses while that hold is up:
        // authored joints are one of the things the hold is waiting for. This
        // is the same read pass, in the enclosing fixed schedule, before the
        // nested solver invocation. It writes Position/Rotation only; no
        // integration occurs here.
        app.add_systems(
            FixedPostUpdate,
            pose_to_position
                .in_set(PhysicsBridgeSystems::Read)
                .in_set(PhysicsSystems::Prepare)
                .before(PhysicsSystems::StepSimulation),
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
///
/// `translation` is the CELL-LOCAL render-frame copy (the raw `Transform`
/// value, not grid-absolute) — deliberately a bare `Vec3`, not a `GridPos`.
#[derive(Component, Clone, Copy, Debug)]
pub struct BridgeShadow {
    cell: CellCoord,
    translation: Vec3,
    rotation: Quat,
    physics_frame: Entity,
}

impl Default for BridgeShadow {
    fn default() -> Self {
        Self {
            cell: CellCoord::ZERO,
            translation: Vec3::NAN,
            rotation: Quat::NAN,
            physics_frame: Entity::PLACEHOLDER,
        }
    }
}

impl BridgeShadow {
    fn matches(&self, cell: Option<&CellCoord>, tf: &Transform, physics_frame: Entity) -> bool {
        self.cell == cell.copied().unwrap_or_default()
            && self.translation == tf.translation
            && self.rotation == tf.rotation
            && self.physics_frame == physics_frame
    }

    fn capture(&mut self, cell: Option<&CellCoord>, tf: &Transform, physics_frame: Entity) {
        self.cell = cell.copied().unwrap_or_default();
        self.translation = tf.translation;
        self.rotation = tf.rotation;
        self.physics_frame = physics_frame;
    }

    /// Whether the current representation is exactly what BigSpace's
    /// `CellCoord::recenter_large_transforms` produces from this shadow.
    ///
    /// This is a semantic/provenance test derived from the owning algorithm:
    /// merely observing that both components changed is insufficient because a
    /// legitimate cross-cell teleport changes the same pair.
    fn is_representation_only(
        &self,
        cell: Option<&CellCoord>,
        tf: &Transform,
        parent_grid: Option<&Grid>,
        physics_frame: Entity,
    ) -> bool {
        if !self.is_seeded() || self.physics_frame != physics_frame || self.rotation != tf.rotation
        {
            return false;
        }
        let (Some(cell), Some(grid)) = (cell, parent_grid) else {
            return false;
        };
        let old_position = grid.cell_to_float(&self.cell) + self.translation.as_dvec3();
        let new_position = grid.cell_to_float(cell) + tf.translation.as_dvec3();
        if old_position == new_position {
            return true;
        }

        // BigSpace computes the new f32 remainder through a dvec split. For
        // non-power-of-two cell edges the reconstructed DVec3 can differ by
        // the final f32 rounding even though the operation is its exact
        // canonical re-split. Validate that operation directly as the second
        // exact representation-only case; no distance epsilon is involved.
        let (delta, expected_translation) = grid.imprecise_translation_to_grid(self.translation);
        delta != CellCoord::ZERO
            && *cell == self.cell + delta
            && tf.translation == expected_translation
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

/// Previous raw BigSpace representation for a non-physical chain node.
///
/// A plain Grid/Xform can carry rigid bodies below it, so moving it is a
/// physical teleport. BigSpace also rewrites its `(CellCoord, Transform)` pair
/// when the local remainder crosses a cell. This shadow makes those two cases
/// structurally distinguishable using BigSpace's own re-split operation.
#[derive(Component, Clone, Copy, Debug)]
struct SpatialBridgeShadow {
    cell: CellCoord,
    translation: Vec3,
    rotation: Quat,
    parent: Entity,
}

impl Default for SpatialBridgeShadow {
    fn default() -> Self {
        Self {
            cell: CellCoord::ZERO,
            translation: Vec3::NAN,
            rotation: Quat::NAN,
            parent: Entity::PLACEHOLDER,
        }
    }
}

impl SpatialBridgeShadow {
    fn is_seeded(&self) -> bool {
        self.translation.is_finite() && self.rotation.is_finite()
    }

    fn is_representation_only(
        &self,
        cell: &CellCoord,
        tf: &Transform,
        parent: Entity,
        grid: &Grid,
    ) -> bool {
        if !self.is_seeded() || self.parent != parent || self.rotation != tf.rotation {
            return false;
        }
        let old_position = grid.cell_to_float(&self.cell) + self.translation.as_dvec3();
        let new_position = grid.cell_to_float(cell) + tf.translation.as_dvec3();
        if old_position == new_position {
            return true;
        }
        let (delta, expected_translation) = grid.imprecise_translation_to_grid(self.translation);
        delta != CellCoord::ZERO
            && *cell == self.cell + delta
            && tf.translation == expected_translation
    }

    fn capture(&mut self, cell: &CellCoord, tf: &Transform, parent: Entity) {
        self.cell = *cell;
        self.translation = tf.translation;
        self.rotation = tf.rotation;
        self.parent = parent;
    }
}

/// Bodies and standalone colliders the bridge syncs. Child colliders of a
/// body (`ColliderOf` present, no own `RigidBody`) are excluded — avian's
/// `update_child_collider_position` derives their pose from the body.
type BridgeSynced = Or<(With<RigidBody>, Without<ColliderOf>)>;

/// Return whether a moved ancestor is a physical input for `body`.
///
/// Bodies below the active frame are solved in that frame, so transforms above
/// it are only the celestial render representation and must not be copied into
/// Avian. Bodies outside that branch (for example a planet picking collider)
/// must follow every changed ancestor when their pose is projected into the
/// active frame.
fn moved_in_active_frame(
    body: Entity,
    moved: Entity,
    active_frame: Entity,
    q_parents: &Query<&ChildOf>,
) -> bool {
    let mut current = body;
    let mut reached_active_frame = false;
    for _ in 0..32 {
        let Ok(child_of) = q_parents.get(current) else {
            return false;
        };
        current = child_of.parent();
        if current == active_frame {
            reached_active_frame = true;
            // The active frame itself is part of the render representation.
            // Its transform must never be copied into descendants' physics
            // poses, even when it is the changed ancestor being inspected.
            if current == moved {
                return false;
            }
            continue;
        }
        if current == moved {
            return !reached_active_frame;
        }
    }
    false
}

fn is_below_active_frame(
    entity: Entity,
    active_frame: Entity,
    q_parents: &Query<&ChildOf>,
) -> bool {
    let mut current = entity;
    for _ in 0..32 {
        if current == active_frame {
            return true;
        }
        let Ok(child_of) = q_parents.get(current) else {
            return false;
        };
        current = child_of.parent();
    }
    false
}

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
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_sleeping: Query<(), (With<Sleeping>, With<RigidBody>)>,
    // Plain chain nodes have a representation shadow when they carry a
    // CellCoord. Transform-only nodes cannot be recentered and every change is
    // semantic. Either kind can carry physical descendants.
    mut q_moved_plain: Query<
        (
            Entity,
            Option<&CellCoord>,
            &Transform,
            Option<&mut SpatialBridgeShadow>,
            Option<&ChildOf>,
        ),
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
            Option<&lunco_core::PhysicsPoseAuthoritative>,
        ),
        BridgeSynced,
    >,
) {
    let active_frame = active_frame.0;
    // Pass 1 (read-only): which entities did an external writer touch?
    let mut moved = EntityHashSet::default();
    for (e, cell, tf, _, _, shadow, pose_override) in q_bodies.iter() {
        if pose_override.is_some() {
            continue;
        }
        let parent_grid = q_parents
            .get(e)
            .ok()
            .and_then(|parent| q_grids.get(parent.parent()).ok());
        let internal_rebranch = shadow.is_representation_only(cell, tf, parent_grid, active_frame);
        let mismatch = !shadow.matches(cell, tf, active_frame);
        if !internal_rebranch && mismatch {
            moved.insert(e);
        }
    }
    for (entity, cell, tf, shadow, child_of) in &mut q_moved_plain {
        let representation_only = match (cell, shadow, child_of) {
            (Some(cell), Some(mut shadow), Some(child_of)) => {
                let parent = child_of.parent();
                let representation_only = q_grids
                    .get(parent)
                    .is_ok_and(|grid| shadow.is_representation_only(cell, tf, parent, grid));
                shadow.capture(cell, tf, parent);
                representation_only
            }
            _ => false,
        };
        if !representation_only {
            moved.insert(entity);
        }
    }
    if moved.is_empty() {
        return;
    }

    // Pass 2: re-read a body if it moved OR any ancestor moved (the ancestor's
    // new Transform is already in place, so the chain walk composes the
    // carried pose).
    for (e, cell, tf, mut pos, mut rot, mut shadow, pose_override) in &mut q_bodies {
        if pose_override.is_some() {
            continue;
        }
        let parent_grid = q_parents
            .get(e)
            .ok()
            .and_then(|parent| q_grids.get(parent.parent()).ok());
        // BigSpace has re-split the same pose into a new cell/local pair.
        // Refresh only the representation shadow; Avian remains unchanged.
        if shadow.is_representation_only(cell, tf, parent_grid, active_frame) {
            shadow.capture(cell, tf, active_frame);
            continue;
        }
        let direct_move = moved.contains(&e);
        let ancestor_move = {
            let mut cur = e;
            let mut hit = false;
            for _ in 0..32 {
                let Ok(co) = q_parents.get(cur) else { break };
                cur = co.parent();
                if moved.contains(&cur) && moved_in_active_frame(e, cur, active_frame, &q_parents) {
                    hit = true;
                    break;
                }
            }
            hit
        };
        let fired = direct_move || ancestor_move;
        if !fired {
            continue;
        }
        // Typed until the component write: the cell chain composes a
        // grid-absolute pose, and avian's `Position`/`Rotation` carry exactly
        // that frame — `.0` at the write IS the frame assertion.
        let seed_cell = cell.copied().unwrap_or_default();
        let (position, rotation) = if is_below_active_frame(e, active_frame, &q_parents) {
            grid_relative_pose_seeded(
                e,
                active_frame,
                &seed_cell,
                tf,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
            .expect("active PhysicsFrame must be an ancestor of every physical body")
        } else {
            pose_in_grid_seeded(
                e,
                active_frame,
                &seed_cell,
                tf,
                &q_parents,
                &q_grids,
                &q_spatial,
            )
            .expect("active PhysicsFrame must share a BigSpace root with every collider")
        };
        let (p, r) = (GridPos(position), GridRot(rotation));
        pos.0 = p.0;
        rot.0 = r.0;
        shadow.capture(cell, tf, active_frame);
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
///
/// This is the ONE sanctioned grid→render `Transform` writer: everything else
/// that wants a solved pose on screen goes through it, never by writing
/// `Transform` from a grid value directly (the `interpolate_all` law — a
/// second writer fights avian's interpolation and big_space's propagation).
#[allow(clippy::type_complexity)]
fn position_to_pose(
    mut commands: Commands,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&Grid>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    // Chain nodes that are not bodies or colliders (grids, plain group
    // nodes). Disjoint from `q_dyn`'s `&mut Transform` via the filters.
    q_plain: Query<(Option<&CellCoord>, &Transform), (Without<RigidBody>, Without<Collider>)>,
    q_poses: Query<(Entity, Ref<Position>, Ref<Rotation>), With<RigidBody>>,
    mut removed_bodies: RemovedComponents<RigidBody>,
    mut q_dyn: Query<(
        Entity,
        &Position,
        &Rotation,
        Option<&CellCoord>,
        &mut Transform,
        &mut BridgeShadow,
        &RigidBody,
        Option<&lunco_core::PhysicsPoseAuthoritative>,
    )>,
    // PERSISTENT across ticks, not just scratch: entries for bodies the solver
    // did not touch this tick (sleeping, settled statics, idle kinematics) are
    // carried over from the last tick rather than rebuilt — their `Position`
    // is unchanged by definition, so the retained entry IS the current pose.
    // Rebuilding the whole map over ALL rigid bodies every physics tick (even
    // all-sleeping) was a steady-state O(bodies) cost. Despawned bodies are
    // pruned via `RemovedComponents`, so recycled entity ids never inherit a
    // stale pose.
    mut body_poses: Local<EntityHashMap<(GridPos, GridRot)>>,
    // NOTE: `chain` stays bare — its entries are parent-frame OFFSETS paired
    // with LOCAL rotations, not grid-absolute points, so typing them `GridPos`
    // would assert a frame they are not in.
    mut chain: Local<Vec<(DVec3, Quat)>>,
) {
    let active_frame = active_frame.0;
    // Pass A: solved world poses — the parent frames for jointed sub-bodies,
    // fresher than any Transform this tick. avian's `Position` carries the
    // grid-absolute frame; wrap at the read.
    //
    // Only bodies whose `Position`/`Rotation` actually changed since this
    // system's last run are (re)written: the solver writes every awake body
    // each step and skips `Sleeping` ones, so "changed" IS avian's activity
    // state — sleeping bodies keep their retained entry (which a non-sleeping
    // jointed descendant may still demand as its anchor), and settled statics
    // cost nothing. A body with no entry yet (first run, spawned mid-flight)
    // is inserted unconditionally.
    for e in removed_bodies.read() {
        body_poses.remove(&e);
    }
    for (e, p, r) in &q_poses {
        if (p.is_changed() || r.is_changed()) || !body_poses.contains_key(&e) {
            body_poses.insert(e, (GridPos(p.0), GridRot(r.0)));
        }
    }

    'bodies: for (e, pos, rot, cell, mut tf, mut shadow, rb, pose_override) in &mut q_dyn {
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
            Body(GridPos, GridRot),
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
            let Ok((p_cell, p_tf)) = q_plain.get(parent) else {
                continue 'bodies;
            };
            let edge = q_parents
                .get(parent)
                .ok()
                .and_then(|co2| q_grids.get(co2.parent()).ok())
                .map(|g| g.cell_edge_length() as f64);
            // `.0`: the composed point re-enters the mixed-frame chain as a
            // parent-frame offset (see the `chain` note above).
            chain.push((
                compose_cell_local(p_cell, edge, p_tf.translation).0,
                p_tf.rotation,
            ));
            cur = parent;
        }

        let (mut fp, mut fr) = match anchor {
            Anchor::Body(p, r) => (p, r),
            Anchor::GridEntity(g) => {
                let Some((p, r)) = pose_in_grid(g, active_frame, &q_parents, &q_grids, &q_plain)
                else {
                    continue;
                };
                (GridPos(p), GridRot(r))
            }
            Anchor::Root => (GridPos(DVec3::ZERO), GridRot(DQuat::IDENTITY)),
        };
        // Compose accumulated intermediates top-down.
        for (off, local_rot) in chain.iter().rev() {
            fp = fp + fr.0 * *off;
            fr.0 *= local_rot.as_dquat();
        }

        // avian's solved `Position` is a grid-absolute point; wrap at the
        // read, then `GridPos - GridPos` yields the frame-free lever arm the
        // parent-frame rotation is applied to.
        let solved = GridPos(pos.0);
        let inv = fr.0.inverse();
        // `local` is a point in the parent GRID's frame (grid-absolute in
        // that grid), ready for the cell split below.
        let local = GridPos(inv * (solved - fp));
        let local_rot = (inv * rot.0).normalize().as_quat();

        // Subtract the current cell only when the direct parent is a Grid —
        // the same convention `world_pose` reads with.
        let direct_edge = q_parents
            .get(e)
            .ok()
            .and_then(|co| q_grids.get(co.parent()).ok())
            .map(|g| g.cell_edge_length() as f64);
        let rem = cell_local_remainder(local, cell, direct_edge);
        // The `Transform` write below stays raw: `rem` leaves the grid frame
        // here and becomes render currency (cell-local f32).
        let new_t = rem.as_vec3();

        // Change-gate: a sleeping body recomputes to identical values — do
        // not dirty `Transform` (that churn is what big_space and the
        // renderer would pay for every tick).
        if tf.translation != new_t || tf.rotation != local_rot {
            tf.translation = new_t;
            tf.rotation = local_rot;
        }
        shadow.capture(cell, &tf, active_frame);
        if pose_override.is_some() {
            commands
                .entity(e)
                .remove::<lunco_core::PhysicsPoseAuthoritative>();
        }
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
/// Change-gated at the WALK, not just the write: the composition below is a
/// pure function of the ancestor chain's `Transform`s, the chain topology
/// (`ChildOf`), and which nodes are bodies. Recomputing every collider's full
/// chain every `FixedPostUpdate` with only the final write compare-gated was
/// O(colliders × depth) even when nothing moved — a static terrain scene paid
/// the whole walk per tick for nothing. Now a collider recomputes only when it
/// is new (`is_added`) or some node on its path changed; component REMOVALS
/// (a `Transform` or `RigidBody` taken off a chain node) are not per-entity
/// attributable, so any removal marks everything dirty for one tick — removal
/// is spawn-shaped, so that full pass is rare.
///
/// The final write stays compare-gated too: values derive from `Transform`s
/// deterministically, so an unchanged chain recomputes bit-identical and
/// dirties nothing.
#[allow(clippy::type_complexity)]
fn propagate_collider_transforms_rootless(
    q_parents: Query<&ChildOf>,
    q_transforms: Query<&Transform>,
    q_rb: Query<(), With<RigidBody>>,
    // Nodes whose contribution to some chain may have changed since this
    // system's last run. `Changed<Transform>` covers motion (incl. the
    // writeback pass, which runs earlier in the same tick), `Changed<ChildOf>`
    // covers reparenting, `Added<RigidBody>` covers a node becoming a
    // frame-resetting body.
    q_dirty_nodes: Query<Entity, Or<(Changed<Transform>, Changed<ChildOf>, Added<RigidBody>)>>,
    mut removed_tf: RemovedComponents<Transform>,
    mut removed_rb: RemovedComponents<RigidBody>,
    mut q_colliders: Query<(Entity, &mut ColliderTransform)>,
    mut path: Local<Vec<Entity>>,
    mut dirty: Local<EntityHashSet>,
) {
    // DRAIN, don't peek (see `escape.rs`): un-drained removal events re-fire
    // for their whole retention window.
    let all_dirty = removed_tf.read().count() > 0 || removed_rb.read().count() > 0;
    dirty.clear();
    if !all_dirty {
        dirty.extend(q_dirty_nodes.iter());
    }

    for (e, mut ct) in &mut q_colliders {
        // Path root → collider (inclusive) — built only when it can matter.
        let recompute = if all_dirty || ct.is_added() {
            ancestor_path(e, &q_parents, &mut path);
            true
        } else if dirty.is_empty() {
            false
        } else {
            ancestor_path(e, &q_parents, &mut path);
            path.iter().any(|n| dirty.contains(n))
        };
        if !recompute {
            continue;
        }

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
            .spawn((
                Grid::new(edge, 0.0),
                CellCoord::ZERO,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let grid_e = world
            .spawn((
                grid,
                grid_cell,
                Transform::from_rotation(grid_rot),
                GlobalTransform::default(),
                ChildOf(root_grid),
            ))
            .id();
        let b_cell = CellCoord::new(3, -1, 2);
        let b_tf =
            Transform::from_xyz(120.0, -40.0, 80.0).with_rotation(Quat::from_rotation_y(0.1745));
        let body = world
            .spawn((b_cell, b_tf, GlobalTransform::default(), ChildOf(grid_e)))
            .id();

        let mut state: SystemState<(
            Query<&ChildOf>,
            Query<&Grid>,
            Query<(Option<&CellCoord>, &Transform)>,
        )> = SystemState::new(&mut world);
        let (q_parents, q_grids, q_spatial) = state
            .get(&world)
            .expect("read-only queries always validate");

        // READ direction: body world pose (typed grid-absolute).
        let (p, r) = world_pose(body, &q_parents, &q_grids, &q_spatial).unwrap();
        assert!(
            p.0.length() > 1.0e11,
            "world pose {p:?} not at astronomical scale"
        );

        // WRITEBACK direction: world → parent-grid-local → remainder against
        // the CURRENT cell (the bridge never rewrites the cell itself) —
        // the same `cell_local_remainder` split the live writeback uses.
        let (gp, grot) = world_pose(grid_e, &q_parents, &q_grids, &q_spatial).unwrap();
        let inv = grot.0.inverse();
        let local = GridPos(inv * (p - gp));
        let local_rot = inv * r.0;
        let e64 = edge as f64;
        let rem = cell_local_remainder(local, Some(&b_cell), Some(e64));

        assert!(
            (rem.as_vec3() - Vec3::new(120.0, -40.0, 80.0)).length() < 1e-2,
            "remainder {rem:?}"
        );
        let rot_err = local_rot.angle_between(b_tf.rotation.as_dquat()).abs();
        assert!(rot_err < 1e-4, "rotation error {rot_err}");
    }
}
