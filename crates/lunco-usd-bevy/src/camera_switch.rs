//! Viewport active-camera switching + the viewport-camera **reconciler**.
//!
//! The scene has one main-window [`Viewport`](lunco_core::SceneViewport): it
//! owns *which* camera renders (its **active camera**), *whether* it renders
//! (visibility), and *what rect* it occupies — modelled on an Omniverse
//! Viewport. [`reconcile_scene_viewport`] is the **single authority** that
//! turns that into Bevy's per-camera `Camera::is_active` + `Camera::viewport`;
//! nothing else writes those for window cameras. Contributors only supply data:
//! - the switch here rebinds the viewport's active camera;
//! - the workbench sets visibility + rect from its layout perspective.
//!
//! This lives in `lunco-usd-bevy` (avatar-free, present in every windowed
//! binary) so switching works in a static/headless world with no avatar and no
//! input. Camera selection is an intent-level operation: a [`SceneCamera`] can
//! be selected before a render host has attached its [`RenderTarget`] (as in a
//! headless run or during projection). The renderer later binds only window
//! targets. RTT (`Image`-target) cameras and the egui `Camera2d` are never
//! selected for the main viewport.
//!
//! Switch surfaces, one mechanism — all funnel through [`ActivateCamera`] →
//! rebind [`SceneViewport::active_camera`](lunco_core::SceneViewport):
//! - [`SetActiveCamera`] — director command (API + rhai `set_camera("Name")`);
//! - [`SetUserCamera`] — explicit operator selection;
//! - [`ObserveAvatar`] / [`ResumeCameraDirector`] — explicit presentation-mode
//!   transitions;
//! - the `KeyC` hotkey ([`cycle_active_camera`]) when a host runs with input.

use bevy::camera::{RenderTarget, Viewport};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_core::{
    on_command, Command, LocalAvatar, OriginAnchor, SceneViewport, TheLocalAvatar, WorldGrid,
};
use lunco_render::SceneCamera;

use crate::UsdPrimPath;

/// Stable camera selection across re-projection. ECS entities are disposable;
/// an authored camera is identified by the composed stage plus its USD path.
#[derive(Resource, Default)]
pub struct ViewportCameraSelection {
    requested: Option<RequestedCamera>,
    owner: CameraSelectionOwner,
    /// Incremented only when the operator explicitly returns control to the
    /// authored director. Camera-track plans use it to re-emit their held cut
    /// even when the held camera name did not change.
    director_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RequestedCamera {
    Authored(UsdCameraKey),
    Entity(Entity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct UsdCameraKey {
    stage: AssetId<crate::UsdStageAsset>,
    path: String,
}

/// Who owns the current presentation selection.
///
/// This is deliberately separate from the selected entity. A scene can have
/// an authored director cut and an operator can explicitly observe the avatar
/// without either path silently taking the viewport back later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CameraSelectionOwner {
    #[default]
    None,
    Director,
    User,
}

/// Change-gated view model for the Camera menu and no-camera presentation.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraSelectionStatus {
    /// Window-targeting USD cameras, sorted by display name.
    pub cameras: Vec<String>,
    pub active_name: Option<String>,
    pub owner: CameraSelectionOwner,
    pub avatar_available: bool,
    pub director_available: bool,
    /// A failed explicit request remains visible until the next successful
    /// request or scene teardown. It is not converted into another camera.
    pub last_error: Option<String>,
}

/// Event-like invalidation for consumers of the current camera fact.
///
/// The camera exposure producer subscribes to this boundary. Camera status is
/// therefore published when a camera-selection lifecycle event changes the
/// status, rather than being rediscovered by a render/update tick.
#[derive(Event, Clone, Copy, Debug, Default)]
pub struct CameraSelectionStatusChanged;

/// Mandatory authored presentation contract for a windowed scene.
///
/// The render host opts into `required`. The USD projection owns the verdict:
/// exactly one authored camera track must resolve to authored scene cameras.
/// A missing or invalid contract is not repaired by choosing a camera; the
/// scene admission owner can reject it and the UI can highlight the finding.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraContractStatus {
    /// Whether the current host requires a presentable authored scene.
    pub required: bool,
    /// Whether the current authored contract has passed validation.
    pub ready: bool,
    /// Stable owning errors for the current scene contract.
    pub errors: Vec<String>,
}

impl ViewportCameraSelection {
    /// Revision observed by the authored camera-track sampler.
    pub(crate) fn director_revision(&self) -> u64 {
        self.director_revision
    }

    pub fn owner(&self) -> CameraSelectionOwner {
        self.owner
    }
}

/// Switch the viewport's active camera to the `SceneCamera` whose `Name` matches.
///
/// Works with no avatar present. `name` matches the full USD prim path *or*
/// its leaf, so a cutscene can `set_camera("ChaseCam")` to reach
/// `/World/Rover/ChaseCam`, or `set_camera("WideShot")` for a scene camera.
#[Command(default)]
pub struct SetActiveCamera {
    /// Camera name (full USD prim path or its leaf).
    pub name: String,
}

/// Explicit operator selection of a named authored camera.
///
/// Unlike [`SetActiveCamera`], this takes ownership from the authored director
/// until [`ResumeCameraDirector`] is requested.
#[Command(default)]
pub struct SetUserCamera {
    /// Camera name (full USD prim path or its leaf).
    pub name: String,
}

