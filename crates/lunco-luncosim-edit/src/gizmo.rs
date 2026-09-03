//! Transform gizmo integration.
//!
//! Uses `transform-gizmo-bevy` for render-space picking and manipulation. The
//! gizmo never owns a scene transform: this module translates its proxy pose
//! through the authoritative owner boundary. Live entities use the active
//! BigSpace frame and scene command; isolated USD previews use their composed
//! Bevy hierarchy and `ApplyUsdOps`. It also handles:
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
use bevy::math::{DVec3, Rect};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use lunco_core::SceneViewport;
use lunco_doc::DocumentId;
use lunco_usd::document::LayerId;
use lunco_usd::ui::viewport::{
    UsdPreviewId, UsdViewportState, USD_PREVIEW_VIEW_PANEL_ID, USD_VIEWPORT_PANEL_ID,
};
use lunco_usd_bevy::UsdPrimPath;
use lunco_workbench::{PanelRect, PanelRects, ScenePickGate, SceneTarget};
use transform_gizmo_bevy::{
    GizmoCamera, GizmoDragStarted, GizmoDragging, GizmoMode, GizmoOptions, GizmoTarget,
};

const SCALE_MODES: [GizmoMode; 7] = [
    GizmoMode::ScaleX,
    GizmoMode::ScaleY,
    GizmoMode::ScaleZ,
    GizmoMode::ScaleUniform,
    GizmoMode::ScaleXY,
    GizmoMode::ScaleXZ,
    GizmoMode::ScaleYZ,
];

fn is_scale_mode(mode: GizmoMode) -> bool {
    SCALE_MODES.contains(&mode)
}

/// Configure the standard transform-gizmo frontend for the live-scene
/// contract. USD preview capability is enabled by [`sync_gizmo_camera`] only
/// when the focused presentation owner is an isolated preview.
pub fn configure_gizmo_modes(mut options: ResMut<GizmoOptions>) {
    for mode in SCALE_MODES {
        options.gizmo_modes.remove(mode);
    }
}

/// Saves the user's gizmo configuration while orbital presentation owns the
/// live viewport and the transform frontend is disabled there.
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

/// The authoritative owner of a gizmo transaction.
///
/// Live scene entities use the existing BigSpace/Avian pose and restoration
/// contract. Isolated USD previews use their explicit document lease and
/// parent-local Bevy transform; they never enter the live physics path.
#[derive(Clone, Debug)]
pub enum GizmoDragOwner {
    Live {
        active_frame: Entity,
        original_body: Option<RigidBody>,
        original_drive: Option<lunco_physics::KinematicDrive>,
        had_custom_position_integration: bool,
        had_translation_interpolation: bool,
        had_rotation_interpolation: bool,
    },
    UsdPreview {
        preview: UsdPreviewId,
        doc: DocumentId,
        edit_target: LayerId,
        generation: u64,
        path: String,
    },
}

