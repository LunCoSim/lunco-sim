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

use std::collections::HashSet;

use avian3d::prelude::{
    AngularVelocity, CustomPositionIntegration, LinearVelocity, RigidBody, RotationInterpolation,
    TranslationInterpolation,
};
use bevy::camera::RenderTarget;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::FloatingOrigin;
use lunco_render::SceneCamera;
use transform_gizmo_bevy::{GizmoCamera, GizmoDragStarted, GizmoDragging, GizmoTarget};

/// The authoritative lifecycle of a gizmo edit.
///
/// Bevy command insertion is deferred, so a component query is not a drag
/// session. Keeping the captured entities and the exact FloatingOrigin holder
/// here makes capture/restore idempotent across frames and scene reloads.
#[derive(Resource, Default)]
pub struct GizmoDragSession {
    /// Real entities whose pre-drag state is owned by this session.
    targets: HashSet<Entity>,
    /// The entity that owned FloatingOrigin when the session began.
    frozen_origin: Option<Entity>,
    /// Whether the origin transaction has been opened.
    origin_frozen: bool,
}

/// Captures the pre-drag body state for drag lifecycle restoration.
#[derive(Component)]
pub struct GizmoDragState {
    /// Original RigidBody type before drag started, or `None` if the entity had
    /// no `RigidBody` at all. `None` must stay `None` on restore: inserting a
    /// `Dynamic` body onto a prim that never had one gives avian a body with no
    /// mass or inertia ("Dynamic rigid body has no mass or inertia. This can
    /// cause NaN values.") and hands the solver a NaN source that outlives the
    /// drag.
    pub original_body: Option<RigidBody>,
    /// A drive that existed before this gizmo session, if any.
    pub original_drive: Option<lunco_physics::KinematicDrive>,
    /// Whether custom Avian position integration existed before capture.
    pub had_custom_position_integration: bool,
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
/// landing correctly and made this look gizmo-specific. It works in the luncosim
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
            commands.entity(proxy).try_despawn();
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
/// Runs in [`lunco_time::InteractionSchedule`]. The gizmo crate updates its
/// proxy in `Last`, so this consumes that completed render-frame edit on the
/// next authoritative cycle without making the real entity compete with the
/// gizmo crate for its `Transform`.
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

/// Converts the edited floating-origin pose into the Avian global pose held by
/// the drag drive.
///
/// This is intentionally part of the unpaused interaction schedule. A gizmo
/// edit is an interface operation, not a physics integration step; it must
/// continue to update the physics pose while `Time<Physics>` is held.
pub fn drive_gizmo_kinematic_pose(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform), Without<GizmoProxy>>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut q_drives: Query<
        &mut lunco_physics::KinematicDrive,
        (With<GizmoDragState>, Without<GizmoProxy>),
    >,
) {
    for (link, gizmo_target) in &gizmo_targets {
        if !gizmo_target.is_active() {
            continue;
        }
        let entity = link.target;
        let Ok((cell, tf)) = q_spatial.get(entity) else {
            continue;
        };
        let Ok((position, rotation)) = lunco_core::coords::world_pose_seeded(
            entity, cell, tf, &q_parents, &q_grids, &q_spatial,
        ) else {
            continue;
        };
        if let Ok(mut drive) = q_drives.get_mut(entity) {
            drive.set_pose(position.0, rotation.0);
        }
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
    q_kinematic_state: Query<(
        Has<CustomPositionIntegration>,
        Option<&lunco_physics::KinematicDrive>,
    )>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_interpolation: Query<(Has<TranslationInterpolation>, Has<RotationInterpolation>)>,
    q_origin_holders: Query<(Entity, Has<FloatingOrigin>, Has<lunco_core::Avatar>)>,
    mut session: ResMut<GizmoDragSession>,
    mut physics_holds: ResMut<lunco_physics::PhysicsHolds>,
    mut commands: Commands,
) {
    let mut captured_any = false;
    for (link, gizmo_target) in gizmo_targets.iter() {
        let entity = link.target;
        if !gizmo_target.is_active() {
            continue;
        }
        // `GizmoDragState` is inserted through deferred commands. The session is
        // the synchronous guard; without it, every Last pass before the insert
        // flushes captures and freezes FloatingOrigin again.
        if session.targets.contains(&entity) {
            continue;
        }

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
        let (had_custom_position_integration, original_drive) = q_kinematic_state
            .get(entity)
            .map_or((false, None), |(had_custom, drive)| {
                (had_custom, drive.copied())
            });

        // Resolve the initial global pose for the drive. `Transform` is the
        // cell-local render remainder; the drive speaks Avian's global frame.
        let Ok((cell, tf)) = q_spatial.get(entity) else {
            continue;
        };
        captured_any = true;
        session.targets.insert(entity);
        let Ok((position, rotation)) = lunco_core::coords::world_pose_seeded(
            entity, cell, tf, &q_parents, &q_grids, &q_spatial,
        ) else {
            session.targets.remove(&entity);
            continue;
        };

        info!(
            "GIZMO: drag started for {:?}, global_pos={:?}",
            entity, position.0
        );

        commands
            .entity(entity)
            .try_insert(RigidBody::Kinematic)
            .try_insert((
                CustomPositionIntegration,
                lunco_physics::KinematicDrive::new(position.0, rotation.0),
                GizmoDragState {
                    original_body,
                    original_drive,
                    had_custom_position_integration,
                    had_translation_interpolation: had_translation,
                    had_rotation_interpolation: had_rotation,
                },
            ));
    }

    if captured_any && !session.origin_frozen {
        // A selected lander is an articulation: legs and pads are separate
        // dynamic bodies coupled to the root. Holding the entire physics world
        // is the only atomic capture boundary available to Avian; changing only
        // the root to Kinematic leaves live joints to integrate against a pose
        // the gizmo is mutating, which creates unbounded impulses.
        physics_holds.set(lunco_physics::PhysicsHolds::CINEMATIC, true);
        // 1. FREEZE COORDINATE SYSTEM
        // Remove FloatingOrigin from the camera. This stops big_space from shifting
        // the world while we drag, breaking the positive feedback loop with the camera.
        session.frozen_origin = q_origin_holders
            .iter()
            .find_map(|(entity, has_origin, _)| has_origin.then_some(entity));
        session.origin_frozen = true;
        if let Some(cam_ent) = session.frozen_origin {
            commands.entity(cam_ent).try_remove::<FloatingOrigin>();
            info!("GIZMO: freezing FloatingOrigin on camera {:?}", cam_ent);
        }
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
/// The op-authoring path already exists — [`lunco_scene_commands::commands::MoveEntity`] is observed
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
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    q_drag: Query<(Entity, &GizmoDragState)>,
    mut q_vel: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    q_origin_holders: Query<(Entity, Has<FloatingOrigin>, Has<lunco_core::Avatar>)>,
    q_tf: Query<&Transform>,
    q_spatial: Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_prim: Query<&lunco_usd_bevy::UsdPrimPath>,
    usd_registry: Option<Res<lunco_doc_bevy::DocumentRegistry<lunco_usd::document::UsdDocument>>>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    mut session: ResMut<GizmoDragSession>,
    mut physics_holds: ResMut<lunco_physics::PhysicsHolds>,
    mut commands: Commands,
) {
    let mut restored_entities = Vec::new();
    let released = mouse
        .as_deref()
        .is_some_and(|buttons| buttons.just_released(MouseButton::Left));
    for (entity, drag) in q_drag.iter() {
        if !session.targets.contains(&entity) {
            continue;
        }
        // `GizmoTarget::is_active` is the transform-gizmo crate's authoritative
        // interaction state.  Do not combine it with raw mouse state here:
        // `Last` is deliberately later than input processing, and doing so can
        // release a target during the same engagement frame, hiding the gizmo
        // and preventing the next drag while paused.
        let active = gizmo_targets
            .iter()
            .any(|(link, gt)| link.target == entity && gt.is_active());
        if active && !released {
            continue;
        }

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
        // luncosim everything sits in cell 0, where the two agree — which is why
        // this survived until a twin with real cells.
        let abs = lunco_core::coords::grid_absolute(entity, &q_parents, &q_grids, &q_spatial);

        // Author the released pose. Same guard every other edit path uses, so a prim
        // the active document doesn't own is left alone.
        if let (Some(reg), Ok(tf), Some(abs)) = (usd_registry.as_deref(), q_tf.get(entity), abs) {
            if let Some((doc, path)) = lunco_scene_commands::commands::authorable_prim(
                entity,
                &q_prim,
                reg,
                workspace.as_deref(),
            ) {
                commands.trigger(lunco_usd::commands::ApplyUsdOp {
                    doc,
                    op: lunco_usd::document::UsdOp::SetTranslate {
                        edit_target: lunco_usd::document::LayerId::runtime(),
                        path: path.clone(),
                        value: [abs.0.x, abs.0.y, abs.0.z],
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
        if drag.had_translation_interpolation {
            commands.entity(entity).try_insert(TranslationInterpolation);
        }
        if drag.had_rotation_interpolation {
            commands.entity(entity).try_insert(RotationInterpolation);
        }

        if let Ok((mut linear, mut angular)) = q_vel.get_mut(entity) {
            linear.0 = DVec3::ZERO;
            angular.0 = DVec3::ZERO;
        }

        // Hand the pre-drag body kind back. An entity that had NO `RigidBody`
        // gets the drag's Kinematic taken away again rather than being handed a
        // fabricated `Dynamic` — see `GizmoDragState::original_body`.
        match drag.original_body {
            Some(body) => {
                commands.entity(entity).try_insert(body);
            }
            None => {
                commands.entity(entity).try_remove::<RigidBody>();
            }
        }
        match drag.original_drive {
            Some(drive) => {
                commands.entity(entity).try_insert(drive);
            }
            None => {
                commands
                    .entity(entity)
                    .try_remove::<lunco_physics::KinematicDrive>();
            }
        }
        if drag.had_custom_position_integration {
            commands
                .entity(entity)
                .try_insert(CustomPositionIntegration);
        } else {
            commands
                .entity(entity)
                .try_remove::<CustomPositionIntegration>();
        }
        commands.entity(entity).try_remove::<GizmoDragState>();

        // AUTHOR THE MOVE. Queued AFTER the `original_body` insert above, so the
        // `MoveEntity` observer captures the pre-drag body kind (not the
        // Kinematic the drag forced) into `JustMovedKinematic.restore` and
        // `clear_kinematic_pulse_velocity` hands it back one tick later. An
        // entity without a `GlobalEntityId` isn't API/USD-addressable, so there
        // is nothing to author for it.
        // Grid-absolute, same as the op above — `MoveEntity::translation` is that
        // frame, not the raw `Transform`.
        if let (Ok(gid), Some(abs)) = (q_gid.get(entity), abs) {
            commands.trigger(lunco_scene_commands::commands::MoveEntity {
                entity_id: gid.get(),
                translation: abs.0.to_array(),
            });
        }
        restored_entities.push(entity);
    }

    for entity in restored_entities {
        session.targets.remove(&entity);
    }
    // A scene reload can despawn a target before the deferred restore runs. Do
    // not leave a dead entity holding the physics transaction open forever.
    session.targets.retain(|entity| q_tf.get(*entity).is_ok());
    let session_empty = session.targets.is_empty();

    // Restore the exact origin holder captured at drag start. Re-attaching to
    // "whatever avatar exists now" was the source of the asymmetric lifecycle:
    // repeated captures removed the origin from one entity while restore guessed
    // another. Only use the current avatar as the documented scene-reload
    // recovery when the original holder no longer exists.
    if session_empty && session.origin_frozen {
        let holder = session
            .frozen_origin
            .filter(|entity| q_tf.get(*entity).is_ok())
            .or_else(|| {
                q_origin_holders
                    .iter()
                    .find_map(|(entity, _, is_avatar)| is_avatar.then_some(entity))
            });
        if let Some(holder) = holder {
            for (origin, has_origin, _) in q_origin_holders.iter() {
                if has_origin && origin != holder {
                    commands.entity(origin).try_remove::<FloatingOrigin>();
                }
            }
            commands.entity(holder).try_insert(FloatingOrigin);
            info!("GIZMO: restored FloatingOrigin on {:?}", holder);
        }
        session.frozen_origin = None;
        session.origin_frozen = false;
    }
    // Resume only after every released drag has authored its root teleport and
    // its velocity was zeroed. This keeps a jointed lander from taking one fixed
    // step against a half-restored articulation.
    if session_empty {
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
    q_targets: Query<&GizmoTarget>,
    mut drag_started: MessageWriter<GizmoDragStarted>,
    mut dragging: MessageWriter<GizmoDragging>,
) {
    if keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
        || !q_targets.iter().any(|target| target.is_focused())
    {
        // Selection and gizmo interaction are two edges: the first click
        // creates/shows the target, and only a later click over a handle may
        // arm the drag. This remains true while physics is paused; a bare
        // click must never make the selected body start moving.
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
    q_cameras: Query<(Entity, &Camera, &RenderTarget), (With<Camera3d>, With<SceneCamera>)>,
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
    fn test_gizmo_drag_state_component() {
        let state = GizmoDragState {
            original_body: Some(RigidBody::Dynamic),
            original_drive: None,
            had_custom_position_integration: false,
            had_translation_interpolation: false,
            had_rotation_interpolation: false,
        };
        assert_eq!(state.original_body, Some(RigidBody::Dynamic));
    }

    #[test]
    fn test_possessed_entity_gizmo_restoration() {
        use lunco_scene_commands::SelectedEntities;

        let mut app = App::new();
        app.init_resource::<SelectedEntities>();
        // `restore_gizmo_dynamic` clears the drag's physics hold, so the resource
        // it writes must exist — a bare `App` has none, and the missing init made
        // the system fail param validation instead of running the case under test.
        app.init_resource::<lunco_physics::PhysicsHolds>();
        app.init_resource::<GizmoDragSession>();
        app.add_systems(Update, restore_gizmo_dynamic);

        let vessel = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::ZERO),
                RigidBody::Kinematic,
                CustomPositionIntegration,
                lunco_physics::KinematicDrive::new(DVec3::ZERO, bevy::math::DQuat::IDENTITY),
                GizmoTarget::default(),
                GizmoDragState {
                    original_body: Some(RigidBody::Dynamic),
                    original_drive: None,
                    had_custom_position_integration: false,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                LinearVelocity::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GizmoDragSession>()
            .targets
            .insert(vessel);

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
        assert!(app.world().get::<GizmoDragState>(vessel).is_none());
        assert!(app
            .world()
            .get::<lunco_physics::KinematicDrive>(vessel)
            .is_none());
        assert!(app
            .world()
            .get::<CustomPositionIntegration>(vessel)
            .is_none());
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
        use lunco_scene_commands::SelectedEntities;

        let mut app = App::new();
        app.init_resource::<SelectedEntities>();
        // `restore_gizmo_dynamic` clears the drag's physics hold, so the resource
        // it writes must exist — a bare `App` has none, and the missing init made
        // the system fail param validation instead of running the case under test.
        app.init_resource::<lunco_physics::PhysicsHolds>();
        app.init_resource::<GizmoDragSession>();
        app.add_systems(Update, restore_gizmo_dynamic);

        let prop = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::ZERO),
                // What the drag put on it — not what it started with.
                RigidBody::Kinematic,
                CustomPositionIntegration,
                lunco_physics::KinematicDrive::new(DVec3::ZERO, bevy::math::DQuat::IDENTITY),
                GizmoTarget::default(),
                GizmoDragState {
                    original_body: None,
                    original_drive: None,
                    had_custom_position_integration: false,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                LinearVelocity::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<GizmoDragSession>()
            .targets
            .insert(prop);
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
        assert!(app.world().get::<GizmoDragState>(prop).is_none());
        assert!(app
            .world()
            .get::<lunco_physics::KinematicDrive>(prop)
            .is_none());
        assert!(app.world().get::<CustomPositionIntegration>(prop).is_none());
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
        // Same reason as the tests above: `restore_gizmo_dynamic` writes the
        // physics hold, so the resource must exist for the system to run at all.
        app.init_resource::<lunco_physics::PhysicsHolds>();
        app.init_resource::<GizmoDragSession>();
        app.add_observer(lunco_scene_commands::commands::persist_move_to_runtime_layer);
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

        // An entity mid-drag (has `GizmoDragState`) whose drag just ended (no
        // active `GizmoTarget`), sitting where the drag left it.
        let dragged = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(3.0, 4.0, 5.0)),
                RigidBody::Kinematic,
                CustomPositionIntegration,
                lunco_physics::KinematicDrive::new(
                    DVec3::new(3.0, 4.0, 5.0),
                    bevy::math::DQuat::IDENTITY,
                ),
                LinearVelocity::default(),
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/World".to_string(),
                },
                lunco_core::GlobalEntityId::from_raw(42),
                GizmoDragState {
                    original_body: Some(RigidBody::Dynamic),
                    original_drive: None,
                    had_custom_position_integration: false,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<GizmoDragSession>()
            .targets
            .insert(dragged);
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
        assert!(app.world().get::<GizmoDragState>(dragged).is_none());
        assert!(app
            .world()
            .get::<lunco_physics::KinematicDrive>(dragged)
            .is_none());
        assert!(app
            .world()
            .get::<CustomPositionIntegration>(dragged)
            .is_none());
    }
}
