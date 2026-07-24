//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` which **automatically applies transforms** to
//! entities with `GizmoTarget`. This module handles:
//! - Making bodies kinematic during gizmo drag
//! - Freezing the Floating Origin to break feedback loops with camera follow
//! - Disabling physics interpolation during manual dragging
//! - Restoring dynamic bodies and origin tracking when drag ends
//!
//! **Architectural Note**: This module provides the "Golden Path" for
//! high-precision manual editing. It ensures the coordinate system
//! remains stable by temporarily pausing origin re-centering.

use avian3d::prelude::{
    LinearVelocity, RigidBody, RotationInterpolation, TranslationInterpolation,
};
use bevy::camera::RenderTarget;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::FloatingOrigin;
use transform_gizmo_bevy::{GizmoCamera, GizmoDragStarted, GizmoDragging, GizmoTarget};

/// Tracks the previous parent-local position and metadata for drag lifecycle.
#[derive(Component)]
pub struct GizmoPrevPos {
    /// Parent-local position in the previous frame (meters).
    pub local_pos: DVec3,
    /// Grid-ABSOLUTE position in the previous frame (meters).
    ///
    /// The drag velocity MUST be differenced from this, never from `local_pos`:
    /// `Transform.translation` is the CELL REMAINDER, and big_space re-splits a
    /// grid-direct entity's cell whenever it crosses a boundary. Freezing
    /// `FloatingOrigin` stops the CAMERA from shifting the world; it does not stop
    /// the dragged entity's own re-binning. So the remainder jumps a full
    /// `cell_edge` (2 km) in one frame while the object barely moved, and Δ/dt
    /// handed the solver ~1.2e5 m/s — which blew up every joint-coupled body into
    /// NaN and surfaced as an `origin.is_finite()` panic inside avian's raycast.
    pub abs_pos: DVec3,
    /// Original RigidBody type before drag started, or `None` if the entity had
    /// no `RigidBody` at all. `None` must stay `None` on restore: inserting a
    /// `Dynamic` body onto a prim that never had one gives avian a body with no
    /// mass or inertia ("Dynamic rigid body has no mass or inertia. This can
    /// cause NaN values.") and hands the solver a NaN source that outlives the
    /// drag.
    pub original_body: Option<RigidBody>,
    /// Whether the entity had TranslationInterpolation.
    pub had_translation_interpolation: bool,
    /// Whether the entity had RotationInterpolation.
    pub had_rotation_interpolation: bool,
}

/// Marks an entity as selected for gizmo editing. The `GizmoTarget` the gizmo
/// crate actually drives lives on a [`GizmoProxy`], never here — see
/// [`spawn_gizmo_proxies`].
#[derive(Component)]
pub struct GizmoSelected;

/// A render-frame stand-in that carries the `GizmoTarget` for one real entity.
///
/// **Why.** `transform-gizmo-bevy::update_gizmos` builds its view matrix from the
/// camera's `GlobalTransform` (render frame) but reads each target's pose from
/// `&Transform` (`lib.rs:496`/`:521`). Those coincide only in a world without
/// big_space. In the moonbase twin they differ by `(cell - origin_cell) *
/// cell_edge`: measured at exactly 1999.9985 m — the rover at cell.y=0/local
/// y=1946.5, the camera's floating origin at cell.y=1. The handles were drawn a
/// whole cell off-screen. The selection AABB reads `GlobalTransform`, so it kept
/// landing correctly and made this look gizmo-specific. It works in the sandbox
/// scene only because everything there sits in the origin cell, where the two
/// frames are the same.
///
/// **Why a proxy rather than swapping the real entity into the render frame.**
/// Any system that mutates the real `Transform` conflicts with `update_gizmos`
/// (`&mut Transform`), which is a private fn — so Bevy orders them arbitrarily
/// and there is nothing to `.before()`. The swap is unorderable by construction,
/// and when it loses the race a render-frame value propagates and teleports the
/// object a cell away. A proxy removes the conflict: the crate only ever touches
/// the proxy, we only ever touch the real entity, and a failure can't move
/// anything because the real `Transform` is written only while a drag is active.
///
/// The proxy is unparented and has no `CellCoord`, so its `Transform` *is* its
/// render-frame pose — exactly the frame the gizmo assumes.
#[derive(Component)]
pub struct GizmoProxy {
    /// The real entity this proxy edits.
    pub target: Entity,
    /// Proxy pose at the last sync, to diff the gizmo's edit against.
    last_translation: Vec3,
    /// Proxy rotation at the last sync.
    last_rotation: Quat,
    /// The target parent's render-frame rotation, to map deltas back to local.
    parent_rotation: Quat,
}

