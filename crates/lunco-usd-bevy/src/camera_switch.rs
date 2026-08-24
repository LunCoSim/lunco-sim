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
use big_space::prelude::{FloatingOrigin, Grid};
use lunco_core::{on_command, Command, LocalAvatar, SceneViewport, TheLocalAvatar};
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
    Fallback,
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

/// Engine-owned presentation camera used only when a loaded scene has no
/// authored window camera. It is an explicit camera policy, not an authored
/// scene fact: the status/menu can identify it and the camera is removed as
/// soon as an authored presentation camera becomes available.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationFallbackCamera;

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
    Fallback,
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

    pub fn fallback(target: Entity) -> Self {
        Self {
            target,
            source: CameraActivationSource::Fallback,
        }
    }
}

fn resolve_named_camera(
    want: &str,
    q_cams: &Query<(Entity, &Name), With<SceneCamera>>,
) -> Option<Entity> {
    q_cams.iter().find_map(|(entity, name)| {
        let value = name.as_str();
        (value == want || value.rsplit('/').next() == Some(want)).then_some(entity)
    })
}

fn record_camera_error(status: &mut CameraSelectionStatus, message: String) {
    status.last_error = Some(message);
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
        Some(target) => commands.trigger(ActivateCamera::director(target)),
        None => {
            let message = format!("director camera '{want}' is not present in the scene");
            record_camera_error(&mut status, message.clone());
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
        Some(target) => commands.trigger(ActivateCamera::user(target)),
        None => {
            let message = format!("operator camera '{want}' is not present in the scene");
            record_camera_error(&mut status, message.clone());
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
        record_camera_error(&mut status, message.clone());
        warn!("[camera] {message}");
        return;
    };
    commands.trigger(ActivateCamera::user(target));
}

/// Establish the authored avatar as the initial presentation view once it has
/// been projected. Scene teardown intentionally clears the previous camera
/// selection because its USD key belongs to the outgoing stage; the incoming
/// avatar is the authored replacement for that default view.
///
/// This is change-gated rather than a reconciler fallback. An explicit
/// director or operator request therefore remains authoritative, while a
/// scene reload cannot leave the main viewport with no camera merely because
/// the replacement avatar was spawned after teardown.
pub fn request_initial_avatar_view(
    q_avatar: Query<
        (),
        (
            With<LocalAvatar>,
            Or<(Added<LocalAvatar>, Added<SceneCamera>)>,
        ),
    >,
    local_avatar: Res<TheLocalAvatar>,
    selection: Res<ViewportCameraSelection>,
    mut commands: Commands,
) {
    if selection.owner() != CameraSelectionOwner::None {
        return;
    }
    if local_avatar
        .0
        .is_some_and(|avatar| q_avatar.contains(avatar))
    {
        commands.trigger(lunco_core::RequestLocalAvatarView);
    }
}

#[on_command(ResumeCameraDirector)]
pub fn on_resume_camera_director(
    _trigger: On<ResumeCameraDirector>,
    q_tracks: Query<(), With<crate::camera_track::CameraTrackPlan>>,
    mut selection: ResMut<ViewportCameraSelection>,
    mut status: ResMut<CameraSelectionStatus>,
) {
    selection.owner = CameraSelectionOwner::Director;
    selection.requested = None;
    selection.director_revision = selection.director_revision.wrapping_add(1);
    if q_tracks.is_empty() {
        let message = "the scene has no authored CameraTrack to resume".to_string();
        record_camera_error(&mut status, message.clone());
        warn!("[camera] {message}");
    } else {
        status.last_error = None;
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
    let cur = vp
        .active_camera
        .and_then(|a| cams.iter().position(|(e, _)| *e == a))
        .unwrap_or(0);
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
                CameraActivationSource::Fallback => CameraSelectionOwner::Fallback,
            };
            status.last_error = None;
            info!(
                "[camera] viewport → {target:?} (owner={:?})",
                selection.owner
            );
        }
        Ok((Some(_), _)) => {
            let message = format!("camera {target:?} is not a window camera");
            record_camera_error(&mut status, message.clone());
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
            record_camera_error(&mut status, message.clone());
            warn!("[camera] {message}");
        }
    }
}