/// Explicitly show the local avatar camera.
#[Command(default)]
pub struct ObserveAvatar {}

/// Return presentation ownership to the authored camera director.
#[Command(default)]
pub struct ResumeCameraDirector {}

/// Internal trigger: bind `.0` as the viewport's active camera. Both the
/// name-based command and the cycle hotkey resolve to an entity and fire this,
/// so the binding is written in exactly one observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraActivationSource {
    Director,
    User,
}

#[derive(Event)]
pub struct ActivateCamera {
    pub target: Entity,
    pub source: CameraActivationSource,
}

impl ActivateCamera {
    pub fn director(target: Entity) -> Self {
        Self {
            target,
            source: CameraActivationSource::Director,
        }
    }

    pub fn user(target: Entity) -> Self {
        Self {
            target,
            source: CameraActivationSource::User,
        }
    }
}

pub(crate) fn resolve_camera_names(
    want: &str,
    cameras: &[(Entity, String)],
) -> Result<Entity, String> {
    let exact: Vec<Entity> = cameras
        .iter()
        .filter_map(|(entity, name)| (name == want).then_some(*entity))
        .collect();
    match exact.as_slice() {
        [entity] => return Ok(*entity),
        [] => {}
        _ => {
            return Err(format!(
                "camera path '{want}' is ambiguous; more than one scene camera has this path"
            ));
        }
    }

    let matches: Vec<(Entity, &str)> = cameras
        .iter()
        .filter_map(|(entity, name)| {
            (name.rsplit('/').next() == Some(want)).then_some((*entity, name.as_str()))
        })
        .collect();
    match matches.as_slice() {
        [] => Err(format!("camera '{want}' is not present in the scene")),
        [(entity, _)] => Ok(*entity),
        _ => {
            let names = matches
                .iter()
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "camera name '{want}' is ambiguous; use a full USD path ({names})"
            ))
        }
    }
}

pub(crate) fn resolve_named_camera(
    want: &str,
    q_cams: &Query<(Entity, &Name), With<SceneCamera>>,
) -> Result<Entity, String> {
    let cameras: Vec<(Entity, String)> = q_cams
        .iter()
        .map(|(entity, name)| (entity, name.as_str().to_string()))
        .collect();
    resolve_camera_names(want, &cameras)
}

fn record_camera_error(
    status: &mut CameraSelectionStatus,
    message: String,
    commands: &mut Commands,
) {
    if status.last_error.as_deref() != Some(message.as_str()) {
        status.last_error = Some(message);
        commands.trigger(CameraSelectionStatusChanged);
    }
}

fn clear_camera_error(status: &mut CameraSelectionStatus, commands: &mut Commands) {
    if status.last_error.take().is_some() {
        commands.trigger(CameraSelectionStatusChanged);
    }
}

fn is_window_render_target(target: &RenderTarget) -> bool {
    matches!(target, RenderTarget::Window(_))
}

/// Command handler: resolve `SetActiveCamera.name` → a camera entity and fire
/// [`ActivateCamera`]. Matches the full USD prim path *or* its leaf.
#[on_command(SetActiveCamera)]
pub fn on_set_active_camera(
    trigger: On<SetActiveCamera>,
    q_cams: Query<(Entity, &Name), With<SceneCamera>>,
    selection: Res<ViewportCameraSelection>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    if selection.owner() == CameraSelectionOwner::User {
        info!(
            "[camera] director request held while operator owns the viewport; use ResumeCameraDirector to return control"
        );
        return;
    }
    let want = trigger.event().name.trim();
    match resolve_named_camera(want, &q_cams) {
        Ok(target) => commands.trigger(ActivateCamera::director(target)),
        Err(message) => {
            record_camera_error(&mut status, message.clone(), &mut commands);
            warn!("[camera] {message}");
        }
    }
}

#[on_command(SetUserCamera)]
pub fn on_set_user_camera(
    trigger: On<SetUserCamera>,
    q_cams: Query<(Entity, &Name), With<SceneCamera>>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    let want = trigger.event().name.trim();
    match resolve_named_camera(want, &q_cams) {
        Ok(target) => commands.trigger(ActivateCamera::user(target)),
        Err(message) => {
            record_camera_error(&mut status, message.clone(), &mut commands);
            warn!("[camera] {message}");
        }
    }
}

#[on_command(ObserveAvatar)]
pub fn on_observe_avatar(_trigger: On<ObserveAvatar>, mut commands: Commands) {
    commands.trigger(lunco_core::RequestLocalAvatarView);
}

/// Resolve the shared avatar-return intent. Avatar mechanics and the UI use
/// this same path, so neither can clear the viewport and accidentally leave a
/// director camera selected behind the scenes.
pub fn on_request_local_avatar_view(
    _trigger: On<lunco_core::RequestLocalAvatarView>,
    local_avatar: Res<TheLocalAvatar>,
    q_cameras: Query<(), With<SceneCamera>>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    let target = local_avatar.0.filter(|entity| q_cameras.contains(*entity));
    let Some(target) = target else {
        let message = "the scene has no local avatar camera to observe".to_string();
        record_camera_error(&mut status, message.clone(), &mut commands);
        warn!("[camera] {message}");
        return;
    };
    commands.trigger(ActivateCamera::user(target));
}