/// Captures the pre-drag pose and its authoritative owner.
#[derive(Component)]
pub struct GizmoDragState {
    /// The pose before the drag. Live poses are in `owner`'s active frame;
    /// preview poses are local to the selected USD prim's Bevy parent.
    pub owner: GizmoDragOwner,
    pub original_position: DVec3,
    pub original_rotation: bevy::math::DQuat,
    /// Preview-local scale before the drag. `None` for live entities because
    /// the live physics contract does not expose scale editing.
    pub original_scale: Option<DVec3>,
    /// The latest valid pose proposed by the gizmo in the same space as the
    /// original pose.
    pub current_position: DVec3,
    pub current_rotation: bevy::math::DQuat,
    /// Latest preview-local scale proposed by the gizmo, when scale editing is
    /// owned by the USD preview transaction.
    pub current_scale: Option<DVec3>,
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
/// Live transactions own an active-frame f64 pose, which is projected back
/// through BigSpace every frame. Preview transactions own a parent-local pose
/// and are refreshed by ordinary Bevy hierarchy propagation.
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
            if let GizmoDragOwner::Live { active_frame, .. } = &state.owner {
                let Ok(grid) = q_grids.get(*active_frame) else {
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
                tf.scale = Vec3::ONE;
                continue;
            }
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
        || !tf.scale.is_finite()
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

/// Convert the gizmo frontend's render-space pose into a preview prim's local
/// transform using Bevy's authoritative hierarchy transform operation.
///
/// Preview entities are ordinary USD projections, not BigSpace bodies. Their
/// authored transform is local to the composed USD parent, so the render-space
/// proxy must be reparented through the current parent `GlobalTransform` before
/// it can be written to the projection or authored as the typed
/// `SetTranslate`/`SetRotate`/`SetScale` operations.
fn proxy_transform_to_preview_local(
    entity: Entity,
    tf: &Transform,
    q_parents: &Query<&ChildOf>,
    q_globals: &Query<&GlobalTransform, Without<GizmoProxy>>,
) -> Option<Transform> {
    let parent = match q_parents.get(entity) {
        Ok(parent) => Some(q_globals.get(parent.parent()).ok()?),
        Err(_) => None,
    };
    preview_global_to_local_transform(tf, parent)
}

fn preview_global_to_local_transform(
    tf: &Transform,
    parent: Option<&GlobalTransform>,
) -> Option<Transform> {
    let local = match parent {
        Some(parent) => GlobalTransform::from(*tf).reparented_to(parent),
        None => *tf,
    };
    if !local.translation.is_finite()
        || !local.rotation.is_finite()
        || local.rotation.length_squared() < 1.0e-12
        || !local.scale.is_finite()
    {
        return None;
    }
    Some(local)
}

fn local_transform_pose(tf: &Transform) -> Option<(DVec3, bevy::math::DQuat)> {
    if !tf.translation.is_finite()
        || !tf.rotation.is_finite()
        || tf.rotation.length_squared() < 1.0e-12
        || !tf.scale.is_finite()
    {
        return None;
    }
    Some((tf.translation.as_dvec3(), tf.rotation.as_dquat()))
}

/// Transfers the proxy's complete render pose into the real entity.
///
/// This is deliberately an absolute conversion, never a render-space delta:
/// render → active-frame f64 → actual parent-local `(CellCoord, Transform)`.
/// Live poses also drive Avian through `KinematicDrive`; the BigSpace physics
/// bridge remains the sole live Position/Rotation adapter. Preview poses stay
/// outside that path and update only their projected local `Transform`.
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
    q_globals: Query<&GlobalTransform, Without<GizmoProxy>>,
    mut commands: Commands,
) {
    for (tf, link, gizmo_target) in &q_proxies {
        if !gizmo_target.is_active() {
            continue;
        }
        let Ok(owner) = world.p3().get(link.target).map(|state| state.owner.clone()) else {
            continue;
        };
        if let GizmoDragOwner::UsdPreview { .. } = owner {
            let Some(local) =
                proxy_transform_to_preview_local(link.target, tf, &q_parents, &q_globals)
            else {
                continue;
            };
            if let Ok(mut target_tf) = world.p1().get_mut(link.target) {
                *target_tf = local;
            } else {
                continue;
            }
            let Some((position, rotation)) = local_transform_pose(&local) else {
                continue;
            };
            if let Ok(mut state) = world.p3().get_mut(link.target) {
                state.current_position = position;
                state.current_rotation = rotation;
                state.current_scale = Some(local.scale.as_dvec3());
            }
            continue;
        }
        let GizmoDragOwner::Live {
            active_frame: drag_frame,
            ..
        } = owner
        else {
            continue;
        };
        // A BigSpace handoff invalidates the transaction. Do not reinterpret a
        // proxy pose captured in the old semantic frame through the new frame
        // for even one interaction update; Last-stage cleanup will discard it.
        if drag_frame != active_frame.0 {
            continue;
        }
        let Ok(grid) = q_grids.get(drag_frame) else {
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
                    drag_frame,
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
                drag_frame,
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
    q_parents: Query<&ChildOf>,
    q_globals: Query<&GlobalTransform, Without<GizmoProxy>>,
    session: Res<GizmoDragSession>,
) {
    for (proxy_tf, link, gizmo_target) in &q_proxies {
        if gizmo_target.is_active() || !session.targets.contains(&link.target) {
            continue;
        }
        let Ok(mut drag) = q_drag.get_mut(link.target) else {
            continue;
        };
        let resolved = match &drag.owner {
            GizmoDragOwner::Live {
                active_frame: drag_frame,
                ..
            } => {
                if *drag_frame != active_frame.0 {
                    continue;
                }
                let Ok(grid) = q_grids.get(*drag_frame) else {
                    continue;
                };
                proxy_pose_to_active_frame(grid, proxy_tf).map(|pose| (pose, None))
            }
            GizmoDragOwner::UsdPreview { .. } => {
                let parent = q_parents
                    .get(link.target)
                    .ok()
                    .and_then(|parent| q_globals.get(parent.parent()).ok());
                preview_global_to_local_transform(proxy_tf, parent).and_then(|local| {
                    let scale = local.scale.as_dvec3();
                    local_transform_pose(&local).map(|pose| (pose, Some(scale)))
                })
            }
        };
        let Some(((position, rotation), preview_scale)) = resolved else {
            continue;
        };
        drag.current_position = position;
        drag.current_rotation = rotation;
        if let Some(scale) = preview_scale {
            drag.current_scale = Some(scale);
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

/// Resolve an active gizmo target to the focused USD preview lease.
///
/// The stage handle and exact preview hierarchy are both required. The handle
/// alone is insufficient because the same composed stage can be projected into
/// the live scene and more than one isolated preview at once.
fn preview_drag_owner(
    entity: Entity,
    viewport: Option<&UsdViewportState>,
    q_paths: &Query<&UsdPrimPath>,
    q_parents: &Query<&ChildOf>,
) -> Option<GizmoDragOwner> {
    let session = viewport?.focused_session()?;
    if !session.projection_ready() {
        return None;
    }
    let prim = q_paths.get(entity).ok()?;
    if prim.stage_handle.id() != session.stage_handle().id()
        || prim.path.is_empty()
        || !crate::ui::is_editor_preview_entity(entity, session.scene_root(), q_parents)
    {
        return None;
    }
    Some(GizmoDragOwner::UsdPreview {
        preview: session.id(),
        doc: session.doc(),
        edit_target: session.edit_target().clone(),
        generation: session.projected_generation(),
        path: prim.path.clone(),
    })
}

/// Makes the selected entity kinematic and freezes the coordinate system when gizmo drag starts.
pub fn capture_gizmo_start(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    viewport: Option<Res<UsdViewportState>>,
    q_paths: Query<&UsdPrimPath>,
    q_parents: Query<&ChildOf>,
    q_transforms: Query<&Transform, Without<GizmoProxy>>,
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
    let mut captured_live_any = false;
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

        if let Some(owner) = preview_drag_owner(entity, viewport.as_deref(), &q_paths, &q_parents) {
            let Ok(local) = q_transforms.get(entity) else {
                continue;
            };
            let Some((position, rotation)) = local_transform_pose(local) else {
                continue;
            };
            let scale = local.scale.as_dvec3();
            session.targets.insert(entity);
            info!(
                "GIZMO: USD preview drag started for {:?}, local_pos={:?}",
                entity, position
            );
            commands.entity(entity).try_insert(GizmoDragState {
                owner,
                original_position: position,
                original_rotation: rotation,
                original_scale: Some(scale),
                current_position: position,
                current_rotation: rotation,
                current_scale: Some(scale),
            });
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

        captured_live_any = true;
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
                    owner: GizmoDragOwner::Live {
                        active_frame: active_frame.0,
                        original_body,
                        original_drive,
                        had_custom_position_integration,
                        had_translation_interpolation: had_translation,
                        had_rotation_interpolation: had_rotation,
                    },
                    original_position: position.0,
                    original_rotation: rotation.0,
                    original_scale: None,
                    current_position: position.0,
                    current_rotation: rotation.0,
                    current_scale: None,
                },
            ));
    }

    if captured_live_any {
        // A selected lander is an articulation: the leg bodies are coupled to
        // the root and carry welded footpad colliders. Holding the entire physics world
        // is the only atomic capture boundary available to Avian; changing only
        // the root to Kinematic leaves live joints to integrate against a pose
        // the gizmo is mutating, which creates unbounded impulses.
        physics_holds.set(lunco_physics::PhysicsHolds::CINEMATIC, true);
    }
}

/// Finish or cancel the active gizmo transactions and restore their pre-drag
/// state. Live transactions emit the existing
/// [`lunco_scene_commands::commands::TransformEntity`] command; USD preview
/// transactions emit one existing [`lunco_usd::commands::ApplyUsdOps`]
/// change set. Each owner keeps its authoritative persistence boundary.
pub fn restore_gizmo_dynamic(
    gizmo_targets: Query<(&GizmoProxy, &GizmoTarget)>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    viewport: Option<Res<UsdViewportState>>,
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

        let (frame_changed, stale_preview) = match &drag.owner {
            GizmoDragOwner::Live {
                active_frame: drag_frame,
                ..
            } => (active_frame.0 != *drag_frame, false),
            GizmoDragOwner::UsdPreview {
                preview,
                doc,
                generation,
                ..
            } => {
                let current = viewport
                    .as_deref()
                    .and_then(|state| state.session(*preview));
                let stale = current.is_none_or(|session| {
                    session.doc() != *doc || session.projected_generation() != *generation
                });
                (false, stale)
            }
        };
        let cancel_transaction = cancelled || frame_changed || stale_preview;

        info!(
            "GIZMO: drag ended for {:?}, restoring coordinate systems",
            entity
        );

        if matches!(&drag.owner, GizmoDragOwner::UsdPreview { .. }) {
            // A document revision or preview close invalidates the captured
            // local pose. The USD projector owns the newer state, so never
            // write the stale snapshot back over it. Escape on an unchanged
            // preview is the ordinary local transaction rollback.
            if cancel_transaction && !stale_preview {
                if let Ok(mut tf) = spatial.p1().get_mut(entity) {
                    tf.translation = drag.original_position.as_vec3();
                    tf.rotation = drag.original_rotation.as_quat();
                    if let Some(scale) = drag.original_scale {
                        tf.scale = scale.as_vec3();
                    }
                }
            }

            if !cancel_transaction {
                if let GizmoDragOwner::UsdPreview {
                    doc,
                    edit_target,
                    generation,
                    path,
                    ..
                } = &drag.owner
                {
                    let mut ops = Vec::with_capacity(3);
                    if (drag.current_position - drag.original_position).length_squared() > 1.0e-12 {
                        ops.push(lunco_usd::document::UsdOp::SetTranslate {
                            edit_target: edit_target.clone(),
                            path: path.clone(),
                            value: drag.current_position.to_array(),
                        });
                    }
                    if drag.current_rotation.dot(drag.original_rotation).abs() < 1.0 - 1.0e-12 {
                        let (rx, ry, rz) = drag.current_rotation.to_euler(EulerRot::XYZ);
                        ops.push(lunco_usd::document::UsdOp::SetRotate {
                            edit_target: edit_target.clone(),
                            path: path.clone(),
                            value: [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()],
                        });
                    }
                    if let (Some(current), Some(original)) =
                        (drag.current_scale, drag.original_scale)
                    {
                        if (current - original).length_squared() > 1.0e-12 {
                            ops.push(lunco_usd::document::UsdOp::SetScale {
                                edit_target: edit_target.clone(),
                                path: path.clone(),
                                value: current.to_array(),
                            });
                        }
                    }
                    if !ops.is_empty() {
                        commands.trigger(lunco_usd::commands::ApplyUsdOps {
                            doc: *doc,
                            parent_gen: (*generation != 0).then_some(*generation),
                            label: "Edit USD transform".to_string(),
                            ops,
                        });
                    }
                }
            }
            commands.entity(entity).try_remove::<GizmoDragState>();
            restored_entities.push(entity);
            continue;
        }

        if cancel_transaction && !frame_changed {
            let rollback = {
                let q_spatial = spatial.p0();
                q_spatial.get(entity).ok().and_then(|(old_cell, _)| {
                    let GizmoDragOwner::Live { active_frame, .. } = &drag.owner else {
                        return None;
                    };
                    let (new_cell, new_translation) =
                        lunco_core::coords::position_in_grid_to_parent_local(
                            entity,
                            drag.original_position,
                            *active_frame,
                            &q_parents,
                            &q_grids,
                            &q_spatial,
                        )?;
                    let new_rotation = lunco_core::coords::rotation_in_grid_to_parent_local(
                        entity,
                        drag.original_rotation,
                        *active_frame,
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
                let GizmoDragOwner::Live { active_frame, .. } = &drag.owner else {
                    unreachable!("USD preview gizmo transactions are handled above");
                };
                warn!(
                    ?entity,
                    active_frame = ?active_frame,
                    "GIZMO: could not restore cancelled pose; completing physics cleanup without reinterpretation"
                );
            }
        }

        let GizmoDragOwner::Live {
            original_body,
            original_drive,
            had_custom_position_integration,
            had_translation_interpolation,
            had_rotation_interpolation,
            ..
        } = &drag.owner
        else {
            unreachable!("USD preview gizmo transactions are handled above");
        };

        // 2. RESTORE INTERPOLATION
        if *had_translation_interpolation {
            commands.entity(entity).try_insert(TranslationInterpolation);
        }
        if *had_rotation_interpolation {
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
        match *original_body {
            Some(body) => {
                commands.entity(entity).try_insert(body);
            }
            None => {
                commands.entity(entity).try_remove::<RigidBody>();
            }
        }
        match *original_drive {
            Some(drive) => {
                commands.entity(entity).try_insert(drive);
            }
            None => {
                commands
                    .entity(entity)
                    .try_remove::<lunco_physics::KinematicDrive>();
            }
        }
        if *had_custom_position_integration {
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
/// The raw egui focus flag is global because it protects the live scene; the
/// focused USD preview is admitted separately only when the scene-pick gate
/// assigns the pointer to its offscreen surface.
pub fn drive_gizmo_drag_no_shift(
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui_focus: Res<lunco_core::EguiFocus>,
    gate: Option<Res<ScenePickGate>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    viewport: Option<Res<UsdViewportState>>,
    panel_rects: Option<Res<PanelRects>>,
    q_targets: Query<&GizmoTarget>,
    mut drag_started: MessageWriter<GizmoDragStarted>,
    mut dragging: MessageWriter<GizmoDragging>,
) {
    let window = windows.single().ok();
    let scale_factor = window.map(Window::scale_factor).unwrap_or(1.0);
    let preview_pointer = window
        .and_then(Window::cursor_position)
        .zip(focused_preview_rect(
            viewport.as_deref(),
            panel_rects.as_deref(),
        ))
        .is_some_and(|(cursor, rect)| rect_to_logical(rect, scale_factor).contains(cursor));
    let preview_owns_pointer = preview_pointer
        && gate
            .as_deref()
            .and_then(ScenePickGate::resolved)
            .is_some_and(|target| matches!(target, SceneTarget::Offscreen(_)));
    let live_owns_pointer = !egui_focus.wants_pointer && !preview_pointer;

    if (!live_owns_pointer && !preview_owns_pointer)
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

/// Keeps `GizmoCamera` on the presentation camera that owns the focused editor.
///
/// The gizmo renders/interacts through whichever camera carries `GizmoCamera`.
/// A focused USD preview owns an offscreen camera and supplies the panel's
/// screen rectangle through `GizmoOptions::viewport_rect`; otherwise
/// `SceneViewport::active_camera` owns the standard full-window presentation.
/// This is the same frontend and the same proxy target set in both editors.
pub(crate) fn sync_gizmo_camera(
    viewport: Res<SceneViewport>,
    windows: Query<&Window, With<PrimaryWindow>>,
    q_cameras: Query<(Entity, &Camera, &RenderTarget), With<Camera3d>>,
    q_tagged: Query<Entity, With<GizmoCamera>>,
    usd_viewport: Option<Res<UsdViewportState>>,
    panel_rects: Option<Res<PanelRects>>,
    orbital_pin: Option<Res<lunco_celestial::OrbitalViewPin>>,
    mut options: ResMut<GizmoOptions>,
    mut visibility: ResMut<GizmoVisibilityState>,
    mut commands: Commands,
) {
    let preview_candidate =
        focused_preview_camera(usd_viewport.as_deref(), panel_rects.as_deref(), &q_cameras);
    // The orbital presentation lock belongs to the live scene camera. An
    // isolated USD editor has its own camera and must remain editable while a
    // mounted simulation happens to use an orbital view.
    let orbital_active =
        preview_candidate.is_none() && orbital_pin.as_ref().is_some_and(|pin| pin.active);
    if orbital_active {
        if visibility.saved_options.is_none() {
            visibility.saved_options = Some(*options);
        }
        options.gizmo_modes.clear();
        options.mode_override = None;
    } else if let Some(saved_options) = visibility.saved_options.take() {
        *options = saved_options;
    }

    let live = viewport.active_camera.and_then(|requested| {
        q_cameras
            .get(requested)
            .ok()
            .and_then(|(entity, camera, target)| {
                (camera.is_active && matches!(target, RenderTarget::Window(_))).then_some(entity)
            })
    });
    let preview = preview_candidate.filter(|_| !orbital_active);
    if preview.is_some() {
        // Scale is an authored USD transform channel. It is available only
        // while the isolated preview owns the gizmo, never on live physics
        // entities whose scale has no solver/topology contract.
        for mode in SCALE_MODES {
            options.gizmo_modes.insert(mode);
        }
    } else {
        for mode in SCALE_MODES {
            options.gizmo_modes.remove(mode);
        }
        if options.mode_override.is_some_and(is_scale_mode) {
            options.mode_override = None;
        }
    }
    let active = preview.map(|(entity, _)| entity).or(live);
    options.viewport_rect = preview.map(|(_, rect)| {
        rect_to_logical(
            rect,
            windows.single().map(Window::scale_factor).unwrap_or(1.0),
        )
    });
    // Untag any camera that is no longer the active presentation view. FALLIBLE: a scene
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

/// Return the focused preview's panel footprint in physical pixels.
///
/// `PanelRects` is cleared and repopulated by the active workbench pass, so a
/// rectangle exists only while the focused singleton or an opened view tab is
/// actually visible. The singleton is preferred when both render the focused
/// view; otherwise the focused instance supplies the footprint.
fn focused_preview_rect(
    viewport: Option<&UsdViewportState>,
    panel_rects: Option<&PanelRects>,
) -> Option<PanelRect> {
    let view = viewport?.focused_view()?;
    let rects = panel_rects?;
    rects
        .get(USD_VIEWPORT_PANEL_ID)
        .or_else(|| rects.get_instance(USD_PREVIEW_VIEW_PANEL_ID, view.id().0))
}

fn focused_preview_camera(
    viewport: Option<&UsdViewportState>,
    panel_rects: Option<&PanelRects>,
    q_cameras: &Query<(Entity, &Camera, &RenderTarget), With<Camera3d>>,
) -> Option<(Entity, PanelRect)> {
    let view = viewport?.focused_view()?;
    let rect = focused_preview_rect(viewport, panel_rects)?;
    let (entity, _camera, target) = q_cameras.get(view.camera()).ok()?;
    // Preview visibility is actuated by the USD panel measurement later in the
    // frame. At this reconciliation point `reset_preview_view_visibility` has
    // already parked every camera, so the focused view and its measured rect
    // are the authoritative presentation binding; requiring `is_active` here
    // would make the binding miss every visible preview.
    matches!(target, RenderTarget::Image(_)).then_some((entity, rect))
}

/// `Window::cursor_position` is logical, while the workbench records panel
/// geometry in physical pixels for camera/image sizing. The gizmo frontend's
/// custom viewport is also logical, so use the window's scale factor at this
/// single boundary rather than duplicating DPI conversion in each consumer.
fn rect_to_logical(rect: PanelRect, scale_factor: f32) -> Rect {
    let scale = scale_factor.max(f32::EPSILON);
    Rect::from_corners(
        Vec2::new(rect.origin.x as f32 / scale, rect.origin.y as f32 / scale),
        Vec2::new(
            rect.origin.x.saturating_add(rect.size.x) as f32 / scale,
            rect.origin.y.saturating_add(rect.size.y) as f32 / scale,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_controller::ControllerLink;
    use lunco_render::SceneCamera;

    #[test]
    fn preview_panel_rect_uses_window_logical_coordinates() {
        let rect = rect_to_logical(
            PanelRect {
                origin: UVec2::new(150, 75),
                size: UVec2::new(900, 600),
            },
            1.5,
        );

        assert_eq!(rect.min, Vec2::new(100.0, 50.0));
        assert_eq!(rect.max, Vec2::new(700.0, 450.0));
        assert!(rect.contains(Vec2::new(400.0, 250.0)));
        assert!(!rect.contains(Vec2::new(701.0, 250.0)));
    }

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
                owner: GizmoDragOwner::Live {
                    active_frame,
                    original_body: None,
                    original_drive: None,
                    had_custom_position_integration: false,
                    had_translation_interpolation: false,
                    had_rotation_interpolation: false,
                },
                original_position: DVec3::ZERO,
                original_rotation: bevy::math::DQuat::IDENTITY,
                original_scale: None,
                current_position: DVec3::ZERO,
                current_rotation: bevy::math::DQuat::IDENTITY,
                current_scale: None,
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
    fn preview_proxy_pose_uses_bevy_parent_local_transform() {
        let parent_transform =
            Transform::from_xyz(10.0, 2.0, -4.0).with_rotation(Quat::from_rotation_y(0.7));
        let local = Transform::from_xyz(1.25, -0.5, 3.0)
            .with_rotation(Quat::from_rotation_x(-0.3) * Quat::from_rotation_z(0.2));
        let proxy = (GlobalTransform::from(parent_transform) * local).compute_transform();

        let local = preview_global_to_local_transform(
            &proxy,
            Some(&GlobalTransform::from(parent_transform)),
        )
        .expect("a preview parent must provide a valid local transform");
        let (position, rotation) =
            local_transform_pose(&local).expect("reparented preview pose must be readable");

        assert!((position - local.translation.as_dvec3()).length() < 1.0e-5);
        assert!(rotation.dot(local.rotation.as_dquat()).abs() > 1.0 - 1.0e-5);
    }

    #[test]
    fn preview_proxy_pose_rejects_an_invalid_rotation() {
        let proxy = Transform {
            rotation: Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0),
            ..Transform::IDENTITY
        };

        assert!(preview_global_to_local_transform(&proxy, None)
            .and_then(|local| local_transform_pose(&local))
            .is_none());
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
                    owner: GizmoDragOwner::Live {
                        active_frame,
                        original_body: Some(RigidBody::Dynamic),
                        original_drive: None,
                        had_custom_position_integration: false,
                        had_translation_interpolation: false,
                        had_rotation_interpolation: false,
                    },
                    original_position: DVec3::ZERO,
                    original_rotation: bevy::math::DQuat::IDENTITY,
                    original_scale: None,
                    current_position: DVec3::ZERO,
                    current_rotation: bevy::math::DQuat::IDENTITY,
                    current_scale: None,
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
                    owner: GizmoDragOwner::Live {
                        active_frame,
                        original_body: None,
                        original_drive: None,
                        had_custom_position_integration: false,
                        had_translation_interpolation: false,
                        had_rotation_interpolation: false,
                    },
                    original_position: DVec3::ZERO,
                    original_rotation: bevy::math::DQuat::IDENTITY,
                    original_scale: None,
                    current_position: DVec3::ZERO,
                    current_rotation: bevy::math::DQuat::IDENTITY,
                    current_scale: None,
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
