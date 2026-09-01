//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` for render-space picking and manipulation. The
//! gizmo never owns a scene transform: this module translates its proxy pose
//! through the active BigSpace frame and the scene-command boundary. It also handles:
//! - Making bodies kinematic during gizmo drag
//! - Holding physics integration during manual dragging
//! - Disabling physics interpolation during manual dragging
//! - Restoring dynamic bodies when drag ends
//!
//! **Architectural Note**: a drag is a transaction over one semantic f64 pose.
//! Render `Transform` values are only a frontend representation; BigSpace cell
//! storage and Avian pose state remain owned by their existing adapters.

use std::collections::HashSet;

use avian3d::prelude::{
    AngularVelocity, CustomPositionIntegration, LinearVelocity, RigidBody, RotationInterpolation,
    TranslationInterpolation,
};
use bevy::camera::RenderTarget;
use bevy::math::DVec3;
use bevy::prelude::*;
use lunco_core::SceneViewport;
use lunco_render::SceneCamera;
use transform_gizmo_bevy::{
    GizmoCamera, GizmoDragStarted, GizmoDragging, GizmoMode, GizmoOptions, GizmoTarget,
};

/// Configure the standard transform-gizmo frontend to expose only operations
/// backed by the scene command contract. Scale is deliberately absent until
/// authored scale and runtime projection have an owner.
pub fn configure_gizmo_modes(mut options: ResMut<GizmoOptions>) {
    for mode in [
        GizmoMode::ScaleX,
        GizmoMode::ScaleY,
        GizmoMode::ScaleZ,
        GizmoMode::ScaleUniform,
        GizmoMode::ScaleXY,
        GizmoMode::ScaleXZ,
        GizmoMode::ScaleYZ,
    ] {
        options.gizmo_modes.remove(mode);
    }
}

/// Saves the user's gizmo configuration while planetary presentation owns the
/// viewport and the transform frontend is disabled.
#[derive(Resource, Default)]
pub(crate) struct GizmoVisibilityState {
    saved_options: Option<GizmoOptions>,
}

/// The authoritative lifecycle of a gizmo edit.
///
/// Bevy command insertion is deferred, so a component query is not a drag
/// session. Keeping the captured entities here makes capture/restore idempotent
/// across frames and scene reloads. BigSpace origin ownership remains with the
/// persistent `OriginAnchor`; a gizmo drag never changes that hierarchy owner.
#[derive(Resource, Default)]
pub struct GizmoDragSession {
    /// Real entities whose pre-drag state is owned by this session.
    targets: HashSet<Entity>,
}