#[on_command(ResumeCameraDirector)]
pub fn on_resume_camera_director(
    _trigger: On<ResumeCameraDirector>,
    q_tracks: Query<(), With<crate::camera_track::CameraTrackPlan>>,
    mut selection: ResMut<ViewportCameraSelection>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    selection.owner = CameraSelectionOwner::Director;
    selection.requested = None;
    selection.director_revision = selection.director_revision.wrapping_add(1);
    if q_tracks.is_empty() {
        let message = "the scene has no authored CameraTrack to resume".to_string();
        record_camera_error(&mut status, message.clone(), &mut commands);
        warn!("[camera] {message}");
    } else {
        clear_camera_error(&mut status, &mut commands);
    }
    info!("[camera] presentation ownership → authored director");
}

/// `KeyC`: advance the viewport's active camera to the next window camera
/// (stable order by `Name`, wrapping). No-op with fewer than two window
/// cameras or no input. "Current" is the viewport binding, not raw `is_active`
/// (which the visibility gate may have cleared).
pub fn cycle_active_camera(
    // Optional: a static/headless world has no `ButtonInput` resource (no input
    // plugin). It simply never cycles — the command path still works there.
    keys: Option<Res<ButtonInput<KeyCode>>>,
    vp: Res<SceneViewport>,
    q_cams: Query<(Entity, &RenderTarget, &Name), With<SceneCamera>>,
    mut commands: Commands,
) {
    let Some(keys) = keys else {
        return;
    };
    if !keys.just_pressed(KeyCode::KeyC) {
        return;
    }
    // Don't hijack modified chords (Ctrl+C copy, etc.).
    if keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::AltLeft,
        KeyCode::AltRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
    ]) {
        return;
    }

    let mut cams: Vec<(Entity, &str)> = q_cams
        .iter()
        .filter(|(_, target, _)| is_window_render_target(target))
        .map(|(e, _, name)| (e, name.as_str()))
        .collect();
    if cams.len() < 2 {
        return;
    }
    cams.sort_by(|a, b| a.1.cmp(b.1));
    let Some(cur) = vp
        .active_camera
        .and_then(|active| cams.iter().position(|(e, _)| *e == active))
    else {
        // Cycling is an operator action over an existing viewport binding. It
        // must not turn a missing/stale binding into an implicit first-camera
        // selection; the authored director or an explicit camera command owns
        // initial presentation.
        return;
    };
    let next = cams[(cur + 1) % cams.len()].0;
    commands.trigger(ActivateCamera::user(next));
}

/// Rebind the viewport's active camera. The reconciler actuates
/// `is_active`/`viewport` from this — this observer never touches cameras
/// directly (single-writer discipline).
pub fn on_activate_camera(
    trigger: On<ActivateCamera>,
    q_cams: Query<(Option<&RenderTarget>, Option<&UsdPrimPath>), With<SceneCamera>>,
    q_identity: Query<(Option<&Name>, Option<&UsdPrimPath>, Has<SceneCamera>)>,
    mut selection: ResMut<ViewportCameraSelection>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    let event = trigger.event();
    let target = event.target;
    match q_cams.get(target) {
        // SceneCamera is the render-free intent marker. A missing target is
        // valid in --no-ui/headless runs and while the render host is still
        // binding a projected camera; the reconciler will act when the
        // window-targeted pipeline exists.
        Ok((None, path)) | Ok((Some(RenderTarget::Window(_)), path)) => {
            selection.requested = Some(match path {
                Some(path) => RequestedCamera::Authored(UsdCameraKey {
                    stage: path.stage_handle.id(),
                    path: path.path.clone(),
                }),
                None => RequestedCamera::Entity(target),
            });
            selection.owner = match event.source {
                CameraActivationSource::Director => CameraSelectionOwner::Director,
                CameraActivationSource::User => CameraSelectionOwner::User,
            };
            clear_camera_error(&mut status, &mut commands);
            info!(
                "[camera] viewport → {target:?} (owner={:?})",
                selection.owner
            );
        }
        Ok((Some(_), _)) => {
            let message = format!("camera {target:?} is not a window camera");
            record_camera_error(&mut status, message.clone(), &mut commands);
            warn!("[camera] {message}");
        }
        Err(_) => {
            let identity = q_identity
                .get(target)
                .ok()
                .map(|(name, path, scene_camera)| {
                    let name = name.map(Name::as_str).unwrap_or("unnamed");
                    let path = path.map(|path| path.path.as_str()).unwrap_or("no USD path");
                    format!("{name} ({path}, scene_camera={scene_camera})")
                })
                .unwrap_or_else(|| "entity no longer exists".to_string());
            let message = format!("camera {target:?} ({identity}) is not a SceneCamera");
            record_camera_error(&mut status, message.clone(), &mut commands);
            warn!("[camera] {message}");
        }
    }
}