/// Back-reference so a selection can't spawn two proxies.
#[derive(Component)]
pub struct HasGizmoProxy {
    /// The proxy entity.
    pub proxy: Entity,
}

/// Spawns a [`GizmoProxy`] for each newly selected entity.
pub fn spawn_gizmo_proxies(
    q_new: Query<(Entity, &GlobalTransform), (With<GizmoSelected>, Without<HasGizmoProxy>)>,
    mut commands: Commands,
) {
    for (target, global) in &q_new {
        let (_, rotation, translation) = global.to_scale_rotation_translation();
        let proxy = commands
            .spawn((
                Name::new("GizmoProxy"),
                Transform::from_translation(translation).with_rotation(rotation),
                GlobalTransform::default(),
                GizmoTarget::default(),
                GizmoProxy {
                    target,
                    last_translation: translation,
                    last_rotation: rotation,
                    parent_rotation: Quat::IDENTITY,
                },
            ))
            .id();
        commands.entity(target).try_insert(HasGizmoProxy { proxy });
    }
}

/// Despawns proxies whose target was deselected or despawned.
pub fn despawn_gizmo_proxies(
    q_proxies: Query<(Entity, &GizmoProxy)>,
    q_selected: Query<(), With<GizmoSelected>>,
    mut commands: Commands,
) {
    for (proxy, link) in &q_proxies {
        if q_selected.get(link.target).is_err() {
            commands.entity(proxy).despawn();
            commands.entity(link.target).try_remove::<HasGizmoProxy>();
        }
    }
}

/// Parks each idle proxy on its target's render-frame pose, so the gizmo draws on
/// the object. Skipped while dragging — then the gizmo owns the proxy.
///
/// Runs after `TransformSystems::Propagate` (big_space's propagation is in that
/// set), so the `GlobalTransform` read here is this frame's.
pub fn sync_gizmo_proxies(
    mut q_proxies: Query<(&mut Transform, &mut GizmoProxy, &GizmoTarget)>,
    q_targets: Query<(&GlobalTransform, &Transform), Without<GizmoProxy>>,
) {
    for (mut tf, mut link, gizmo_target) in &mut q_proxies {
        if gizmo_target.is_active() {
            continue;
        }
        let Ok((global, local)) = q_targets.get(link.target) else {
            continue;
        };
        let (_, rotation, translation) = global.to_scale_rotation_translation();
        tf.translation = translation;
        tf.rotation = rotation;
        link.last_translation = translation;
        link.last_rotation = rotation;
        // Recovered from the pair, so a target parented under a rotated prim maps
        // its deltas back correctly. Identity for a grid-direct entity.
        link.parent_rotation = rotation * local.rotation.inverse();
    }
}

/// Transfers a drag from the proxy onto the real entity as a **delta**.
///
/// A translation delta is frame-invariant (up to the parent's rotation), so this
/// never converts an absolute pose between the render frame and the grid — the
/// mistake that produces unbounded cell-drift when a driver writes a render-frame
/// value into a cell-local field and big_space re-bins it every frame.
///
/// Runs in `First`: strictly after the gizmo crate's `Last`, with no ambiguity to
/// lose.
pub fn apply_gizmo_proxy_drag(
    mut q_proxies: Query<(&Transform, &mut GizmoProxy, &GizmoTarget)>,
    mut q_targets: Query<&mut Transform, Without<GizmoProxy>>,
) {
    for (tf, mut link, gizmo_target) in &mut q_proxies {
        if !gizmo_target.is_active() {
            continue;
        }
        let d_translation = tf.translation - link.last_translation;
        let d_rotation = link.last_rotation.inverse() * tf.rotation;
        if d_translation.length_squared() < 1e-12 && d_rotation.is_near_identity() {
            continue;
        }
        let inv_parent = link.parent_rotation.inverse();
        if let Ok(mut target_tf) = q_targets.get_mut(link.target) {
            target_tf.translation += inv_parent * d_translation;
            target_tf.rotation = (inv_parent * tf.rotation).normalize();
        }
        link.last_translation = tf.translation;
        link.last_rotation = tf.rotation;
    }
}