/// Captures the pre-drag body state for drag lifecycle restoration.
#[derive(Component)]
pub struct GizmoDragState {
    /// The active semantic frame captured at drag start.
    pub active_frame: Entity,
    /// The pose before the drag, in `active_frame`.
    pub original_position: DVec3,
    pub original_rotation: bevy::math::DQuat,
    /// The latest valid pose proposed by the gizmo, in `active_frame`.
    pub current_position: DVec3,
    pub current_rotation: bevy::math::DQuat,
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
/// y=1946.5, the origin anchor at cell.y=1. The handles were drawn a
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
        let (scale, rotation, translation) = global.to_scale_rotation_translation();
        let proxy = commands
            .spawn((
                Name::new("GizmoProxy"),
                Transform::from_translation(translation)
                    .with_rotation(rotation)
                    .with_scale(scale),
                GlobalTransform::default(),
                GizmoTarget::default(),
                GizmoProxy { target },
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

/// Parks each proxy on the pose owned by its current side of the transaction.
///
/// Idle proxies use the target's propagated render pose. During a drag the
/// transaction owns an active-frame f64 pose, which is projected back through
/// BigSpace every frame. That re-projection is what keeps the handle attached
/// when a cell or floating origin changes between interaction steps.
///
/// Runs after `TransformSystems::Propagate` (big_space's propagation is in that
/// set), so the `GlobalTransform` read here is this frame's.
pub fn sync_gizmo_proxies(
    mut q_proxies: Query<(&mut Transform, &GizmoProxy, &GizmoTarget)>,
    q_targets: Query<&GlobalTransform, Without<GizmoProxy>>,
    q_drag: Query<&GizmoDragState, Without<GizmoProxy>>,
    q_grids: Query<&big_space::prelude::Grid>,
) {
    for (mut tf, link, _gizmo_target) in &mut q_proxies {
        if let Ok(state) = q_drag.get(link.target) {
            let Ok(grid) = q_grids.get(state.active_frame) else {
                continue;
            };
            let (render_position, render_rotation) =
                lunco_core::coords::grid_absolute_pose_to_render(
                    grid,
                    lunco_core::coords::GridPos(state.current_position),
                    lunco_core::coords::GridRot(state.current_rotation),
                );
            tf.translation = render_position.0.as_vec3();
            tf.rotation = render_rotation.0.as_quat();
            // Scale editing is intentionally disabled until it has a scene
            // command and an authored contract of its own.
            tf.scale = Vec3::ONE;
            continue;
        }
        let Ok(global) = q_targets.get(link.target) else {
            continue;
        };
        let (scale, rotation, translation) = global.to_scale_rotation_translation();
        tf.translation = translation;
        tf.rotation = rotation;
        tf.scale = scale;
    }
}

/// Convert the gizmo frontend's render-space pose into the active semantic
/// frame. Both the live drag and the release edge use this exact conversion;
/// keeping it here prevents the release edge from falling back to a stale
/// cached pose or inventing a second frame convention.
fn proxy_pose_to_active_frame(
    grid: &big_space::prelude::Grid,
    tf: &Transform,
) -> Option<(DVec3, bevy::math::DQuat)> {
    if !tf.translation.is_finite()
        || !tf.rotation.is_finite()
        || tf.rotation.length_squared() < 1.0e-12
    {
        return None;
    }

    let (position, rotation) = lunco_core::coords::render_pose_to_grid_absolute(
        grid,
        lunco_core::coords::RenderPos::from_render_f32(tf.translation),
        lunco_core::coords::GridRot::from_render_rotation(tf.rotation),
    );
    if !position.0.is_finite() || !rotation.0.is_finite() {
        return None;
    }
    Some((position.0, rotation.0))
}

/// Transfers the proxy's complete render pose into the real entity.
///
/// This is deliberately an absolute conversion, never a render-space delta:
/// render → active-frame f64 → actual parent-local `(CellCoord, Transform)`.
/// The same active pose also drives Avian through `KinematicDrive`; the
/// BigSpace physics bridge remains the sole Position/Rotation adapter.
pub fn apply_gizmo_proxy_drag(
    q_proxies: Query<(&Transform, &GizmoProxy, &GizmoTarget)>,
    mut world: ParamSet<(
        Query<(Option<&big_space::prelude::CellCoord>, &Transform), Without<GizmoProxy>>,
        Query<&mut Transform, Without<GizmoProxy>>,
        Query<&mut lunco_physics::KinematicDrive, (With<GizmoDragState>, Without<GizmoProxy>)>,
        Query<&mut GizmoDragState, Without<GizmoProxy>>,
    )>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids: Query<&big_space::prelude::Grid>,
    q_parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    for (tf, link, gizmo_target) in &q_proxies {
        if !gizmo_target.is_active() {
            continue;
        }
        let Ok(state_snapshot) = world.p3().get(link.target).map(|state| {
            (
                state.active_frame,
                state.original_position,
                state.original_rotation,
            )
        }) else {
            continue;
        };
        // A BigSpace handoff invalidates the transaction. Do not reinterpret a
        // proxy pose captured in the old semantic frame through the new frame
        // for even one interaction update; Last-stage cleanup will discard it.
        if state_snapshot.0 != active_frame.0 {
            continue;
        }
        let Ok(grid) = q_grids.get(state_snapshot.0) else {
            continue;
        };
        let Some((position, rotation)) = proxy_pose_to_active_frame(grid, tf) else {
            continue;
        };

        let (old_cell, new_cell, new_translation, new_rotation) = {
            let q_spatial = world.p0();
            let Ok((old_cell, _)) = q_spatial.get(link.target) else {
                continue;
            };
            let Some((new_cell, new_translation)) =
                lunco_core::coords::position_in_grid_to_parent_local(
                    link.target,
                    position,
                    state_snapshot.0,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                )
            else {
                continue;
            };
            let Some(new_rotation) = lunco_core::coords::rotation_in_grid_to_parent_local(
                link.target,
                rotation,
                state_snapshot.0,
                &q_parents,
                &q_grids,
                &q_spatial,
            ) else {
                continue;
            };
            (old_cell.copied(), new_cell, new_translation, new_rotation)
        };

        if let Ok(mut target_tf) = world.p1().get_mut(link.target) {
            target_tf.translation = new_translation;
            target_tf.rotation = new_rotation.as_quat();
        } else {
            continue;
        }
        match (new_cell, old_cell) {
            (Some(cell), _) => {
                commands.entity(link.target).try_insert(cell);
            }
            (None, Some(_)) => {
                commands
                    .entity(link.target)
                    .try_remove::<big_space::prelude::CellCoord>();
            }
            (None, None) => {}
        }
        if let Ok(mut drive) = world.p2().get_mut(link.target) {
            drive.set_pose(position, rotation);
        }
        if let Ok(mut state) = world.p3().get_mut(link.target) {
            state.current_position = position;
            state.current_rotation = rotation;
        }
    }
}

/// Capture the transform-gizmo crate's final proxy write before the release
/// cleanup consumes the drag transaction.
///
/// `transform-gizmo-bevy::update_gizmos` runs in `Last`, after the unpausable
/// interaction schedule that normally transfers active drags. On the release
/// frame it clears `GizmoTarget::is_active` immediately after writing the last
/// proxy pose, so `apply_gizmo_proxy_drag` cannot see that write. Snapshotting
/// the proxy here keeps the transaction's current pose authoritative without
/// touching the real entity or adding a competing transform writer.
pub fn capture_final_gizmo_pose(
    q_proxies: Query<(&Transform, &GizmoProxy, &GizmoTarget)>,
    mut q_drag: Query<&mut GizmoDragState, Without<GizmoProxy>>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_grids: Query<&big_space::prelude::Grid>,
    session: Res<GizmoDragSession>,
) {
    for (proxy_tf, link, gizmo_target) in &q_proxies {
        if gizmo_target.is_active() || !session.targets.contains(&link.target) {
            continue;
        }
        let Ok(mut drag) = q_drag.get_mut(link.target) else {
            continue;
        };
        if drag.active_frame != active_frame.0 {
            continue;
        }
        let Ok(grid) = q_grids.get(drag.active_frame) else {
            continue;
        };
        let Some((position, rotation)) = proxy_pose_to_active_frame(grid, proxy_tf) else {
            continue;
        };
        drag.current_position = position;
        drag.current_rotation = rotation;
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
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    simulation_pose: lunco_physics::SimulationPoseQuery,
    q_rigid_bodies: Query<&RigidBody>,
    q_kinematic_state: Query<(
        Has<CustomPositionIntegration>,
        Option<&lunco_physics::KinematicDrive>,
    )>,
    q_interpolation: Query<(Has<TranslationInterpolation>, Has<RotationInterpolation>)>,
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
        // flushes captures and reopens the physics hold.
        if session.targets.contains(&entity) {
            continue;
        }

        let original_body = q_rigid_bodies.get(entity).copied().ok();
        let (had_custom_position_integration, original_drive) = q_kinematic_state
            .get(entity)
            .map_or((false, None), |(had_custom, drive)| {
                (had_custom, drive.copied())
            });

        // Physical entities come from Avian's exact f64 pose; nonphysical
        // entities come from the active-frame BigSpace hierarchy.
        let Some((position, rotation)) = simulation_pose.pose(entity) else {
            continue;
        };

        // Disable interpolation only after the authoritative pose has been
        // captured. A failed pose lookup leaves the entity untouched.
        let (had_translation, had_rotation) = q_interpolation.get(entity).unwrap_or((false, false));
        if had_translation {
            commands.entity(entity).remove::<TranslationInterpolation>();
        }
        if had_rotation {
            commands.entity(entity).remove::<RotationInterpolation>();
        }

        captured_any = true;
        session.targets.insert(entity);

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
                    active_frame: active_frame.0,
                    original_position: position.0,
                    original_rotation: rotation.0,
                    current_position: position.0,
                    current_rotation: rotation.0,
                    original_body,
                    original_drive,
                    had_custom_position_integration,
                    had_translation_interpolation: had_translation,
                    had_rotation_interpolation: had_rotation,
                },
            ));
    }