/// The **single authority** over window-camera `is_active` + `viewport`.
///
/// Reads the [`SceneViewport`] (active-camera binding + visibility + rect) and
/// actuates it: exactly the bound camera is active (and only when visible); all
/// other window cameras are off. RTT (`Image`-target) cameras are ignored.
/// Also updates the persistent BigSpace [`OriginAnchor`] to the active camera's
/// `WorldGrid` cell using the authoritative f64 hierarchy pose. The camera is
/// a render consumer; it never owns the origin marker.
///
/// There is deliberately no implicit camera selection here. If the selection
/// is absent, stale, or still waiting for its authored camera to finish
/// projection, every window camera is inactive and the status view model says
/// why. A camera-less scene is an explicit no-camera state, not an engine view.
/// This reconciler only fulfils an explicit request and never chooses a
/// different camera itself.
pub fn reconcile_scene_viewport(
    mut vp: ResMut<SceneViewport>,
    selection: Res<ViewportCameraSelection>,
    mut q_cams: Query<
        (
            Entity,
            &mut Camera,
            &RenderTarget,
            Has<bevy::camera::Projection>,
            Option<&UsdPrimPath>,
            Has<SceneCamera>,
        ),
        With<Camera3d>,
    >,
) {
    // A camera is only ACTIVATABLE once its 3D pipeline (`Camera3d` → required
    // `Projection`) is bound by `lunco-render-bevy`. A `SceneCamera` spawns as a
    // bare `Camera` and gets that pipeline a frame or two later; if we activate it
    // in that window it is extracted as a live view but SKIPPED by Bevy's
    // `build_directional_light_cascades` (whose query requires `&Projection`), so a
    // shadow-casting sun's `prepare_lights` then `unwrap()`s a cascade map with no
    // entry for the view and PANICS the render app. It only bites a scene whose sun
    // has shadows enabled (e.g. the moonbase `DistantLight`) — a shadowless sandbox
    // sun skips the cascade path — which is exactly the "headfull crashes on the DEM
    // project, not the flat sandbox" symptom. Requiring `Projection` here keeps the
    // sole active window view always cascade-covered and forces any transient
    // projectionless camera off.
    let activatable = |q: &Query<
        (
            Entity,
            &mut Camera,
            &RenderTarget,
            Has<bevy::camera::Projection>,
            Option<&UsdPrimPath>,
            Has<SceneCamera>,
        ),
        With<Camera3d>,
    >,
                       e: Entity|
     -> bool {
        q.get(e).is_ok_and(|(_, _, t, has_proj, _, scene_camera)| {
            is_window_render_target(t) && has_proj && scene_camera
        })
    };

    // ── Resolve only the explicit request ───────────────────────────────
    let active = selection.requested.as_ref().and_then(|requested| {
        let entity = match requested {
            RequestedCamera::Entity(entity) => Some(*entity),
            RequestedCamera::Authored(wanted) => q_cams
                .iter()
                .find(|(_, _, _, _, path, scene_camera)| {
                    *scene_camera
                        && path.is_some_and(|path| {
                            path.stage_handle.id() == wanted.stage && path.path == wanted.path
                        })
                })
                .map(|(entity, _, _, _, _, _)| entity),
        }?;
        activatable(&q_cams, entity).then_some(entity)
    });
    if vp.active_camera != active {
        vp.active_camera = active;
    }

    let visible = vp.visible;
    let rect = vp.rect;

    // ── Actuate: the ONE writer of window-camera is_active + viewport ────
    for (e, mut cam, target, _, _, _) in q_cams.iter_mut() {
        if !is_window_render_target(target) {
            continue; // RTT/offscreen cameras are self-managed
        }
        // `active` is already Projection-gated, so a projectionless camera is
        // never `active` → want_active=false → it is (kept) off until its 3D
        // pipeline binds. That is the guard against the cascade-unwrap panic.
        let want_active = Some(e) == active && visible;
        if cam.is_active != want_active {
            cam.is_active = want_active;
        }
        let want_vp = if Some(e) == active {
            rect.map(|(pos, size)| Viewport {
                physical_position: pos,
                physical_size: size,
                ..default()
            })
        } else {
            None
        };
        // Compare pos+size only (Viewport's `depth: Range<f32>` isn't `Eq`).
        let same = match (&cam.viewport, &want_vp) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.physical_position == b.physical_position && a.physical_size == b.physical_size
            }
            _ => false,
        };
        if !same {
            cam.viewport = want_vp;
        }
    }
}