/// Mirrors each `GizmoTarget`'s active state onto the core
/// [`lunco_core::GizmoDragging`] marker, so render/sim crates (e.g. the avatar
/// camera-follow systems) can react to a drag **without** depending on
/// `transform-gizmo-bevy`. This is the only place the marker is written.
pub fn sync_gizmo_dragging_marker(
    mut commands: Commands,
    q: Query<(&GizmoProxy, &GizmoTarget)>,
    mut drag_mode: ResMut<lunco_core::DragModeActive>,
) {
    let mut any_active = false;
    for (link, gt) in &q {
        let e = link.target;
        if gt.is_active() {
            any_active = true;
            // `try_*`: a `GizmoTarget` entity can be despawned (scene reset,
            // deselect-then-despawn) between this query read and command apply.
            // The plain `insert`/`remove` then error on the dead entity; the
            // fallible variants no-op instead.
            commands.entity(e).try_insert(lunco_core::GizmoDragging);
        } else {
            commands.entity(e).try_remove::<lunco_core::GizmoDragging>();
        }
    }
    // Single writer of `DragModeActive`: possession (plain-click) is blocked ONLY
    // while a gizmo handle is actively dragged — not merely because something is
    // selected. So Shift-selecting an object just highlights it; you can still
    // plain-click to possess a rover.
    drag_mode.active = any_active;
}

/// Makes the selected entity kinematic and freezes the coordinate system when gizmo drag starts.
pub fn capture_gizmo_start(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    q_rigid_bodies: Query<&RigidBody>,
    q_prev_pos: Query<&GizmoPrevPos>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    // The cell chain, for the grid-absolute drag anchor (`GizmoPrevPos::abs_pos`).
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_interpolation: Query<(Has<TranslationInterpolation>, Has<RotationInterpolation>)>,
    q_floating_origin: Query<Entity, With<FloatingOrigin>>,
    mut physics_holds: ResMut<lunco_physics::PhysicsHolds>,
    mut commands: Commands,
) {
    let mut captured_any = false;
    for (link, gizmo_target) in gizmo_targets.iter() {
        let entity = link.target;
        if !gizmo_target.is_active() {
            continue;
        }
        if q_prev_pos.get(entity).is_ok() {
            continue;
        }
        captured_any = true;

        // 2. DISABLE INTERPOLATION
        // Remove interpolation components so the visual mesh doesn't "fight" the gizmo.
        let (had_translation, had_rotation) = q_interpolation.get(entity).unwrap_or((false, false));
        if had_translation {
            commands.entity(entity).remove::<TranslationInterpolation>();
        }
        if had_rotation {
            commands.entity(entity).remove::<RotationInterpolation>();
        }

        let original_body = q_rigid_bodies.get(entity).copied().ok();

        // Resolve initial parent-local position, and the absolute pose the drag
        // velocity is differenced from (see `GizmoPrevPos::abs_pos`).
        let Ok((_, tf)) = q_spatial.get(entity) else {
            continue;
        };
        let local_pos = tf.translation.as_dvec3();
        let abs_pos = lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial)
            .unwrap_or(local_pos);

        info!(
            "GIZMO: drag started for {:?}, local_pos={:?}",
            entity, local_pos
        );

        commands
            .entity(entity)
            .try_insert(RigidBody::Kinematic)
            .try_insert(GizmoPrevPos {
                local_pos,
                abs_pos,
                original_body,
                had_translation_interpolation: had_translation,
                had_rotation_interpolation: had_rotation,
            });
    }

    if captured_any {
        // A selected lander is an articulation: legs and pads are separate
        // dynamic bodies coupled to the root. Holding the entire physics world
        // is the only atomic capture boundary available to Avian; changing only
        // the root to Kinematic leaves live joints to integrate against a pose
        // the gizmo is mutating, which creates unbounded impulses.
        physics_holds.set(lunco_physics::PhysicsHolds::CINEMATIC, true);
        // 1. FREEZE COORDINATE SYSTEM
        // Remove FloatingOrigin from the camera. This stops big_space from shifting
        // the world while we drag, breaking the positive feedback loop with the camera.
        for cam_ent in q_floating_origin.iter() {
            commands.entity(cam_ent).try_remove::<FloatingOrigin>();
            info!("GIZMO: freezing FloatingOrigin on camera {:?}", cam_ent);
        }
    }
}