    if captured_any {
        // A selected lander is an articulation: the leg bodies are coupled to
        // the root and carry welded footpad colliders. Holding the entire physics world
        // is the only atomic capture boundary available to Avian; changing only
        // the root to Kinematic leaves live joints to integrate against a pose
        // the gizmo is mutating, which creates unbounded impulses.
        physics_holds.set(lunco_physics::PhysicsHolds::CINEMATIC, true);
    }
}

/// Finish or cancel the active gizmo transactions and restore their pre-drag
/// physics state. A completed transaction emits exactly one
/// [`lunco_scene_commands::commands::TransformEntity`] command; the command
/// layer owns the live and USD persistence legs together.
pub fn restore_gizmo_dynamic(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    active_frame: Res<lunco_core::ActivePhysicsFrame>,
    q_drag: Query<(Entity, &GizmoDragState)>,
    mut q_vel: Query<(Option<&mut LinearVelocity>, Option<&mut AngularVelocity>)>,
    q_gid: Query<&lunco_core::GlobalEntityId>,
    mut spatial: ParamSet<(
        Query<(Option<&big_space::prelude::CellCoord>, &Transform)>,
        Query<&mut Transform>,
    )>,
    q_parents: Query<&ChildOf>,
    q_grids: Query<&big_space::prelude::Grid>,
    mut session: ResMut<GizmoDragSession>,
    mut physics_holds: ResMut<lunco_physics::PhysicsHolds>,
    mut commands: Commands,
) {
    let mut restored_entities = Vec::new();
    let released = mouse
        .as_deref()
        .is_some_and(|buttons| buttons.just_released(MouseButton::Left));
    let cancelled = keys
        .as_deref()
        .is_some_and(|buttons| buttons.just_pressed(KeyCode::Escape));
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
        if active && !released && !cancelled {
            continue;
        }

        let frame_changed = active_frame.0 != drag.active_frame;
        let cancel_transaction = cancelled || frame_changed;

        info!(
            "GIZMO: drag ended for {:?}, restoring coordinate systems",
            entity
        );

        if cancel_transaction && !frame_changed {
            let rollback = {
                let q_spatial = spatial.p0();
                q_spatial.get(entity).ok().and_then(|(old_cell, _)| {
                    let (new_cell, new_translation) =
                        lunco_core::coords::position_in_grid_to_parent_local(
                            entity,
                            drag.original_position,
                            drag.active_frame,
                            &q_parents,
                            &q_grids,
                            &q_spatial,
                        )?;
                    let new_rotation = lunco_core::coords::rotation_in_grid_to_parent_local(
                        entity,
                        drag.original_rotation,
                        drag.active_frame,
                        &q_parents,
                        &q_grids,
                        &q_spatial,
                    )?;
                    Some((old_cell.copied(), new_cell, new_translation, new_rotation))
                })
            };
            if let Some((old_cell, new_cell, new_translation, new_rotation)) = rollback {
                if let Ok(mut tf) = spatial.p1().get_mut(entity) {
                    tf.translation = new_translation;
                    tf.rotation = new_rotation.as_quat();
                    match (new_cell, old_cell) {
                        (Some(cell), _) => {
                            commands.entity(entity).try_insert(cell);
                        }
                        (None, Some(_)) => {
                            commands
                                .entity(entity)
                                .try_remove::<big_space::prelude::CellCoord>();
                        }
                        (None, None) => {}
                    }
                }
            } else {
                warn!(
                    ?entity,
                    active_frame = ?drag.active_frame,
                    "GIZMO: could not restore cancelled pose; completing physics cleanup without reinterpretation"
                );
            }
        }

        // 2. RESTORE INTERPOLATION
        if drag.had_translation_interpolation {
            commands.entity(entity).try_insert(TranslationInterpolation);
        }
        if drag.had_rotation_interpolation {
            commands.entity(entity).try_insert(RotationInterpolation);
        }

        if let Ok((linear, angular)) = q_vel.get_mut(entity) {
            if let Some(mut linear) = linear {
                linear.0 = DVec3::ZERO;
            }
            if let Some(mut angular) = angular {
                angular.0 = DVec3::ZERO;
            }
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

        // The active frame can change during a scene handoff. The old pose is
        // then intentionally not reinterpreted in the new frame; dropping the
        // transaction is safer than inventing a cross-frame restore.
        //
        // Queue the commit after the original body/drive state above. When the
        // deferred commands flush, TransformEntity must see the pre-drag body
        // kind so its JustMovedKinematic marker preserves the same one-tick
        // restoration contract as TransformEntity.
        if !cancel_transaction {
            if let Ok(gid) = q_gid.get(entity) {
                commands.trigger(lunco_scene_commands::commands::TransformEntity {
                    entity_id: gid.get(),
                    translation: drag.current_position.to_array(),
                    rotation: drag.current_rotation.to_array(),
                });
            }
        }

        restored_entities.push(entity);
    }

    for entity in restored_entities {
        session.targets.remove(&entity);
    }
    // A scene reload can despawn a target before the deferred restore runs. Do
    // not leave a dead entity holding the physics transaction open forever.
    session.targets.retain(|entity| q_drag.get(*entity).is_ok());
    let session_empty = session.targets.is_empty();

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
    egui_focus: Res<lunco_core::EguiFocus>,
    q_targets: Query<&GizmoTarget>,
    mut drag_started: MessageWriter<GizmoDragStarted>,
    mut dragging: MessageWriter<GizmoDragging>,
) {
    if egui_focus.wants_pointer
        || keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight])
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