/// Project the selected camera into the persistent origin frame.
///
/// This runs after every camera pose writer, including mounted followers,
/// avatar rigs, and cinematic paths, but before BigSpace recenters and
/// propagates transforms. The camera pose is composed in f64 and split exactly
/// once in the persistent `WorldGrid`; no camera-relative `GlobalTransform` is
/// read and no camera receives `FloatingOrigin`.
pub fn update_camera_origin(
    vp: Res<SceneViewport>,
    q_grids: Query<&Grid>,
    q_world_grid: Query<Entity, With<WorldGrid>>,
    mut q_origin: Query<(&mut CellCoord, &mut Transform), With<OriginAnchor>>,
    q_parents: Query<&ChildOf>,
    q_spatial: Query<(Option<&CellCoord>, &Transform), Without<OriginAnchor>>,
    diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    // `FloatingOrigin` is owned by `OriginAnchor`, which is a valid Grid
    // archetype. The camera pose is composed in f64 and split exactly once in
    // the persistent `WorldGrid`; no camera-relative GlobalTransform is read.
    let mut origin_errors = Vec::new();
    let world_grids: Vec<Entity> = q_world_grid.iter().collect();
    if let Ok((mut origin_cell, mut origin_transform)) = q_origin.single_mut() {
        if let [world_grid] = world_grids.as_slice() {
            if let Some(active) = vp.active_camera {
                if let Some((camera_position, _camera_rotation)) = lunco_core::coords::pose_in_grid(
                    active,
                    *world_grid,
                    &q_parents,
                    &q_grids,
                    &q_spatial,
                ) {
                    if let Ok(world_grid_component) = q_grids.get(*world_grid) {
                        let (new_cell, new_translation) =
                            world_grid_component.translation_to_grid(camera_position);
                        if *origin_cell != new_cell {
                            *origin_cell = new_cell;
                        }
                        if origin_transform.translation != new_translation
                            || origin_transform.rotation != Quat::IDENTITY
                            || origin_transform.scale != Vec3::ONE
                        {
                            *origin_transform = Transform::from_translation(new_translation);
                        }
                    } else {
                        *origin_cell = CellCoord::default();
                        *origin_transform = Transform::IDENTITY;
                        origin_errors.push(
                            "[camera-origin] the active WorldGrid has no BigSpace Grid component"
                                .to_string(),
                        );
                    }
                } else {
                    *origin_cell = CellCoord::default();
                    *origin_transform = Transform::IDENTITY;
                    origin_errors.push(format!(
                        "[camera-origin] active camera {active:?} has no complete f64 pose in WorldGrid"
                    ));
                }
            } else {
                *origin_cell = CellCoord::default();
                *origin_transform = Transform::IDENTITY;
            }
            // The origin tracker is a frame marker, not scene content. Keep its
            // rotation and scale canonical, while retaining the f32 remainder
            // returned by the same f64-to-grid split as the cell. Together the
            // `(CellCoord, Transform)` pair places the render origin at the
            // selected pose rather than at the nearest cell centre.
            if origin_transform.rotation != Quat::IDENTITY || origin_transform.scale != Vec3::ONE {
                origin_transform.rotation = Quat::IDENTITY;
                origin_transform.scale = Vec3::ONE;
            }
        } else {
            *origin_cell = CellCoord::default();
            *origin_transform = Transform::IDENTITY;
        }
        if vp.active_camera.is_some() && world_grids.len() != 1 {
            origin_errors.push(
                format!(
                    "[camera-origin] the persistent WorldGrid contract requires exactly one entity, found {}",
                    world_grids.len()
                ),
            );
        }
    }
    if let Some(mut diagnostics) = diagnostics {
        let findings: Vec<lunco_core::RuntimeDiagnostic> = origin_errors
            .iter()
            .map(|message| lunco_core::RuntimeDiagnostic {
                code: "camera-origin".to_string(),
                severity: lunco_core::DiagnosticSeverity::Error,
                producer: "camera-origin".to_string(),
                subject: "viewport-origin".to_string(),
                message: message.clone(),
            })
            .collect();
        diagnostics.replace_producer("camera-origin", findings);
    }
}

/// Rebuild the camera menu/no-camera view model from the live projection.
/// This is the only world scan used by the menu; the menu itself reads this
/// already-shaped resource and emits typed commands.
pub fn update_camera_selection_status(
    selection: Res<ViewportCameraSelection>,
    vp: Res<SceneViewport>,
    q_cams: Query<(Entity, &Name, &RenderTarget, Has<LocalAvatar>), With<SceneCamera>>,
    q_tracks: Query<(), With<crate::camera_track::CameraTrackPlan>>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    let mut cameras: Vec<(Entity, String, bool)> = q_cams
        .iter()
        .filter(|(_, _, target, _)| is_window_render_target(target))
        .map(|(entity, name, _, avatar)| (entity, name.as_str().to_string(), avatar))
        .collect();
    cameras.sort_by(|a, b| a.1.cmp(&b.1));
    let active_name = vp.active_camera.and_then(|active| {
        cameras
            .iter()
            .find(|(entity, _, _)| *entity == active)
            .map(|(_, name, _)| name.clone())
    });
    let next = CameraSelectionStatus {
        cameras: cameras.iter().map(|(_, name, _)| name.clone()).collect(),
        active_name,
        owner: selection.owner,
        avatar_available: cameras.iter().any(|(_, _, avatar)| *avatar),
        director_available: !q_tracks.is_empty(),
        last_error: status.last_error.clone(),
    };
    if *status != next {
        *status = next;
        commands.trigger(CameraSelectionStatusChanged);
    }
}

/// Run the camera status projection only after one of its inputs changed.
///
/// This is deliberately a change detector, not a render-frame camera scan.
/// Camera projection emits entity lifecycle changes, viewport selection and
/// ownership are resources, and the scene camera's name/target/local-avatar
/// markers are component changes. The status resource is then the event-like
/// boundary consumed by engine exposures and UI.
pub fn camera_selection_status_changed(
    selection: Res<ViewportCameraSelection>,
    viewport: Res<SceneViewport>,
    cameras: Query<
        (),
        (
            With<SceneCamera>,
            Or<(
                Added<SceneCamera>,
                Changed<Name>,
                Changed<RenderTarget>,
                Changed<LocalAvatar>,
            )>,
        ),
    >,
    tracks: Query<(), Added<crate::camera_track::CameraTrackPlan>>,
    removed_cameras: RemovedComponents<SceneCamera>,
    removed_tracks: RemovedComponents<crate::camera_track::CameraTrackPlan>,
) -> bool {
    selection.is_changed()
        || viewport.is_changed()
        || !cameras.is_empty()
        || !tracks.is_empty()
        || !removed_cameras.is_empty()
        || !removed_tracks.is_empty()
}