/// Syncs Avian `Position` and computes velocity from local coordinates.
pub fn sync_gizmo_transforms(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    mut q_prev_pos: Query<&mut GizmoPrevPos>,
    time: Res<Time>,
) {
    for (link, gizmo_target) in gizmo_targets.iter() {
        let entity = link.target;
        if !gizmo_target.is_active() {
            continue;
        }

        let Ok((_, tf)) = q_spatial.get(entity) else {
            continue;
        };
        let local_pos = tf.translation.as_dvec3();

        // NO `Position`/`Rotation` write. Those are the BigSpace ROOT frame;
        // `tf.translation` is the cell REMAINDER. Writing one into the other was
        // right only in the origin cell and dropped `cell × edge` (2 km) anywhere
        // else — the bridge's writeback then fed that short Position back into
        // `Transform` (the drag pins the body Kinematic, which the writeback
        // covers) and the object vanished a cell away. `pose_to_position` seats
        // both from the `(cell, Transform)` the drag writes, which is the only
        // side of this that knows the cell.

        // `restore_dragged_transform` clamps the mesh back to `prev.local_pos`
        // every frame (to cancel Avian's integrator writeback), so `prev` MUST
        // advance to the gizmo's current position every frame — including while
        // PAUSED. When paused, time is frozen and `delta_secs()` is 0; the old
        // `if dt > 1e-6` gate wrapped the whole block, so `prev` went stale and
        // the restore snapped the object back to its drag-start spot — the gizmo
        // couldn't move anything while paused. Only the velocity estimate (which
        // drags joint-coupled child bodies along and is meaningless at dt = 0)
        // stays gated on dt.
        if let Ok(mut prev) = q_prev_pos.get_mut(entity) {
            // ABSOLUTE, not the cell remainder — see `GizmoPrevPos::abs_pos`.
            let abs_pos =
                lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial)
                    .unwrap_or(local_pos);
            let dt = time.delta_secs();
            if dt > 1e-6 {
                let delta = abs_pos - prev.abs_pos;
                if let Ok(mut lin_vel) = q_lin_vel.get_mut(entity) {
                    let v = delta / dt as f64;
                    // Finite AND bounded: a mid-drag reparent, a scene reload or a
                    // teleport can still move the absolute pose arbitrarily far in
                    // one frame, and this velocity is injected into a Kinematic body
                    // that drags its joint-coupled children with it.
                    lin_vel.0 = if v.is_finite() {
                        v.clamp_length_max(1.0e3)
                    } else {
                        DVec3::ZERO
                    };
                }
            }
            prev.abs_pos = abs_pos;
            prev.local_pos = local_pos;
        }
    }
}

/// TODO: Architectural Workaround for User vs. Physics Authority Conflict
///
/// **Why we have this:**
/// During active user-drag (gizmo), the visual editor (`transform-gizmo-bevy`)
/// is the absolute authority on `Transform`. However, because the entity has
/// `LinearVelocity` set (so joint-coupled dynamic child bodies are dragged along
/// by the solver), Avian's integrator updates the physics `Position` and its
/// writeback system overwrites `Transform` with `local_pos + delta`. Without this
/// system restoring the visual position, `transform-gizmo-bevy` would read the
/// overwritten value and add the new mouse delta on top of it, creating a 2x
/// speed feedback loop (runaway/multiplication of movement).
///
/// **The Proper Fix:**
/// Once Avian3D introduces a first-class Kinematic Drive/Teleport API (allowing
/// manual positioning of kinematic bodies with implicit velocity calculation for
/// joints *without* running the integrator step) or a way to disable writeback
/// on a per-entity basis, this system should be replaced with that native API.
pub fn restore_dragged_transform(mut q: Query<(&mut Transform, &GizmoPrevPos)>) {
    for (mut tf, prev) in q.iter_mut() {
        tf.translation = prev.local_pos.as_vec3();
    }
}