/// The **single authority** over window-camera `is_active` + `viewport`.
///
/// Reads the [`SceneViewport`] (active-camera binding + visibility + rect) and
/// actuates it: exactly the bound camera is active (and only when visible); all
/// other window cameras are off. RTT (`Image`-target) cameras are ignored.
/// Also relocates the big_space [`FloatingOrigin`] onto the active camera when
/// it is grid-direct.
///
/// There is deliberately no implicit camera selection here. If the selection
/// is absent, stale, or still waiting for its authored camera to finish
/// projection, every window camera is inactive and the status view model says
/// why. The windowed host may publish an explicit fallback activation for a
/// camera-less scene; this reconciler only fulfils that request and never
/// chooses a different camera itself.
pub fn reconcile_scene_viewport(
    mut vp: ResMut<SceneViewport>,
    selection: Res<ViewportCameraSelection>,
    mut q_cams: Query<
        (
            Entity,
            &mut Camera,
            &RenderTarget,
            Option<&ChildOf>,
            Has<bevy::camera::Projection>,
            Option<&UsdPrimPath>,
        ),
        With<SceneCamera>,
    >,
    q_grids: Query<(), With<Grid>>,
    q_origins: Query<Entity, With<FloatingOrigin>>,
    mut commands: Commands,
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
            Option<&ChildOf>,
            Has<bevy::camera::Projection>,
            Option<&UsdPrimPath>,
        ),
        With<SceneCamera>,
    >,
                       e: Entity|
     -> bool {
        q.get(e)
            .is_ok_and(|(_, _, t, _, has_proj, _)| is_window_render_target(t) && has_proj)
    };

    // ── Resolve only the explicit request ───────────────────────────────
    let active = selection.requested.as_ref().and_then(|requested| {
        let entity = match requested {
            RequestedCamera::Entity(entity) => Some(*entity),
            RequestedCamera::Authored(wanted) => q_cams
                .iter()
                .find(|(_, _, _, _, _, path)| {
                    path.is_some_and(|path| {
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

    // Can the active camera host the FloatingOrigin? (grid-direct only)
    let grid_direct = active
        .and_then(|e| q_cams.get(e).ok())
        .and_then(|(_, _, _, parent, _, _)| parent)
        .map(|c| q_grids.contains(c.parent()))
        .unwrap_or(false);

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

    // ── FloatingOrigin follows the active camera (grid-direct only) ──────
    // Only mutate when it actually needs to move — re-inserting the marker
    // every frame churns big_space's recentring and jitters camera follow.
    if let (Some(active), true) = (active, grid_direct) {
        let active_has_origin = q_origins.contains(active);
        for prior in q_origins.iter() {
            if prior != active {
                commands.entity(prior).remove::<FloatingOrigin>();
            }
        }
        if !active_has_origin {
            commands.entity(active).try_insert(FloatingOrigin);
        }
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
    }
}

/// Scene teardown is the ownership boundary for camera selection. A stale
/// authored key must not select a camera from the next scene.
pub fn reset_camera_selection(
    mut selection: ResMut<ViewportCameraSelection>,
    mut viewport: ResMut<SceneViewport>,
    mut status: ResMut<CameraSelectionStatus>,
) {
    *selection = ViewportCameraSelection::default();
    viewport.active_camera = None;
    *status = CameraSelectionStatus::default();
}

/// Hand ownership back to the authored scene when its camera is projected
/// after the explicit presentation camera. This is the camera-selection
/// boundary, so fallback retirement cannot leave both cameras active or make a
/// later authored projection invisible behind stale fallback state.
pub fn retire_presentation_fallback(
    authored: Query<
        (Entity, Option<&Name>, Option<&RenderTarget>),
        (With<SceneCamera>, Without<PresentationFallbackCamera>),
    >,
    fallback: Query<Entity, With<PresentationFallbackCamera>>,
    selection: Res<ViewportCameraSelection>,
    mut viewport: ResMut<SceneViewport>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    if fallback.is_empty() {
        return;
    }

    let Some((target, _, _)) = authored
        .iter()
        .find(|(_, _, render_target)| render_target.is_none_or(is_window_render_target))
    else {
        return;
    };

    for entity in &fallback {
        commands.entity(entity).try_despawn();
    }
    if selection.owner() != CameraSelectionOwner::Fallback {
        return;
    }

    viewport.active_camera = None;
    *status = CameraSelectionStatus::default();
    commands.trigger(ActivateCamera::director(target));
}

/// Final guard for the window render target. Scene-camera reconciliation owns
/// the intended camera, but a stale render pipeline can outlive its
/// `SceneCamera` marker for one deferred-command boundary during scene reload.
/// Never allow that orphaned `Camera3d` to render alongside the selected view.
pub(crate) fn enforce_one_window_camera(
    vp: Res<SceneViewport>,
    mut cameras: Query<(
        Entity,
        &mut Camera,
        &RenderTarget,
        Has<SceneCamera>,
        Has<Camera3d>,
    )>,
) {
    let active = vp.active_camera;
    for (entity, mut camera, target, _scene_camera, has_pipeline) in &mut cameras {
        if !is_window_render_target(target) || !has_pipeline {
            continue;
        }
        // The active entity is filtered by the reconciler to a window camera
        // with a complete 3D pipeline. Every other window Camera3d is off,
        // including an orphan whose SceneCamera marker was just removed.
        camera.is_active = Some(entity) == active;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window_cam(is_active: bool, name: &str) -> impl Bundle {
        (
            SceneCamera::default(),
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
    fn authored_camera_retires_fallback_and_takes_selection_ownership() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_systems(Update, retire_presentation_fallback);
        let fallback = app
            .world_mut()
            .spawn((SceneCamera::default(), PresentationFallbackCamera))
            .id();
        let authored = app
            .world_mut()
            .spawn((SceneCamera::default(), Name::new("Authored")))
            .id();
        {
            let mut selection = app.world_mut().resource_mut::<ViewportCameraSelection>();
            selection.requested = Some(RequestedCamera::Entity(fallback));
            selection.owner = CameraSelectionOwner::Fallback;
        }

        app.update();

        assert!(app.world().get_entity(fallback).is_err());
        let selection = app.world().resource::<ViewportCameraSelection>();
        assert_eq!(selection.owner, CameraSelectionOwner::Director);
        assert_eq!(selection.requested, Some(RequestedCamera::Entity(authored)));
    }

    #[test]
    fn projected_local_avatar_reestablishes_initial_view_after_selection_reset() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalAvatar>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_observer(on_request_local_avatar_view)
            .add_systems(Update, request_initial_avatar_view);

        let avatar = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalAvatar))
            .id();

        app.update();

        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            None,
            "camera binding is reconciler-owned and waits for the render host"
        );
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner,
            CameraSelectionOwner::User
        );
        assert!(
            app.world()
                .resource::<ViewportCameraSelection>()
                .requested
                .as_ref()
                .is_some_and(|requested| matches!(
                    requested,
                    RequestedCamera::Entity(entity) if *entity == avatar
                )),
            "the incoming avatar must publish the existing selection intent"
        );
    }

    #[test]
    fn initial_avatar_view_does_not_override_explicit_director_selection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalAvatar>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_observer(on_request_local_avatar_view)
            .add_systems(Update, request_initial_avatar_view);

        let director = app
            .world_mut()
            .spawn((SceneCamera::default(), Name::new("Director")))
            .id();
        app.world_mut().trigger(ActivateCamera::director(director));

        let _ = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalAvatar))
            .id();

        app.update();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner,
            CameraSelectionOwner::Director
        );
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().requested,
            Some(RequestedCamera::Entity(director))
        );
    }

    #[test]
    fn initial_avatar_view_waits_for_a_camera_added_after_the_avatar() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalAvatar>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_observer(on_request_local_avatar_view)
            .add_systems(Update, request_initial_avatar_view);

        let avatar = app.world_mut().spawn(LocalAvatar).id();
        app.update();
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner,
            CameraSelectionOwner::None,
            "the intent must wait until the avatar has a usable scene camera"
        );

        app.world_mut()
            .entity_mut(avatar)
            .insert(SceneCamera::default());
        app.update();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner,
            CameraSelectionOwner::User
        );
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().requested,
            Some(RequestedCamera::Entity(avatar))
        );
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