/// Validate the authored window presentation contract after USD camera-track
/// plans are derived. This is a structural admission check, not a policy lint:
/// duplicate tracks, absent cameras, unresolved names, and multiple mounted
/// stages are errors owned by the camera domain.
pub fn validate_authored_camera_contract(
    mount: Res<lunco_core::SceneMountState>,
    scene_roots: Query<Has<crate::UsdVisualSynced>, With<crate::UsdSceneRoot>>,
    pending_projection: Query<Entity, With<crate::UsdAwaitingStage>>,
    q_scene_root: Query<(), With<crate::UsdSceneRoot>>,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    tracks: Query<
        (&UsdPrimPath, Option<&crate::camera_track::CameraTrackPlan>),
        With<crate::camera_track::CameraTrack>,
    >,
    cameras: Query<(Entity, &Name, &UsdPrimPath, Has<LocalAvatar>), With<SceneCamera>>,
    selection: Res<ViewportCameraSelection>,
    mut commands: Commands,
    mut contract: ResMut<CameraContractStatus>,
    mut status: ResMut<CameraSelectionStatus>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if !contract.required {
        request_authored_local_avatar_view(&cameras, &tracks, &selection, &mut commands);
        publish_camera_contract_diagnostics(&mut diagnostics, &[]);
        if !contract.errors.is_empty() || !contract.ready {
            *contract = CameraContractStatus {
                ready: true,
                ..default()
            };
        }
        if status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("[camera-contract]"))
        {
            clear_camera_error(&mut status, &mut commands);
        }
        return;
    }
    if mount.active_root().is_none() {
        publish_camera_contract_diagnostics(&mut diagnostics, &[]);
        if !contract.errors.is_empty() || contract.ready {
            *contract = CameraContractStatus {
                required: contract.required,
                ..default()
            };
        }
        if status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("[camera-contract]"))
        {
            clear_camera_error(&mut status, &mut commands);
        }
        return;
    }

    let active_root = mount.active_root();
    let projection_pending = active_root.is_some_and(|root| {
        !scene_roots.get(root).is_ok_and(|synced| synced)
            || pending_projection.iter().any(|entity| {
                crate::scene_root_ancestor(entity, &q_scene_root, &q_child_of, &q_entities)
                    .ok()
                    .flatten()
                    == Some(root)
            })
    });
    if projection_pending {
        // USD projection is an explicit lifecycle phase. Do not turn the
        // interval before authored camera entities exist into a contract
        // failure; once the active root is fully projected, the same validator
        // reports a real missing/invalid camera contract as an Error.
        publish_camera_contract_diagnostics(&mut diagnostics, &[]);
        if contract.required || contract.ready || !contract.errors.is_empty() {
            *contract = CameraContractStatus::default();
        }
        if status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("[camera-contract]"))
        {
            clear_camera_error(&mut status, &mut commands);
        }
        return;
    }

    request_authored_local_avatar_view(&cameras, &tracks, &selection, &mut commands);

    let mut stage_ids = std::collections::BTreeSet::new();
    let mut camera_names = Vec::new();
    let mut local_avatar_names = Vec::new();
    for (entity, name, prim, local_avatar) in &cameras {
        stage_ids.insert(prim.stage_handle.id());
        camera_names.push((entity, name.as_str().to_string()));
        if local_avatar {
            local_avatar_names.push(name.as_str().to_string());
        }
    }
    for (prim, _) in &tracks {
        stage_ids.insert(prim.stage_handle.id());
    }

    let mut errors = Vec::new();
    if stage_ids.len() > 1 {
        errors.push(
            "[camera-contract] multiple USD stages provide one viewport; author explicit viewport scopes"
                .to_string(),
        );
    }
    if cameras.is_empty() {
        errors.push(
            "[camera-contract] scene has no authored SceneCamera for the window presentation"
                .to_string(),
        );
    }
    if tracks.is_empty() {
        match local_avatar_names.as_slice() {
            [_] => {}
            [] => errors.push(
                "[camera-contract] scene has no authored CameraTrack or LocalAvatar initial presentation"
                    .to_string(),
            ),
            names => errors.push(format!(
                "[camera-contract] scene has multiple LocalAvatar initial presentations: {}",
                names.join(", ")
            )),
        }
    } else if tracks.iter().count() > 1 {
        errors.push(
            "[camera-contract] scene has multiple CameraTrack providers without an explicit viewport scope"
                .to_string(),
        );
    }

    for (prim, plan) in &tracks {
        let Some(plan) = plan else {
            errors.push(format!(
                "[camera-contract] CameraTrack '{}' has not finished projection",
                prim.path
            ));
            continue;
        };
        if plan.keys.is_empty() {
            errors.push(format!(
                "[camera-contract] CameraTrack '{}' has no activeCamera keys",
                prim.path
            ));
        }
        for (_, want) in &plan.keys {
            if let Err(reason) = resolve_camera_names(want, &camera_names) {
                errors.push(format!(
                    "[camera-contract] CameraTrack '{}' cannot resolve '{want}': {reason}",
                    prim.path
                ));
            }
        }
    }

    let ready = errors.is_empty();
    publish_camera_contract_diagnostics(&mut diagnostics, &errors);
    let changed = contract.ready != ready || contract.errors != errors;
    contract.ready = ready;
    contract.errors = errors;
    if changed {
        if let Some(error) = contract.errors.first() {
            record_camera_error(&mut status, error.clone(), &mut commands);
            error!("[camera] {error}");
        } else if status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("[camera-contract]"))
        {
            clear_camera_error(&mut status, &mut commands);
        }
    }
}