/// Restores dynamic state and re-enables origin tracking when gizmo drag ends —
/// and **authors the completed move into USD**.
///
/// USD is the source of truth for *authored* state, so a gizmo drag must end up
/// as a document op, not just an ECS `Transform` write (which is lost on reload
/// and never reaches the Twin journal / networked peers). Before this, a gizmo drag
/// was invisible to USD: it never saved, never journaled, never replicated, and
/// Ctrl+Z could not touch it — the same class of gap the old editor-side undo stack
/// was papering over.
///
/// The op-authoring path already exists — [`crate::commands::MoveEntity`] is observed
/// by `persist_move_to_runtime_layer`, which authors `UsdOp::SetTranslate` into the
/// active document's runtime layer (ownership-guarded: a non-document entity simply
/// doesn't author). The drag itself is deliberately ECS-only, so drag-end fires
/// exactly ONE `MoveEntity` per completed drag — not one per frame, which would flood
/// the journal with a thousand ops for a single drag. (That is what
/// `EditIntent::Interactive` means elsewhere.)
///
/// No fight with re-projection: `SetTranslate` lands as an `InfoOnly` change and
/// `live_consume::apply_translates_live` writes the entity's `Transform` to the
/// value we just authored (identical to where the drag left it), with no
/// structural rebuild. The drag is over by then, so the gizmo has nothing to
/// fight.
pub fn restore_gizmo_dynamic(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    q_prev_pos: Query<(Entity, &GizmoPrevPos)>,
    mut q_lin_vel: Query<&mut LinearVelocity>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_avatar: Query<Entity, With<lunco_core::Avatar>>,
    q_floating_origin: Query<Entity, With<FloatingOrigin>>,
    q_tf: Query<&Transform>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_prim: Query<&lunco_usd_bevy::UsdPrimPath>,
    usd_registry: Option<Res<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut physics_holds: ResMut<lunco_physics::PhysicsHolds>,
    mut commands: Commands,
) {
    let mut restored_any = false;
    for (entity, prev) in q_prev_pos.iter() {
        if gizmo_targets
            .iter()
            .any(|(link, gt)| link.target == entity && gt.is_active())
        {
            continue;
        }

        restored_any = true;

        info!(
            "GIZMO: drag ended for {:?}, restoring coordinate systems",
            entity
        );

        // The pose to author is GRID-ABSOLUTE — the frame `xformOp:translate`
        // means on a grid-direct prim (spawn plants its whole value at cell 0
        // and lets big_space re-split it). `tf.translation` is what's LEFT after
        // that split, so authoring it raw published a position short by
        // `cell × edge`: at the moonbase the next projection of the prim
        // re-seated the panel 2 km under the site and it disappeared. In the
        // sandbox everything sits in cell 0, where the two agree — which is why
        // this survived until a twin with real cells.
        let abs = lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial);

        // Author the released pose. Same guard every other edit path uses, so a prim
        // the active document doesn't own is left alone.
        if let (Some(reg), Ok(tf), Some(abs)) = (usd_registry.as_deref(), q_tf.get(entity), abs) {
            if let Some((doc, path)) =
                crate::commands::authorable_prim(entity, &q_prim, reg, workspace.as_deref())
            {
                commands.trigger(lunco_usd::commands::ApplyUsdOp {
                    doc,
                    op: lunco_usd::document::UsdOp::SetTranslate {
                        edit_target: lunco_usd::document::LayerId::runtime(),
                        path: path.clone(),
                        value: [abs.x, abs.y, abs.z],
                    },
                });
                // The gizmo rotates as well as translates, so the rotation is part of
                // the authored pose — `xformOp:rotateXYZ`, Euler degrees.
                let (rx, ry, rz) = tf.rotation.to_euler(EulerRot::XYZ);
                commands.trigger(lunco_usd::commands::ApplyUsdOp {
                    doc,
                    op: lunco_usd::document::UsdOp::SetRotate {
                        edit_target: lunco_usd::document::LayerId::runtime(),
                        path,
                        value: [
                            rx.to_degrees() as f64,
                            ry.to_degrees() as f64,
                            rz.to_degrees() as f64,
                        ],
                    },
                });
            }
        }

        // 2. RESTORE INTERPOLATION
        if prev.had_translation_interpolation {
            commands.entity(entity).try_insert(TranslationInterpolation);
        }
        if prev.had_rotation_interpolation {
            commands.entity(entity).try_insert(RotationInterpolation);
        }

        if let Ok(mut vel) = q_lin_vel.get_mut(entity) {
            vel.0 = DVec3::ZERO;
        }

        // Hand the pre-drag body kind back. An entity that had NO `RigidBody`
        // gets the drag's Kinematic taken away again rather than being handed a
        // fabricated `Dynamic` — see `GizmoPrevPos::original_body`.
        match prev.original_body {
            Some(body) => {
                commands.entity(entity).try_insert(body);
            }
            None => {
                commands.entity(entity).try_remove::<RigidBody>();
            }
        }
        commands.entity(entity).try_remove::<GizmoPrevPos>();

        // AUTHOR THE MOVE. Queued AFTER the `original_body` insert above, so the
        // `MoveEntity` observer captures the pre-drag body kind (not the
        // Kinematic the drag forced) into `JustMovedKinematic.restore` and
        // `clear_kinematic_pulse_velocity` hands it back one tick later. An
        // entity without a `GlobalEntityId` isn't API/USD-addressable, so there
        // is nothing to author for it.
        // Grid-absolute, same as the op above — `MoveEntity::translation` is that
        // frame, not the raw `Transform`.
        if let (Ok(gid), Some(abs)) = (q_gid.get(entity), abs) {
            commands.trigger(crate::commands::MoveEntity {
                entity_id: gid.get(),
                translation: abs.as_vec3(),
            });
        }
    }

    // Re-attach FloatingOrigin to the avatar camera ONLY when the LAST drag
    // ends — i.e. no `GizmoTarget` is still active. With several entities
    // shift-selected and dragged together, restoring the origin the instant the
    // first one releases would un-freeze big_space *underneath* the entities
    // still being dragged (re-introducing the camera/origin feedback loop the
    // capture-time freeze exists to prevent).
    let any_still_active = gizmo_targets.iter().any(|(_, gt)| gt.is_active());
    // Take the origin off its current holder ONLY once we know who gets it next.
    // If the scene reloaded mid-drag the avatar is gone, and stripping the origin
    // with nobody to hand it to left the world with zero origins — big_space
    // logs "BigSpace … has no floating origins" and stops propagating transforms
    // until the `anchor_owns_origin_by_default` guard re-seats it a frame later.
    // No avatar means: leave whoever holds it holding it.
    if restored_any && !any_still_active && !q_avatar.is_empty() {
        // 1. RESTORE ORIGIN TRACKING
        // Claim FloatingOrigin from the fallback anchor.
        for origin in q_floating_origin.iter() {
            commands.entity(origin).try_remove::<FloatingOrigin>();
        }
        // Re-attach FloatingOrigin to the avatar camera.
        for av_ent in q_avatar.iter() {
            commands.entity(av_ent).try_insert(FloatingOrigin);
            info!(
                "GIZMO: restored FloatingOrigin on avatar camera {:?}",
                av_ent
            );
        }
    }
    // Resume only after every released drag has authored its root teleport and
    // its velocity was zeroed. This keeps a jointed lander from taking one fixed
    // step against a half-restored articulation.
    if restored_any && !any_still_active {
        physics_holds.set(lunco_physics::PhysicsHolds::CINEMATIC, false);
    }
}