/// Keeps `GizmoCamera` on the viewport-bound window camera only.
///
/// The gizmo renders/interacts through whichever camera carries `GizmoCamera`.
/// With multiple scene cameras present (USD `def Camera` prims spawn as extra
/// window `Camera3d`s), tagging *every* window camera made the gizmo bind to
/// the wrong one. `SceneViewport::active_camera` is the presentation owner;
/// `Camera::is_active` is only checked as the reconciler's actuated readiness
/// state. The rest are untagged as the active view switches.
pub(crate) fn sync_gizmo_camera(
    viewport: Res<SceneViewport>,
    q_cameras: Query<(Entity, &Camera, &RenderTarget), (With<Camera3d>, With<SceneCamera>)>,
    q_tagged: Query<Entity, With<GizmoCamera>>,
    orbital_pin: Option<Res<lunco_celestial::OrbitalViewPin>>,
    mut options: ResMut<GizmoOptions>,
    mut visibility: ResMut<GizmoVisibilityState>,
    mut commands: Commands,
) {
    if orbital_pin.is_some_and(|pin| pin.active) {
        if visibility.saved_options.is_none() {
            visibility.saved_options = Some(*options);
        }
        options.gizmo_modes.clear();
        options.mode_override = None;
    } else if let Some(saved_options) = visibility.saved_options.take() {
        *options = saved_options;
    }

    let active = viewport.active_camera.and_then(|requested| {
        q_cameras
            .get(requested)
            .ok()
            .and_then(|(entity, camera, target)| {
                (camera.is_active && matches!(target, RenderTarget::Window(_))).then_some(entity)
            })
    });

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
    fn proxy_reconciliation_presents_only_the_current_selection() {
        let mut app = App::new();
        let first = app
            .world_mut()
            .spawn((
                GizmoSelected,
                Transform::from_translation(Vec3::new(1.0, 2.0, 3.0)),
                GlobalTransform::IDENTITY,
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(4.0, 5.0, 6.0)),
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.add_systems(
            PostUpdate,
            (spawn_gizmo_proxies, despawn_gizmo_proxies).chain(),
        );

        app.update();
        let proxies = {
            let world = app.world_mut();
            let mut query = world.query::<&GizmoProxy>();
            query
                .iter(world)
                .map(|proxy| proxy.target)
                .collect::<Vec<_>>()
        };
        assert_eq!(proxies, vec![first]);

        app.world_mut().entity_mut(first).remove::<GizmoSelected>();
        app.world_mut().entity_mut(second).insert(GizmoSelected);
        app.update();

        let proxies = {
            let world = app.world_mut();
            let mut query = world.query::<&GizmoProxy>();
            query
                .iter(world)
                .map(|proxy| proxy.target)
                .collect::<Vec<_>>()
        };
        assert_eq!(proxies, vec![second]);
    }

    #[test]
    fn gizmo_camera_follows_the_viewport_binding() {
        let mut app = App::new();
        app.init_resource::<SceneViewport>()
            .init_resource::<GizmoOptions>()
            .init_resource::<GizmoVisibilityState>();
        let first = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    is_active: true,
                    ..default()
                },
                RenderTarget::Window(bevy::window::WindowRef::Primary),
                SceneCamera::default(),
                GizmoCamera,
            ))
            .id();
        let second = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    is_active: true,
                    ..default()
                },
                RenderTarget::Window(bevy::window::WindowRef::Primary),
                SceneCamera::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(second);
        app.add_systems(Update, sync_gizmo_camera);

        app.update();

        assert!(app.world().get::<GizmoCamera>(second).is_some());
        assert!(app.world().get::<GizmoCamera>(first).is_none());
    }

    #[test]
    fn captures_final_proxy_pose_before_release_cleanup() {
        let mut app = App::new();
        app.init_resource::<GizmoDragSession>();

        let active_frame = app
            .world_mut()
            .spawn(big_space::prelude::Grid::new(2_000.0, 100.0))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active_frame));

        let wanted_position = DVec3::new(1234.5, -22.25, 78.5);
        let wanted_rotation =
            bevy::math::DQuat::from_rotation_y(0.37) * bevy::math::DQuat::from_rotation_x(-0.19);
        let (render_position, render_rotation) = {
            let grid = app
                .world()
                .get::<big_space::prelude::Grid>(active_frame)
                .unwrap();
            lunco_core::coords::grid_absolute_pose_to_render(
                grid,
                lunco_core::coords::GridPos(wanted_position),
                lunco_core::coords::GridRot(wanted_rotation),
            )
        };

        let target = app
            .world_mut()
            .spawn(GizmoDragState {
                active_frame,
                original_position: DVec3::ZERO,
                original_rotation: bevy::math::DQuat::IDENTITY,
                current_position: DVec3::ZERO,
                current_rotation: bevy::math::DQuat::IDENTITY,
                original_body: None,
                original_drive: None,
                had_custom_position_integration: false,
                had_translation_interpolation: false,
                had_rotation_interpolation: false,
            })
            .id();
        app.world_mut()
            .resource_mut::<GizmoDragSession>()
            .targets
            .insert(target);
        app.world_mut().spawn((
            Transform::from_translation(render_position.0.as_vec3())
                .with_rotation(render_rotation.0.as_quat()),
            GizmoProxy { target },
            GizmoTarget::default(),
        ));
        app.add_systems(Update, capture_final_gizmo_pose);

        app.update();

        let drag = app.world().get::<GizmoDragState>(target).unwrap();
        assert!((drag.current_position - wanted_position).length() < 1.0e-3);
        assert!(drag.current_rotation.dot(wanted_rotation).abs() > 1.0 - 1.0e-6);
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
        let active_frame = app
            .world_mut()
            .spawn(big_space::prelude::Grid::new(2_000.0, 100.0))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active_frame));
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
                    active_frame,
                    original_position: DVec3::ZERO,
                    original_rotation: bevy::math::DQuat::IDENTITY,
                    current_position: DVec3::ZERO,
                    current_rotation: bevy::math::DQuat::IDENTITY,
                    original_body: Some(RigidBody::Dynamic),
                    original_drive: None,
                    had_custom_position_integration: false,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                LinearVelocity(DVec3::new(4.0, 5.0, 6.0)),
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
        assert_eq!(
            app.world().get::<LinearVelocity>(vessel).unwrap().0,
            DVec3::ZERO
        );
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
        let active_frame = app
            .world_mut()
            .spawn(big_space::prelude::Grid::new(2_000.0, 100.0))
            .id();
        app.insert_resource(lunco_core::ActivePhysicsFrame(active_frame));
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
                    active_frame,
                    original_position: DVec3::ZERO,
                    original_rotation: bevy::math::DQuat::IDENTITY,
                    current_position: DVec3::ZERO,
                    current_rotation: bevy::math::DQuat::IDENTITY,
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
}