/// A scene with no cinematic track may still have an authored LocalAvatar camera.
/// That camera is the initial presentation owner for both the windowed viewport and
/// offscreen recording. Bind it once it is projected; never choose by ECS order.
fn request_authored_local_avatar_view(
    cameras: &Query<(Entity, &Name, &UsdPrimPath, Has<LocalAvatar>), With<SceneCamera>>,
    tracks: &Query<
        (&UsdPrimPath, Option<&crate::camera_track::CameraTrackPlan>),
        With<crate::camera_track::CameraTrack>,
    >,
    selection: &Res<ViewportCameraSelection>,
    commands: &mut Commands,
) {
    if !tracks.is_empty() || selection.requested.is_some() {
        return;
    }
    let mut local_avatars = cameras
        .iter()
        .filter_map(|(entity, _, _, local)| local.then_some(entity));
    let Some(target) = local_avatars.next() else {
        return;
    };
    if local_avatars.next().is_none() {
        commands.trigger(ActivateCamera::user(target));
    }
}

fn publish_camera_contract_diagnostics(
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    errors: &[String],
) {
    let Some(diagnostics) = diagnostics.as_deref_mut() else {
        return;
    };
    let findings = errors.iter().map(|message| lunco_core::RuntimeDiagnostic {
        code: "camera-contract".to_string(),
        severity: lunco_core::DiagnosticSeverity::Error,
        producer: "usd-camera".to_string(),
        subject: "window-presentation".to_string(),
        message: message.clone(),
    });
    diagnostics.replace_producer("usd-camera", findings);
}