/// App-owned replacement for transform-gizmo-bevy's default `mouse_interaction`
/// driver (disabled via Cargo features). The crate's version wrote
/// `GizmoDragStarted`/`GizmoDragging` on EVERY left press/hold — so the
/// **Shift+left-click** used to *select* an object also armed a drag, and once
/// the gizmo renders ON the object (its handles under the cursor) that grab
/// fired immediately. Gating on `!Shift` keeps Shift+click for selection only;
/// a **plain** left-drag on a handle still moves the object (the gizmo only
/// engages when `hovered`, i.e. the cursor is actually over a handle). Matches
/// the app's shift=select / plain=possess partition (see `on_scene_click_select`).
pub fn drive_gizmo_drag_no_shift(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    mut drag_started: MessageWriter<GizmoDragStarted>,
    mut dragging: MessageWriter<GizmoDragging>,
) {
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]) {
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        drag_started.write_default();
    }
    if mouse.pressed(MouseButton::Left) {
        dragging.write_default();
    }
}

/// Keeps `GizmoCamera` on the **active** window camera only.
///
/// The gizmo renders/interacts through whichever camera carries `GizmoCamera`.
/// With multiple scene cameras present (USD `def Camera` prims spawn as extra
/// window `Camera3d`s), tagging *every* window camera made the gizmo bind to
/// the wrong one. So exactly the active window camera (`Camera::is_active`) is
/// tagged; the rest are untagged as the active view switches.
pub fn sync_gizmo_camera(
    q_cameras: Query<(Entity, &Camera, &RenderTarget), With<Camera3d>>,
    q_tagged: Query<Entity, With<GizmoCamera>>,
    mut commands: Commands,
) {
    let active = q_cameras
        .iter()
        .find(|(_, cam, target)| cam.is_active && matches!(target, RenderTarget::Window(_)))
        .map(|(e, _, _)| e);

    // Untag any camera that is no longer the active window view. FALLIBLE: a scene
    // clear (LoadScene) despawns the scene's cameras, and this system's queries were
    // built before that despawn flushed — so `tagged`/`active` can already be dead by
    // the time these commands apply. A plain `remove`/`insert` panics on that
    // ("Entity despawned: ID … is invalid", from `apply_deferred`) and takes the app
    // down mid-reload; the `try_` forms just no-op on a dead entity.
    for tagged in q_tagged.iter() {
        if Some(tagged) != active {
            commands.entity(tagged).try_remove::<GizmoCamera>();
        }
    }
    // Tag the active window camera (idempotent).
    if let Some(active) = active {
        if !q_tagged.contains(active) {
            commands.entity(active).try_insert(GizmoCamera);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_controller::ControllerLink;

    #[test]
    fn test_gizmo_prev_pos_component() {
        let pos = GizmoPrevPos {
            local_pos: DVec3::new(1.0, 2.0, 3.0),
            abs_pos: DVec3::new(1.0, 2.0, 3.0),
            original_body: Some(RigidBody::Dynamic),
            had_translation_interpolation: false,
            had_rotation_interpolation: false,
        };
        assert_eq!(pos.local_pos, DVec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_possessed_entity_gizmo_restoration() {
        use crate::SelectedEntities;

        let mut app = App::new();
        app.init_resource::<SelectedEntities>();
        app.add_systems(Update, restore_gizmo_dynamic);

        let vessel = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::ZERO),
                RigidBody::Kinematic,
                GizmoTarget::default(),
                GizmoPrevPos {
                    local_pos: DVec3::ZERO,
                    abs_pos: DVec3::ZERO,
                    original_body: Some(RigidBody::Dynamic),
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                LinearVelocity::default(),
            ))
            .id();

        app.world_mut().spawn(ControllerLink {
            vessel_entity: vessel,
        });
        app.world_mut()
            .resource_mut::<SelectedEntities>()
            .entities
            .push(vessel);

        app.update();

        assert_eq!(
            app.world().get::<RigidBody>(vessel),
            Some(&RigidBody::Dynamic)
        );
        assert!(app.world().get::<GizmoPrevPos>(vessel).is_none());
    }

    /// Dragging a prop that was never a rigid body must not MAKE it one.
    ///
    /// The drag itself inserts `RigidBody::Kinematic` so the gizmo can move the
    /// thing without avian fighting it. Restore then has to put back what was
    /// there before — and for plain scene geometry that is *nothing*.
    /// `original_body` used to be a bare `RigidBody`, so the capture had to
    /// invent a value for the had-no-body case and restore fabricated a
    /// `RigidBody::Dynamic` on a mass-less entity. Avian then logged
    /// "Dynamic rigid body … has no mass or inertia" every frame for an entity
    /// the user had merely nudged. Hence `Option`: `None` means remove.
    #[test]
    fn dragging_a_non_body_leaves_it_a_non_body() {
        use crate::SelectedEntities;

        let mut app = App::new();
        app.init_resource::<SelectedEntities>();
        app.add_systems(Update, restore_gizmo_dynamic);

        let prop = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::ZERO),
                // What the drag put on it — not what it started with.
                RigidBody::Kinematic,
                GizmoTarget::default(),
                GizmoPrevPos {
                    local_pos: DVec3::ZERO,
                    abs_pos: DVec3::ZERO,
                    original_body: None,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                LinearVelocity::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<SelectedEntities>()
            .entities
            .push(prop);

        app.update();

        assert!(
            app.world().get::<RigidBody>(prop).is_none(),
            "restore must REMOVE the drag's kinematic body, not swap in a \
             fabricated Dynamic one — a mass-less Dynamic body makes avian \
             log 'has no mass or inertia' forever"
        );
        assert!(app.world().get::<GizmoPrevPos>(prop).is_none());
    }

    /// A2: the gizmo is not an authority — a completed drag authors USD.
    /// Drag-end fires `MoveEntity`, whose `persist_move_to_runtime_layer`
    /// observer writes `xformOp:translate` into the document's RUNTIME layer, so
    /// the move survives a reload instead of living only in ECS.
    #[test]
    fn drag_end_authors_the_move_into_the_runtime_layer() {
        use lunco_doc_bevy::DocumentRegistry;
        use lunco_usd::document::UsdDocument;
        use lunco_usd_bevy::usd_data::UsdDataExt;
        use lunco_usd_bevy::UsdPrimPath;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        // Provides `DocumentRegistry<UsdDocument>` + the `ApplyUsdOp` handler the
        // persister dispatches into.
        app.add_plugins(lunco_usd::commands::UsdCommandsPlugin);
        app.init_resource::<lunco_api::registry::ApiEntityRegistry>();
        app.add_observer(crate::commands::persist_move_to_runtime_layer);
        app.add_systems(Update, restore_gizmo_dynamic);

        let doc = {
            let mut reg = app
                .world_mut()
                .resource_mut::<DocumentRegistry<UsdDocument>>();
            reg.allocate(
                "#usda 1.0\ndef Xform \"World\"\n{\n}\n".to_string(),
                lunco_doc::PathlessOrigin::untitled("Scene.usda"),
            )
        };
        let mut ws = lunco_workspace::Workspace::default();
        ws.active_document = Some(doc);
        app.insert_resource(lunco_workspace::WorkspaceResource(ws));

        // An entity mid-drag (has `GizmoPrevPos`) whose drag just ended (no
        // active `GizmoTarget`), sitting where the drag left it.
        let dragged = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(3.0, 4.0, 5.0)),
                RigidBody::Kinematic,
                LinearVelocity::default(),
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/World".to_string(),
                },
                lunco_core::GlobalEntityId::from_raw(42),
                GizmoPrevPos {
                    local_pos: DVec3::new(3.0, 4.0, 5.0),
                    abs_pos: DVec3::new(3.0, 4.0, 5.0),
                    original_body: Some(RigidBody::Dynamic),
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_api::registry::ApiEntityRegistry>()
            .assign(dragged, lunco_core::GlobalEntityId::from_raw(42));

        for _ in 0..3 {
            app.update();
        }

        let reg = app.world().resource::<DocumentRegistry<UsdDocument>>();
        let docu = reg.host(doc).expect("doc alive").document();
        let world_path = lunco_usd_bevy::SdfPath::new("/World").unwrap();
        assert_eq!(
            docu.runtime_data()
                .prim_attribute_value::<[f64; 3]>(&world_path, "xformOp:translate"),
            Some([3.0, 4.0, 5.0]),
            "drag-end must author the move into the runtime layer"
        );
        // Save stays base-only: the runtime move never dirties the .usda.
        assert!(
            !docu.source().contains("xformOp:translate"),
            "base layer untouched by a runtime move"
        );
        // Drag bookkeeping still completes (body restored, marker cleared).
        assert!(app.world().get::<GizmoPrevPos>(dragged).is_none());
    }
}