/// Scene teardown is the ownership boundary for camera selection. A stale
/// authored key must not select a camera from the next scene.
pub fn reset_camera_selection(
    mut selection: ResMut<ViewportCameraSelection>,
    mut viewport: ResMut<SceneViewport>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    *selection = ViewportCameraSelection::default();
    viewport.active_camera = None;
    if *status != CameraSelectionStatus::default() {
        *status = CameraSelectionStatus::default();
        commands.trigger(CameraSelectionStatusChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_cam(is_active: bool, name: &str) -> impl Bundle {
        (
            SceneCamera::default(),
            Camera3d::default(),
            Camera {
                is_active,
                ..default()
            },
            // A `Projection` stands in for the bound 3D pipeline: the reconciler
            // only activates cameras whose pipeline is present (guards the shadow
            // cascade-unwrap panic), so a test camera must carry one to be eligible.
            bevy::camera::Projection::default(),
            RenderTarget::Window(bevy::window::WindowRef::Primary),
            Name::new(name.to_string()),
        )
    }

    fn active_set(app: &mut App) -> Vec<Entity> {
        let mut q = app
            .world_mut()
            .query_filtered::<(Entity, &Camera), With<SceneCamera>>();
        q.iter(app.world())
            .filter(|(_, c)| c.is_active)
            .map(|(e, _)| e)
            .collect()
    }

    #[test]
    fn selected_camera_projects_origin_anchor_without_claiming_floating_origin() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<lunco_core::RuntimeDiagnostics>();
        let world_grid = lunco_core::ensure_world_root(app.world_mut());
        let camera = app
            .world_mut()
            .spawn((
                Transform::from_translation(Vec3::new(321.25, -44.5, 17.75)),
                GlobalTransform::default(),
                CellCoord::new(12_345, -7, 3),
                ChildOf(world_grid),
            ))
            .id();
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(camera);
        app.add_systems(Update, update_camera_origin);

        app.update();

        let mut q_anchor = app.world_mut().query_filtered::<(
            &CellCoord,
            &Transform,
            &Grid,
            Has<big_space::prelude::FloatingOrigin>,
        ), With<OriginAnchor>>();
        let (anchor_cell, anchor_transform, _, anchor_has_origin) =
            q_anchor.single(app.world()).unwrap();
        assert_eq!(*anchor_cell, CellCoord::new(12_345, -7, 3));
        assert_eq!(
            anchor_transform.translation,
            Vec3::new(321.25, -44.5, 17.75)
        );
        assert!(anchor_has_origin);
        assert!(app
            .world()
            .get::<big_space::prelude::FloatingOrigin>(camera)
            .is_none());

        assert!(app
            .world()
            .resource::<lunco_core::RuntimeDiagnostics>()
            .findings
            .is_empty());
    }

    /// The reconciler activates exactly the bound camera and deactivates every
    /// other window camera — even stray ones spawned active.
    #[test]
    fn reconciler_activates_only_the_bound_camera() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let _a = app.world_mut().spawn(window_cam(true, "A")).id();
        let b = app.world_mut().spawn(window_cam(false, "B")).id();
        let _c = app.world_mut().spawn(window_cam(true, "C")).id(); // stray active
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(b);
        app.world_mut()
            .resource_mut::<ViewportCameraSelection>()
            .requested = Some(RequestedCamera::Entity(b));

        app.update();

        assert_eq!(
            active_set(&mut app),
            vec![b],
            "only the bound camera renders"
        );
    }

    #[test]
    fn reconciler_deactivates_a_stale_window_camera_without_scene_intent() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let active = app.world_mut().spawn(window_cam(false, "active")).id();
        let orphan = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    is_active: true,
                    ..default()
                },
                Projection::default(),
                RenderTarget::Window(bevy::window::WindowRef::Primary),
                Name::new("orphan"),
            ))
            .id();
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(active);
        app.world_mut()
            .resource_mut::<ViewportCameraSelection>()
            .requested = Some(RequestedCamera::Entity(active));

        app.update();

        assert!(app.world().get::<Camera>(active).unwrap().is_active);
        assert!(!app.world().get::<Camera>(orphan).unwrap().is_active);
    }

    /// When the viewport is not visible (workbench Design perspective), no
    /// window camera renders — but the binding is preserved for restore.
    #[test]
    fn invisible_viewport_deactivates_all() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let b = app.world_mut().spawn(window_cam(true, "B")).id();
        {
            let mut vp = app.world_mut().resource_mut::<SceneViewport>();
            vp.active_camera = Some(b);
            vp.visible = false;
        }
        app.world_mut()
            .resource_mut::<ViewportCameraSelection>()
            .requested = Some(RequestedCamera::Entity(b));

        app.update();

        assert!(
            active_set(&mut app).is_empty(),
            "nothing renders while hidden"
        );
        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            Some(b),
            "binding preserved across a hide"
        );
    }

    #[test]
    fn authored_camera_selection_survives_entity_reprojection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let stage = Handle::<crate::UsdStageAsset>::default();
        let path = "/Scene/Wide";
        let old = app
            .world_mut()
            .spawn((
                window_cam(false, "Wide"),
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: path.into(),
                },
            ))
            .id();
        {
            let mut selection = app.world_mut().resource_mut::<ViewportCameraSelection>();
            selection.requested = Some(RequestedCamera::Authored(UsdCameraKey {
                stage: stage.id(),
                path: path.into(),
            }));
        }
        app.world_mut().despawn(old);
        let replacement = app
            .world_mut()
            .spawn((
                window_cam(false, "Wide"),
                UsdPrimPath {
                    stage_handle: stage,
                    path: path.into(),
                },
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            Some(replacement),
            "the persistent USD key resolves to the reprojected entity"
        );
        assert_eq!(active_set(&mut app), vec![replacement]);
    }

    /// Selection is valid before the render host binds a target. This is the
    /// normal representation of authored cameras in a headless simulation;
    /// the render reconciler will consume the stable USD key if a window host
    /// is present later.
    #[test]
    fn render_free_authored_camera_is_a_valid_selection_intent() {
        let mut app = App::new();
        app.init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera);
        let stage = Handle::<crate::UsdStageAsset>::default();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                Name::new("Wide"),
                UsdPrimPath {
                    stage_handle: stage.clone(),
                    path: "/Scene/Wide".into(),
                },
            ))
            .id();

        app.world_mut().trigger(ActivateCamera::director(camera));

        let selection = app.world().resource::<ViewportCameraSelection>();
        assert_eq!(selection.owner, CameraSelectionOwner::Director);
        assert_eq!(
            selection.requested,
            Some(RequestedCamera::Authored(UsdCameraKey {
                stage: stage.id(),
                path: "/Scene/Wide".into(),
            }))
        );
        assert_eq!(
            app.world().resource::<CameraSelectionStatus>().last_error,
            None
        );
    }

    #[test]
    fn reconciler_does_not_select_a_camera_without_an_explicit_request() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let _a = app.world_mut().spawn(window_cam(true, "A")).id();
        let _b = app.world_mut().spawn(window_cam(true, "B")).id();

        app.update();

        assert!(
            active_set(&mut app).is_empty(),
            "a camera-less presentation must remain visibly camera-less"
        );
        assert_eq!(app.world().resource::<SceneViewport>().active_camera, None);
    }

    #[test]
    fn avatar_presence_does_not_select_a_camera() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalAvatar>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_systems(Update, reconcile_scene_viewport);

        app.world_mut()
            .spawn((window_cam(false, "Avatar"), LocalAvatar));
        app.update();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner,
            CameraSelectionOwner::None,
            "avatar projection is presence, not presentation policy"
        );
        assert_eq!(app.world().resource::<SceneViewport>().active_camera, None);
    }

    #[test]
    fn avatar_view_request_uses_newest_claim_during_deferred_demotion() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalAvatar>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_observer(on_request_local_avatar_view);

        let old = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalAvatar))
            .id();
        let new = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalAvatar))
            .id();

        // The old role removal is deferred by the component hook, but the
        // authoritative slot already names the newest claimant.
        assert_eq!(app.world().resource::<TheLocalAvatar>().0, Some(new));
        assert!(app.world().get::<SceneCamera>(old).is_some());
        app.world_mut().trigger(lunco_core::RequestLocalAvatarView);
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().requested,
            Some(RequestedCamera::Entity(new))
        );
    }
}
