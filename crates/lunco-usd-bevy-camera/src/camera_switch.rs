//! Viewport active-camera switching + the viewport-camera **reconciler**.
//!
//! The scene has one main-window [`Viewport`](lunco_viewport_core::SceneViewport): it
//! owns *which* camera renders (its **active camera**), *whether* it renders
//! (visibility), and *what rect* it occupies — modelled on an Omniverse
//! Viewport. [`reconcile_scene_viewport`] is the **single authority** that
//! turns that into Bevy's per-camera `Camera::is_active` + `Camera::viewport`;
//! nothing else writes those for window cameras. Contributors only supply data:
//! - the switch here rebinds the viewport's active camera;
//! - the workbench sets visibility + rect from its layout perspective.
//!
//! This lives in `lunco-usd-bevy-camera` (without avatar runtime dependencies, present in every windowed
//! binary) so switching works in a static/headless world with no avatar and no
//! input. Camera selection is an intent-level operation: a [`SceneCamera`] can
//! be selected before a render host has attached its [`RenderTarget`] (as in a
//! headless run or during projection). The renderer later binds the selected
//! camera to its presentation surface. Ordinary RTT (`Image`-target) cameras
//! and the egui `Camera2d` are never selected; an offscreen recorder may keep a
//! non-writing authored source camera as the selected pose owner while its
//! image-target mirror renders the take.
//!
//! Switch surfaces, one mechanism — all funnel through [`ActivateCamera`] →
//! rebind [`SceneViewport::active_camera`](lunco_viewport_core::SceneViewport):
//! - [`SetActiveCamera`] — director command (API + rhai `set_camera("Name")`);
//! - [`SetUserCamera`] — explicit operator selection;
//! - [`ObserveAvatar`] / [`ResumeCameraDirector`] — explicit presentation-mode
//!   transitions;
//! - the `KeyC` hotkey ([`cycle_active_camera`]) when a host runs with input.

use bevy::camera::{Exposure, RenderTarget, Viewport, primitives::Aabb};
use bevy::prelude::*;
use big_space::prelude::{CellCoord, Grid};
use lunco_camera_core::{DEFAULT_PRESENTATION_HOOK, SCENE_AVATAR_HOOK};
use lunco_control_core::ControlBinding;
use lunco_core::{Command, on_command};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment, TheLocalEmbodiment};
use lunco_hooks::HookValue;
use lunco_render::SceneCamera;
use lunco_spatial::{OriginAnchor, WorldGrid};
use lunco_usd_bevy_stage::UsdStageAsset;
use lunco_viewport_core::SceneViewport;

use lunco_usd_bevy_scene::UsdPrimPath;

/// Telemetry event name for camera admission findings shown in Recent Events.
pub const CAMERA_CONTRACT_EVENT_NAME: &str = "camera-contract";

/// Stable camera selection across re-projection. ECS entities are disposable;
/// an authored camera is identified by the composed stage plus its USD path.
#[derive(Resource, Clone, PartialEq, Eq, Default)]
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
    stage: AssetId<UsdStageAsset>,
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
    /// The application presentation policy selected this camera.
    Policy,
    Generated,
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

/// Marks the transient avatar camera supplied to a viewed scene.
///
/// This is intentionally separate from [`UsdPrimPath`]: the camera is a
/// presentation projection of the mounted assembly, not authored USD content.
/// Parenting it below the scene root gives it the same Twin/scene lifetime as
/// the geometry it frames.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct StandalonePresentationCamera;

/// Marks the directional light generated with a standalone presentation camera.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct StandalonePresentationLight;

/// Opt-in and lifecycle state for generated standalone USD presentations.
///
/// The windowed host enables this resource. Headless hosts leave it disabled,
/// so they retain the authored camera contract and never create render-only
/// scene content. The generated entities are children of the active
/// [`lunco_usd_bevy_scene::UsdSceneRoot`] and are therefore reclaimed with that Twin scene;
/// [`reset_camera_selection`] also clears them at the explicit teardown
/// boundary before a replacement scene is admitted.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct StandalonePresentationState {
    /// Whether the host requests a transient avatar when the active scene
    /// has no authored local avatar.
    pub enabled: bool,
    /// Active scene root that owns the generated presentation, if any.
    pub root: Option<Entity>,
    /// Generated camera entity, including one queued for insertion.
    pub camera: Option<Entity>,
    /// Generated directional-light entity, including one queued for insertion.
    pub light: Option<Entity>,
    /// The owner is waiting for projection or for deferred entity insertion.
    pub pending: bool,
    /// A terminal presentation reason for the active root, if generation is
    /// rejected (for example, renderable bounds are non-finite).
    pub error: Option<String>,
    /// Hook-registry generation used for the last avatar-presence
    /// decision. A policy replacement reopens the decision without polling
    /// the hook on every frame.
    pub policy_generation: u64,
}

/// Tunable defaults for the generated standalone framing. These are a resource
/// rather than literals in the projection system so the presentation owner has
/// one explicit policy surface and tests can exercise it without a window.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct StandalonePresentationSettings {
    /// Multiplier applied to the distance needed to fit the bounding sphere.
    pub framing_margin: f32,
    /// Framing radius used only for a scene with no renderable geometry.
    pub empty_scene_framing_radius: f32,
    /// Camera position direction from the framed assembly center.
    pub camera_direction: Vec3,
    /// Light position direction from the framed assembly center.
    pub light_direction: Vec3,
    /// Illuminance of the generated presentation light.
    pub light_illuminance: f32,
    /// Fixed photographic exposure for the generated presentation camera.
    /// This is part of the standalone presentation rig and is independent of
    /// render-quality selection.
    pub camera_exposure_ev100: f32,
    /// Near clip plane in metres.
    pub near_clip: f32,
    /// Minimum far clip plane in metres.
    pub minimum_far_clip: f32,
}

impl Default for StandalonePresentationSettings {
    fn default() -> Self {
        Self {
            framing_margin: 1.35,
            empty_scene_framing_radius: 1.0,
            camera_direction: Vec3::new(1.0, 0.65, 1.0),
            light_direction: Vec3::new(-1.0, 1.5, 1.0),
            light_illuminance: 128_000.0,
            camera_exposure_ev100: 16.0,
            near_clip: 0.01,
            minimum_far_clip: 1_000.0,
        }
    }
}

impl StandalonePresentationSettings {
    fn validate(&self) -> Result<(), String> {
        if !self.empty_scene_framing_radius.is_finite() || self.empty_scene_framing_radius <= 0.0 {
            return Err(
                "standalone presentation empty_scene_framing_radius must be finite and positive"
                    .into(),
            );
        }
        if !(self.framing_margin.is_finite() && self.framing_margin >= 1.0) {
            return Err(
                "standalone presentation framing_margin must be finite and at least 1.0"
                    .to_string(),
            );
        }
        if !(self.camera_direction.is_finite()
            && self.camera_direction.length_squared().is_finite()
            && self.camera_direction.length_squared() > 0.0)
        {
            return Err(
                "standalone presentation camera_direction must be finite and non-zero".to_string(),
            );
        }
        if !(self.light_direction.is_finite()
            && self.light_direction.length_squared().is_finite()
            && self.light_direction.length_squared() > 0.0)
        {
            return Err(
                "standalone presentation light_direction must be finite and non-zero".to_string(),
            );
        }
        if !(self.light_illuminance.is_finite() && self.light_illuminance > 0.0) {
            return Err(
                "standalone presentation light_illuminance must be finite and positive".to_string(),
            );
        }
        if !self.camera_exposure_ev100.is_finite() {
            return Err("standalone presentation camera_exposure_ev100 must be finite".to_string());
        }
        if !(self.near_clip.is_finite()
            && self.near_clip > 0.0
            && self.minimum_far_clip.is_finite()
            && self.minimum_far_clip > self.near_clip)
        {
            return Err(
                "standalone presentation clip settings must be finite with 0 < near_clip < minimum_far_clip"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DefaultPresentationAction {
    None,
    Embodiment,
    FirstAuthored,
}

/// Consult the application presentation policy over facts already derived by
/// the USD camera projection. The return vocabulary is deliberately closed:
/// an absent hook, a fault, or any other value is an explicit presentation
/// error, never a request to use a guessed camera.
fn default_presentation_action(
    authored_camera_count: usize,
    camera_track_count: usize,
    local_avatar_camera_count: usize,
    runtime_context: lunco_core::RuntimeExecutionContext,
) -> Result<DefaultPresentationAction, String> {
    let context = HookValue::map([
        (
            "authored_camera_count",
            HookValue::Int(authored_camera_count as i64),
        ),
        (
            "camera_track_count",
            HookValue::Int(camera_track_count as i64),
        ),
        (
            "local_avatar_camera_count",
            HookValue::Int(local_avatar_camera_count as i64),
        ),
    ]);
    let result = lunco_hooks::invoke_with_context(
        DEFAULT_PRESENTATION_HOOK,
        &[context],
        runtime_context,
    )
    .ok_or_else(|| {
        format!(
            "camera default-presentation policy '{DEFAULT_PRESENTATION_HOOK}' is not registered"
        )
    })?;
    let value = result.map_err(|error| {
        format!("camera default-presentation policy '{DEFAULT_PRESENTATION_HOOK}' faulted: {error}")
    })?;
    match value.as_str() {
        Some("none") => Ok(DefaultPresentationAction::None),
        Some("avatar") => Ok(DefaultPresentationAction::Embodiment),
        Some("first") => Ok(DefaultPresentationAction::FirstAuthored),
        Some(other) => Err(format!(
            "camera default-presentation policy '{DEFAULT_PRESENTATION_HOOK}' returned unsupported action '{other}'"
        )),
        None => Err(format!(
            "camera default-presentation policy '{DEFAULT_PRESENTATION_HOOK}' must return 'none', 'avatar', or 'first'"
        )),
    }
}

/// Apply the policy's stable first-camera rule to active-root candidates.
/// Both window framing and the authored contract validator use this ordering.
fn first_authored_camera(candidates: impl IntoIterator<Item = (Entity, String)>) -> Option<Entity> {
    candidates
        .into_iter()
        .min_by(|(_, left_path), (_, right_path)| left_path.cmp(right_path))
        .map(|(entity, _)| entity)
}

fn presentation_runtime_context(time: &Time<Real>) -> lunco_core::RuntimeExecutionContext {
    lunco_core::RuntimeExecutionContext {
        route: Some(lunco_core::RuntimeRoute::application(
            lunco_core::RuntimeCycle::Presentation,
        )),
        phase: lunco_core::RuntimePhase::Preparation,
        clock: lunco_core::RuntimeClock::Presentation,
        time_seconds: Some(time.elapsed_secs_f64()),
        delta_seconds: Some(time.delta_secs_f64()),
        sequence: None,
        producer: None,
    }
}

/// One viewport contract state for authored and standalone camera presentation.
///
/// The render host opts into `required`. This state owns the verdict and
/// findings; `RuntimeDiagnostics` and Recent Events are projections of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraContractFinding {
    pub severity: lunco_core::DiagnosticSeverity,
    pub subject: String,
    pub message: String,
}

#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct CameraContractStatus {
    /// Whether the current host requires a presentable authored scene.
    pub required: bool,
    /// Whether the current authored contract has passed validation.
    pub ready: bool,
    /// Canonical findings for the current scene contract. RuntimeDiagnostics
    /// and Recent Events are projections of this same state.
    pub findings: Vec<CameraContractFinding>,
}

impl ViewportCameraSelection {
    /// Whether `entity` is the camera most recently selected by an explicit
    /// director or operator command. The authored path is the stable identity
    /// across asynchronous USD re-projection; the entity argument keeps the
    /// offscreen renderer on the same selected camera in the current world.
    pub fn matches_requested(&self, entity: Entity, path: Option<&UsdPrimPath>) -> bool {
        match (&self.requested, path) {
            (Some(RequestedCamera::Entity(requested)), _) => *requested == entity,
            (Some(RequestedCamera::Authored(requested)), Some(path)) => {
                requested.stage == path.stage_handle.id() && requested.path == path.path
            }
            _ => false,
        }
    }

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
    Policy,
    Generated,
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

    /// Select a camera from the application-level presentation policy.
    pub fn policy(target: Entity) -> Self {
        Self {
            target,
            source: CameraActivationSource::Policy,
        }
    }

    pub fn generated(target: Entity) -> Self {
        Self {
            target,
            source: CameraActivationSource::Generated,
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
    event_name: &'static str,
    commands: &mut Commands,
) {
    if update_camera_error_status(status, &message, commands) {
        lunco_core::trigger_runtime_error(commands, event_name, message.clone());
    }
}

fn record_camera_contract_finding(
    status: &mut CameraSelectionStatus,
    message: String,
    commands: &mut Commands,
) {
    if update_camera_error_status(status, &message, commands) {
        lunco_telemetry_core::emit_status_telemetry_event(
            commands,
            CAMERA_CONTRACT_EVENT_NAME,
            lunco_telemetry_core::Severity::Warning,
            message,
        );
    }
}

fn camera_contract_finding(
    severity: lunco_core::DiagnosticSeverity,
    subject: impl Into<String>,
    message: impl Into<String>,
) -> CameraContractFinding {
    CameraContractFinding {
        severity,
        subject: subject.into(),
        message: message.into(),
    }
}

/// Commit one canonical camera-contract verdict, then project it to the shared
/// diagnostics query and Recent Events feed. Severity and text have one owner.
fn publish_camera_contract_status(
    contract: &mut CameraContractStatus,
    diagnostics: &mut Option<ResMut<lunco_core::RuntimeDiagnostics>>,
    status: &mut CameraSelectionStatus,
    commands: &mut Commands,
    ready: bool,
    findings: Vec<CameraContractFinding>,
) {
    let changed = contract.ready != ready || contract.findings != findings;
    contract.ready = ready;
    contract.findings = findings;

    if let Some(diagnostics) = diagnostics.as_deref_mut() {
        let projected = contract
            .findings
            .iter()
            .map(|finding| lunco_core::RuntimeDiagnostic {
                code: CAMERA_CONTRACT_EVENT_NAME.to_string(),
                severity: finding.severity,
                producer: "usd-camera".to_string(),
                subject: finding.subject.clone(),
                message: finding.message.clone(),
            });
        diagnostics.replace_producer("usd-camera", projected);
    }

    if !changed {
        return;
    }
    if let Some(error) = contract
        .findings
        .iter()
        .find(|finding| finding.severity == lunco_core::DiagnosticSeverity::Error)
    {
        record_camera_contract_finding(status, error.message.clone(), commands);
    } else if let Some(warning) = contract.findings.first() {
        if status
            .last_error
            .as_deref()
            .is_some_and(|error| error.starts_with("[camera-contract]"))
        {
            clear_camera_error(status, commands);
        }
        warn!("[camera] {}", warning.message);
        lunco_telemetry_core::emit_status_telemetry_event(
            commands,
            CAMERA_CONTRACT_EVENT_NAME,
            lunco_telemetry_core::Severity::Warning,
            warning.message.clone(),
        );
    } else if status
        .last_error
        .as_deref()
        .is_some_and(|error| error.starts_with("[camera-contract]"))
    {
        clear_camera_error(status, commands);
    }
}

fn update_camera_error_status(
    status: &mut CameraSelectionStatus,
    message: &str,
    commands: &mut Commands,
) -> bool {
    if status.last_error.as_deref() == Some(message) {
        return false;
    }
    warn!("[camera] {message}");
    status.last_error = Some(message.to_owned());
    commands.trigger(CameraSelectionStatusChanged);
    true
}

fn clear_camera_error(status: &mut CameraSelectionStatus, commands: &mut Commands) {
    if status.last_error.take().is_some() {
        commands.trigger(CameraSelectionStatusChanged);
    }
}

fn is_window_render_target(target: &RenderTarget) -> bool {
    matches!(target, RenderTarget::Window(_))
}

fn is_offscreen_pose_owner(target: &RenderTarget, camera: &Camera) -> bool {
    matches!(
        (&camera.output_mode, target),
        (bevy::camera::CameraOutputMode::Skip, RenderTarget::Image(_))
    )
}

fn is_viewport_render_target(target: &RenderTarget, camera: &Camera) -> bool {
    is_window_render_target(target) || is_offscreen_pose_owner(target, camera)
}

fn is_selectable_camera_target(target: Option<&RenderTarget>, camera: Option<&Camera>) -> bool {
    target.is_none_or(|target| {
        is_window_render_target(target)
            || camera.is_some_and(|camera| is_offscreen_pose_owner(target, camera))
    })
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
            record_camera_error(&mut status, message, "camera-selection", &mut commands);
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
            record_camera_error(&mut status, message, "camera-selection", &mut commands);
        }
    }
}

#[on_command(ObserveAvatar)]
pub fn on_observe_avatar(_trigger: On<ObserveAvatar>, mut commands: Commands) {
    commands.trigger(lunco_camera_core::RequestLocalEmbodimentView);
}

/// Resolve the shared avatar-return intent. Embodiment mechanics and the UI use
/// this same path, so neither can clear the viewport and accidentally leave a
/// director camera selected behind the scenes.
pub fn on_request_local_avatar_view(
    _trigger: On<lunco_camera_core::RequestLocalEmbodimentView>,
    local_avatar: Res<TheLocalEmbodiment>,
    q_cameras: Query<(), With<SceneCamera>>,
    mut status: ResMut<CameraSelectionStatus>,
    mut commands: Commands,
) {
    let target = local_avatar.0.filter(|entity| q_cameras.contains(*entity));
    let Some(target) = target else {
        let message = "the scene has no local avatar camera to observe".to_string();
        record_camera_error(&mut status, message, "camera-selection", &mut commands);
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
        record_camera_error(&mut status, message, "camera-selection", &mut commands);
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
    q_cams: Query<
        (Option<&RenderTarget>, Option<&UsdPrimPath>, Option<&Camera>),
        With<SceneCamera>,
    >,
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
        // selected presentation surface exists.
        Ok((render_target, path, camera)) => {
            if is_selectable_camera_target(render_target, camera) {
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
                    CameraActivationSource::Policy => CameraSelectionOwner::Policy,
                    CameraActivationSource::Generated => CameraSelectionOwner::Generated,
                };
                clear_camera_error(&mut status, &mut commands);
                info!(
                    "[camera] viewport → {target:?} (owner={:?})",
                    selection.owner
                );
            } else {
                let message = format!("camera {target:?} is not a window camera");
                record_camera_error(&mut status, message, "camera-selection", &mut commands);
            }
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
            record_camera_error(&mut status, message, "camera-selection", &mut commands);
        }
    }
}

/// The **single authority** over presentation-camera binding and window-camera
/// `is_active` + `viewport`.
///
/// Reads the [`SceneViewport`] (active-camera binding + visibility + rect) and
/// actuates it: exactly the bound camera is active (and only when visible); all
/// other window cameras are off. An image-target camera participates in the
/// binding only when it is an explicit non-writing pose owner; ordinary RTT
/// cameras remain ignored. Also updates the persistent BigSpace
/// [`OriginAnchor`] to the active camera's `WorldGrid` cell using the
/// authoritative f64 hierarchy pose. The camera is a render consumer; it never
/// owns the origin marker.
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
            Option<&bevy::light::cluster::Clusters>,
            Option<&UsdPrimPath>,
            Has<lunco_render::CameraRetiring>,
            Has<SceneCamera>,
        ),
        With<Camera3d>,
    >,
) {
    // A camera is only ACTIVATABLE once its 3D pipeline (`Camera3d` → required
    // `Projection`) is bound by `lunco-render-bevy` and Bevy has computed a positive
    // physical target and a positive clustered-light grid. A `SceneCamera` arrives
    // asynchronously from USD projection; the render binder and `CameraUpdateSystems`
    // complete on later schedules. If the camera is activated before the target or
    // cluster grid exists, Bevy's GPU preparation attempts to create a zero-width
    // dummy texture. wgpu then keeps rejecting the invalid view, so the presentation
    // ladder eventually deactivates the cameras and leaves an otherwise ready scene
    // empty. Requiring all three readiness conditions keeps the sole active window
    // view valid from its first extracted frame.
    let activatable = |q: &Query<
        (
            Entity,
            &mut Camera,
            &RenderTarget,
            Has<bevy::camera::Projection>,
            Option<&bevy::light::cluster::Clusters>,
            Option<&UsdPrimPath>,
            Has<lunco_render::CameraRetiring>,
            Has<SceneCamera>,
        ),
        With<Camera3d>,
    >,
                       e: Entity|
     -> bool {
        q.get(e).is_ok_and(
            |(_, camera, t, has_proj, clusters, _, retiring, scene_camera)| {
                let is_pose_owner = is_viewport_render_target(t, camera);
                let has_render_clusters =
                    clusters.is_none_or(|clusters| clusters.dimensions != UVec3::ZERO);
                !retiring
                    && is_pose_owner
                    && has_proj
                    && scene_camera
                    && camera
                        .physical_viewport_size()
                        .is_some_and(|size| size.x > 0 && size.y > 0)
                    && (has_render_clusters || is_offscreen_pose_owner(t, camera))
            },
        )
    };

    // ── Resolve only the explicit request ───────────────────────────────
    let active = selection.requested.as_ref().and_then(|requested| {
        let entity = match requested {
            RequestedCamera::Entity(entity) => Some(*entity),
            RequestedCamera::Authored(wanted) => q_cams
                .iter()
                .find(|(_, _, _, _, _, path, retiring, scene_camera)| {
                    *scene_camera
                        && !*retiring
                        && path.is_some_and(|path| {
                            path.stage_handle.id() == wanted.stage && path.path == wanted.path
                        })
                })
                .map(|(entity, _, _, _, _, _, _, _)| entity),
        }?;
        activatable(&q_cams, entity).then_some(entity)
    });
    if vp.active_camera != active {
        vp.active_camera = active;
    }

    let visible = vp.visible;
    let rect = vp.rect;

    // ── Actuate: the ONE writer of window-camera is_active + viewport ────
    for (e, mut cam, target, _, _, _, _, _) in q_cams.iter_mut() {
        if !is_window_render_target(target) {
            continue; // RTT/offscreen cameras are self-managed
        }
        // `active` is gated by pipeline, target, and cluster readiness, so a
        // camera that is still binding or computing its first render frame is
        // kept off until the complete window presentation is valid.
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
    let mut world_grid_iter = q_world_grid.iter();
    let world_grid = world_grid_iter.next();
    let world_grid_count = usize::from(world_grid.is_some()) + world_grid_iter.count();
    if let Ok((mut origin_cell, mut origin_transform)) = q_origin.single_mut() {
        if let Some(world_grid) = world_grid.filter(|_| world_grid_count == 1) {
            if let Some(active) = vp.active_camera {
                if let Some((camera_position, _camera_rotation)) =
                    lunco_spatial::coords::pose_in_grid(
                        active, world_grid, &q_parents, &q_grids, &q_spatial,
                    )
                {
                    if let Ok(world_grid_component) = q_grids.get(world_grid) {
                        let (new_cell, new_translation) =
                            world_grid_component.translation_to_grid(camera_position);
                        origin_cell.set_if_neq(new_cell);
                        origin_transform.set_if_neq(Transform::from_translation(new_translation));
                    } else {
                        origin_cell.set_if_neq(CellCoord::default());
                        origin_transform.set_if_neq(Transform::IDENTITY);
                        origin_errors.push(
                            "[camera-origin] the active WorldGrid has no BigSpace Grid component"
                                .to_string(),
                        );
                    }
                } else {
                    origin_cell.set_if_neq(CellCoord::default());
                    origin_transform.set_if_neq(Transform::IDENTITY);
                    origin_errors.push(format!(
                        "[camera-origin] active camera {active:?} has no complete f64 pose in WorldGrid"
                    ));
                }
            } else {
                origin_cell.set_if_neq(CellCoord::default());
                origin_transform.set_if_neq(Transform::IDENTITY);
            }
        } else {
            origin_cell.set_if_neq(CellCoord::default());
            origin_transform.set_if_neq(Transform::IDENTITY);
        }
        if vp.active_camera.is_some() && world_grid_count != 1 {
            origin_errors.push(
                format!(
                    "[camera-origin] the persistent WorldGrid contract requires exactly one entity, found {}",
                    world_grid_count
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
    q_cams: Query<(Entity, &Name, &RenderTarget, Has<LocalEmbodiment>), With<SceneCamera>>,
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
                Changed<LocalEmbodiment>,
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

/// Scene identity survives camera reparenting into an orbital or physics Grid.
/// Spatial ancestry remains useful for unowned entities, but cannot determine
/// membership for cameras whose mode owns their spatial parent.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CameraSceneOwnership<'w, 's> {
    prims: Query<'w, 's, &'static UsdPrimPath>,
    provenance: Query<'w, 's, &'static lunco_core::Provenance>,
    identities: Query<'w, 's, &'static lunco_core::GlobalEntityId>,
}

impl CameraSceneOwnership<'_, '_> {
    fn belongs_to(&self, entity: Entity, root: Entity) -> Option<bool> {
        if let Ok(prim) = self.prims.get(entity) {
            return Some(self.prims.get(root).is_ok_and(|owner| {
                prim.stage_handle.id() == owner.stage_handle.id()
                    && (prim.path == owner.path
                        || prim
                            .path
                            .strip_prefix(&owner.path)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            }));
        }
        if let Ok(lunco_core::Provenance::Derived { parent, .. }) = self.provenance.get(entity) {
            return Some(
                self.identities
                    .get(root)
                    .is_ok_and(|id| id.get() == *parent),
            );
        }
        None
    }
}

/// Derived scene facts used to provision a transient avatar without writing USD.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct StandalonePresentationQueries<'w, 's> {
    ownership: CameraSceneOwnership<'w, 's>,
    scene_roots: Query<'w, 's, (), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    synced_roots: Query<'w, 's, (), With<lunco_usd_bevy_scene::UsdSceneProjected>>,
    child_of: Query<'w, 's, &'static ChildOf>,
    entities: Query<'w, 's, Entity>,
    pending: Query<
        'w,
        's,
        Entity,
        Or<(
            With<lunco_usd_bevy_scene::UsdSceneAwaitingStage>,
            With<lunco_usd_bevy_scene::UsdSceneGeometryPending>,
        )>,
    >,
    bounds: Query<'w, 's, (Entity, &'static Aabb, &'static GlobalTransform)>,
    tracks: Query<'w, 's, Entity, With<crate::camera_track::CameraTrack>>,
    authored_cameras: Query<'w, 's, Entity, (With<SceneCamera>, With<UsdPrimPath>)>,
    avatar_cameras: Query<
        'w,
        's,
        Entity,
        (
            With<SceneCamera>,
            With<LocalEmbodiment>,
            Without<StandalonePresentationCamera>,
        ),
    >,
    generated_cameras:
        Query<'w, 's, (Entity, &'static ChildOf), With<StandalonePresentationCamera>>,
    generated_lights: Query<'w, 's, (Entity, &'static ChildOf), With<StandalonePresentationLight>>,
    directional_lights: Query<
        'w,
        's,
        Option<&'static bevy::camera::visibility::RenderLayers>,
        With<DirectionalLight>,
    >,
    root_identities: Query<
        'w,
        's,
        &'static lunco_core::GlobalEntityId,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    root_transforms:
        Query<'w, 's, &'static GlobalTransform, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
}

/// Reconcile avatar availability only at scene/projection/policy boundaries.
/// Avatar movement does not invalidate this provisioning pass.
pub(crate) fn scene_avatar_inputs_changed(
    mount: Res<lunco_core::SceneMountState>,
    settings: Res<StandalonePresentationSettings>,
    presentation: Res<StandalonePresentationState>,
    added: Query<
        Entity,
        Or<(
            Added<SceneCamera>,
            Added<LocalEmbodiment>,
            Added<lunco_usd_bevy_scene::UsdSceneProjected>,
            Added<Aabb>,
            Added<lunco_core::GlobalEntityId>,
            Added<crate::camera_track::CameraTrack>,
        )>,
    >,
    mut removed_cameras: RemovedComponents<SceneCamera>,
    mut removed_avatars: RemovedComponents<LocalEmbodiment>,
    mut removed_geometry: RemovedComponents<lunco_usd_bevy_scene::UsdSceneGeometryPending>,
    mut removed_roots: RemovedComponents<lunco_usd_bevy_scene::UsdSceneRoot>,
    mut generation: Local<Option<u64>>,
) -> bool {
    let policy_changed =
        generation.replace(lunco_hooks::generation()) != Some(lunco_hooks::generation());
    let cameras = removed_cameras.read().next().is_some();
    let avatars = removed_avatars.read().next().is_some();
    let geometry = removed_geometry.read().next().is_some();
    let roots = removed_roots.read().next().is_some();
    policy_changed
        || mount.is_changed()
        || settings.is_changed()
        || presentation.is_changed()
        || presentation.pending
        || !added.is_empty()
        || cameras
        || avatars
        || geometry
        || roots
}

/// Apply the application avatar policy using the shared scene-owned camera rig.
/// An authored avatar remains its USD owner's responsibility. Transient rigs
/// survive director/operator camera switches and are reclaimed with the scene.
pub(crate) fn ensure_standalone_presentation(
    time: Res<Time<Real>>,
    mount: Res<lunco_core::SceneMountState>,
    settings: Res<StandalonePresentationSettings>,
    mut presentation: ResMut<StandalonePresentationState>,
    mut selection: ResMut<ViewportCameraSelection>,
    mut viewport: ResMut<SceneViewport>,
    queries: StandalonePresentationQueries,
    mut commands: Commands,
) {
    let active_root = mount.active_root();

    // Any generated presentation is owned by this host's active scene. Reclaim
    // stale or disabled instances by exact marker, never by a broad scene query.
    let generated_entities: Vec<Entity> = queries
        .generated_cameras
        .iter()
        .map(|(entity, _)| entity)
        .chain(queries.generated_lights.iter().map(|(entity, _)| entity))
        .collect();
    if !presentation.enabled || active_root.is_none() {
        despawn_generated_presentation(
            &generated_entities,
            &mut selection,
            &mut viewport,
            &mut commands,
        );
        if presentation.root.is_some()
            || presentation.camera.is_some()
            || presentation.light.is_some()
            || presentation.pending
            || presentation.error.is_some()
        {
            let enabled = presentation.enabled;
            presentation.set_if_neq(StandalonePresentationState {
                enabled,
                ..default()
            });
        }
        return;
    }
    let root = active_root.expect("active_root was checked above");

    if presentation.root != Some(root) {
        despawn_generated_presentation(
            &generated_entities,
            &mut selection,
            &mut viewport,
            &mut commands,
        );
        let enabled = presentation.enabled;
        presentation.set_if_neq(StandalonePresentationState {
            enabled,
            root: Some(root),
            pending: true,
            ..default()
        });
    }

    let authored_track = queries.tracks.iter().any(|entity| {
        entity_belongs_to_root(
            entity,
            root,
            &queries.scene_roots,
            &queries.child_of,
            &queries.entities,
        )
    });
    let authored_avatar = queries.avatar_cameras.iter().any(|entity| {
        queries
            .ownership
            .belongs_to(entity, root)
            .unwrap_or_else(|| {
                entity_belongs_to_root(
                    entity,
                    root,
                    &queries.scene_roots,
                    &queries.child_of,
                    &queries.entities,
                )
            })
    });
    if authored_avatar {
        despawn_generated_presentation(
            &generated_entities,
            &mut selection,
            &mut viewport,
            &mut commands,
        );
        if presentation.root.is_some()
            || presentation.camera.is_some()
            || presentation.light.is_some()
            || presentation.pending
            || presentation.error.is_some()
        {
            let enabled = presentation.enabled;
            presentation.set_if_neq(StandalonePresentationState {
                enabled,
                ..default()
            });
        }
        return;
    }

    if !queries.scene_roots.contains(root) || !queries.synced_roots.contains(root) {
        let enabled = presentation.enabled;
        let camera = presentation.camera;
        let light = presentation.light;
        presentation.set_if_neq(StandalonePresentationState {
            enabled,
            root: Some(root),
            camera,
            light,
            pending: true,
            ..default()
        });
        return;
    }

    // Waiting for the USD projection queue is not a missing-presentation error.
    // The next update will retry after the renderable descendants commit.
    if queries.pending.iter().any(|entity| {
        entity_belongs_to_root(
            entity,
            root,
            &queries.scene_roots,
            &queries.child_of,
            &queries.entities,
        )
    }) {
        let enabled = presentation.enabled;
        let camera = presentation.camera;
        let light = presentation.light;
        presentation.set_if_neq(StandalonePresentationState {
            enabled,
            root: Some(root),
            camera,
            light,
            pending: true,
            ..default()
        });
        return;
    }

    if let Err(message) = settings.validate() {
        if presentation.error.as_deref() != Some(message.as_str()) {
            let enabled = presentation.enabled;
            presentation.set_if_neq(StandalonePresentationState {
                enabled,
                root: Some(root),
                error: Some(message.clone()),
                ..default()
            });
            error!("[camera] {message}");
        }
        return;
    }

    let policy_generation = lunco_hooks::generation();
    if presentation.root == Some(root)
        && !presentation.pending
        && presentation.camera.is_none()
        && presentation.policy_generation == policy_generation
        && !settings.is_changed()
    {
        return;
    }
    let existing_camera = queries
        .generated_cameras
        .iter()
        .find_map(|(entity, _child)| {
            queries
                .ownership
                .belongs_to(entity, root)
                .unwrap_or_else(|| {
                    entity_belongs_to_root(
                        entity,
                        root,
                        &queries.scene_roots,
                        &queries.child_of,
                        &queries.entities,
                    )
                })
                .then_some(entity)
        });
    let existing_light = queries
        .generated_lights
        .iter()
        .find_map(|(entity, _child)| {
            queries
                .ownership
                .belongs_to(entity, root)
                .unwrap_or_else(|| {
                    entity_belongs_to_root(
                        entity,
                        root,
                        &queries.scene_roots,
                        &queries.child_of,
                        &queries.entities,
                    )
                })
                .then_some(entity)
        });
    let unscoped_directional_light = queries
        .directional_lights
        .iter()
        .any(|layers| layers.is_none());

    if let Some(camera) =
        existing_camera.filter(|_| presentation.policy_generation == policy_generation)
    {
        let should_activate = presentation.pending;
        if presentation.pending {
            presentation.pending = false;
        }
        if should_activate && selection.requested.is_none() && !authored_track {
            commands.trigger(ActivateCamera::generated(camera));
        }
        return;
    }
    let decision = lunco_hooks::invoke_with_context(
        SCENE_AVATAR_HOOK,
        &[HookValue::map([(
            "local_avatar_camera_count",
            HookValue::Int(0),
        )])],
        presentation_runtime_context(&time),
    )
    .ok_or_else(|| format!("avatar policy '{SCENE_AVATAR_HOOK}' is not registered"))
    .and_then(|result| result.map_err(|error| error.to_string()))
    .and_then(parse_scene_avatar_decision);
    let binding = match decision {
        Ok(Some(binding)) => binding,
        result => {
            despawn_generated_presentation(
                &generated_entities,
                &mut selection,
                &mut viewport,
                &mut commands,
            );
            let error = result.err().map(|error| format!("[camera-policy] {error}"));
            if let Some(message) = &error {
                error!("[camera] {message}");
            }
            let enabled = presentation.enabled;
            presentation.set_if_neq(StandalonePresentationState {
                enabled,
                root: Some(root),
                policy_generation,
                error,
                ..default()
            });
            return;
        }
    };
    let inputs = lunco_port_core::InputPorts::new(&binding.ports().collect::<Vec<_>>());
    if let Some(camera) = existing_camera {
        commands.entity(camera).insert((binding, inputs));
        let was_known = presentation.camera == Some(camera);
        let should_activate = presentation.pending || !was_known;
        let enabled = presentation.enabled;
        presentation.set_if_neq(StandalonePresentationState {
            enabled,
            root: Some(root),
            camera: Some(camera),
            light: existing_light,
            policy_generation,
            ..default()
        });
        if should_activate && selection.requested.is_none() && !authored_track {
            commands.trigger(ActivateCamera::generated(camera));
        }
        return;
    }

    let framing = standalone_presentation_bounds(
        root,
        &queries.scene_roots,
        &queries.child_of,
        &queries.entities,
        &queries.root_transforms,
        &queries.bounds,
    );
    let (center, radius) = match framing {
        Ok(Some((min, max))) => ((min + max) * 0.5, ((max - min) * 0.5).length()),
        // An empty scene has no geometry to frame. This is its explicit host
        // placement contract, never a substitute for invalid bounds.
        Ok(None) => (Vec3::ZERO, settings.empty_scene_framing_radius),
        Err(message) => {
            let enabled = presentation.enabled;
            presentation.set_if_neq(StandalonePresentationState {
                enabled,
                root: Some(root),
                policy_generation,
                error: Some(message.clone()),
                ..default()
            });
            error!("[camera] {message}");
            return;
        }
    };
    let (camera_transform, light_transform, projection) =
        standalone_presentation_pose(center, radius, &settings);
    if !camera_transform.translation.is_finite()
        || !light_transform.translation.is_finite()
        || !camera_transform.rotation.is_finite()
        || !light_transform.rotation.is_finite()
    {
        let message = "scene avatar framing exceeds the finite camera range".to_string();
        let enabled = presentation.enabled;
        presentation.set_if_neq(StandalonePresentationState {
            enabled,
            root: Some(root),
            policy_generation,
            error: Some(message.clone()),
            ..default()
        });
        error!("[camera] {message}");
        return;
    }
    let Ok(root_identity) = queries.root_identities.get(root) else {
        presentation.pending = true;
        return;
    };
    let camera = commands
        .spawn((
            StandalonePresentationCamera,
            (
                Embodiment,
                LocalEmbodiment,
                binding,
                inputs,
                lunco_core::Provenance::Derived {
                    parent: root_identity.get(),
                    role: "scene-avatar".into(),
                },
            ),
            SceneCamera::agx(),
            Exposure {
                ev100: settings.camera_exposure_ev100,
            },
            Camera {
                is_active: false,
                ..default()
            },
            projection,
            RenderTarget::Window(bevy::window::WindowRef::Primary),
            camera_transform,
            GlobalTransform::default(),
            Visibility::Visible,
            InheritedVisibility::default(),
            ViewVisibility::default(),
            CellCoord::default(),
            ChildOf(root),
            Name::new("Avatar"),
        ))
        .id();
    let authored_camera = queries.authored_cameras.iter().any(|entity| {
        queries
            .ownership
            .belongs_to(entity, root)
            .unwrap_or_else(|| {
                entity_belongs_to_root(
                    entity,
                    root,
                    &queries.scene_roots,
                    &queries.child_of,
                    &queries.entities,
                )
            })
    });
    let light = if unscoped_directional_light || authored_camera || authored_track {
        None
    } else {
        Some(
            commands
                .spawn((
                    StandalonePresentationLight,
                    DirectionalLight {
                        illuminance: settings.light_illuminance,
                        shadow_maps_enabled: true,
                        ..default()
                    },
                    bevy::light::CascadeShadowConfig::default(),
                    light_transform,
                    GlobalTransform::default(),
                    Visibility::Visible,
                    InheritedVisibility::default(),
                    ViewVisibility::default(),
                    CellCoord::default(),
                    ChildOf(root),
                    Name::new("LunCo standalone presentation light"),
                ))
                .id(),
        )
    };

    let enabled = presentation.enabled;
    presentation.set_if_neq(StandalonePresentationState {
        enabled,
        root: Some(root),
        camera: Some(camera),
        light,
        pending: true,
        policy_generation,
        ..default()
    });
}

fn parse_scene_avatar_decision(value: HookValue) -> Result<Option<ControlBinding>, String> {
    let Some(HookValue::Bool(create)) = value.get("create") else {
        return Err("avatar policy must return a boolean create field".into());
    };
    if !*create {
        return Ok(None);
    }
    let Some(HookValue::Array(rows)) = value.get("bindings") else {
        return Err("avatar policy must supply control bindings".into());
    };
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let HookValue::Array(fields) = row else {
            return Err("avatar control binding must be [intent, port, factor]".into());
        };
        if fields.len() != 3 {
            return Err("avatar control binding must be [intent, port, factor]".into());
        }
        let intent = fields[0]
            .as_str()
            .ok_or_else(|| "avatar control intent must be a string".to_string())?;
        let port = fields[1]
            .as_str()
            .ok_or_else(|| "avatar control port must be a string".to_string())?;
        let HookValue::Float(factor) = fields[2] else {
            return Err("avatar control factor must be an f64".into());
        };
        if !factor.is_finite() {
            return Err("avatar control factor must be finite".into());
        }
        if lunco_control_core::parse_user_intent(intent).is_none() || port.trim().is_empty() {
            return Err("avatar policy supplied an invalid control binding".into());
        }
        entries.push((intent.to_string(), port.to_string(), factor));
    }
    ControlBinding::from_intent_entries(&entries)
        .map(Some)
        .ok_or_else(|| "avatar policy supplied no control bindings".into())
}

fn entity_belongs_to_root(
    entity: Entity,
    root: Entity,
    q_scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_child_of: &Query<&ChildOf>,
    q_entities: &Query<Entity>,
) -> bool {
    lunco_usd_bevy_scene::scene_root_ancestor(entity, q_scene_roots, q_child_of, q_entities)
        .ok()
        .flatten()
        == Some(root)
}

fn despawn_generated_presentation(
    entities: &[Entity],
    selection: &mut ViewportCameraSelection,
    viewport: &mut SceneViewport,
    commands: &mut Commands,
) {
    let selected_generated = selection.owner == CameraSelectionOwner::Generated
        || entities.iter().any(|entity| {
            matches!(
                selection.requested,
                Some(RequestedCamera::Entity(requested)) if requested == *entity
            )
        });
    for entity in entities {
        commands.entity(*entity).try_despawn();
    }
    if selected_generated {
        *selection = ViewportCameraSelection::default();
        viewport.active_camera = None;
    }
}

fn standalone_presentation_bounds(
    root: Entity,
    q_scene_roots: &Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_child_of: &Query<&ChildOf>,
    q_entities: &Query<Entity>,
    q_root_transforms: &Query<&GlobalTransform, With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_bounds: &Query<(Entity, &Aabb, &GlobalTransform)>,
) -> Result<Option<(Vec3, Vec3)>, String> {
    let root_transform = q_root_transforms
        .get(root)
        .map_err(|_| "scene root has no transform for avatar placement".to_string())?;
    let root_from_world = root_transform.affine().inverse();
    if !root_from_world.is_finite() {
        return Err("scene root has an invalid transform for avatar placement".into());
    }
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;

    for (entity, aabb, transform) in q_bounds.iter() {
        if !entity_belongs_to_root(entity, root, q_scene_roots, q_child_of, q_entities) {
            continue;
        }
        if !aabb.center.is_finite()
            || !aabb.half_extents.is_finite()
            || !transform.affine().is_finite()
        {
            return Err("scene has non-finite renderable bounds for avatar placement".into());
        }
        let half = aabb.half_extents.abs();
        let center = aabb.center;
        for x in [-1.0_f32, 1.0] {
            for y in [-1.0_f32, 1.0] {
                for z in [-1.0_f32, 1.0] {
                    let corner = center + half * Vec3A::new(x, y, z);
                    let world = transform.affine().transform_point3a(corner);
                    let local: Vec3 = root_from_world.transform_point3a(world).into();
                    if !local.is_finite() {
                        return Err(
                            "scene has non-finite renderable bounds for avatar placement".into(),
                        );
                    }
                    min = min.min(local);
                    max = max.max(local);
                    found = true;
                }
            }
        }
    }
    if found && (!(max - min).length_squared().is_finite() || (max - min).length_squared() <= 0.0) {
        return Err("scene has degenerate renderable bounds for avatar placement".into());
    }
    Ok(found.then_some((min, max)))
}

fn standalone_presentation_pose(
    center: Vec3,
    radius: f32,
    settings: &StandalonePresentationSettings,
) -> (Transform, Transform, Projection) {
    let camera_direction = settings.camera_direction.normalize();
    let fov = std::f32::consts::FRAC_PI_4;
    let tan_half_fov = (fov * 0.5).tan();
    let margin = settings.framing_margin;
    let distance = (radius / tan_half_fov + radius) * margin;
    let camera_position = center + camera_direction * distance;
    let camera_up = if camera_direction.dot(Vec3::Y).abs() > 0.95 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let camera_transform =
        Transform::from_translation(camera_position).looking_at(center, camera_up);

    let light_direction = settings.light_direction.normalize();
    let light_position = center + light_direction * (radius * 4.0).max(4.0);
    let light_up = if light_direction.dot(Vec3::Y).abs() > 0.95 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let light_transform = Transform::from_translation(light_position).looking_at(center, light_up);

    let mut projection = lunco_render::usd_default_perspective_projection();
    if let Projection::Perspective(perspective) = &mut projection {
        perspective.near = settings.near_clip.max(0.001).min(distance * 0.25);
        perspective.far = settings
            .minimum_far_clip
            .max(distance + radius * margin * 2.0)
            .max(perspective.near + 1.0);
    }
    (camera_transform, light_transform, projection)
}

/// Run the authored camera-contract admission check only when one of its
/// authoritative inputs changed. The validator owns a structural scan, so an
/// unconditional `Update` registration would make a settled scene pay for
/// every camera, track, root, and ancestry lookup on every render frame.
///
/// `CameraContractStatus` contains both the host's `required` input and the
/// validator's verdict. The local cursor watches only `required`, preventing a
/// verdict write from feeding the validator back into a steady-state loop.
/// Camera-track plans are mutable because their sampler owns a runtime cut
/// cursor; only plan insertion/removal is structural input to this validator.
/// The hook-registry generation is also an input: replacing a Rhai presentation
/// policy must reopen the decision without scanning the scene every frame.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct CameraContractInputQueries<'w, 's> {
    scene_roots: Query<
        'w,
        's,
        (),
        (
            With<lunco_usd_bevy_scene::UsdSceneRoot>,
            Or<(
                Added<lunco_usd_bevy_scene::UsdSceneRoot>,
                Changed<lunco_usd_bevy_scene::UsdSceneProjected>,
            )>,
        ),
    >,
    pending_added: Query<'w, 's, (), Added<lunco_usd_bevy_scene::UsdSceneAwaitingStage>>,
    cameras: Query<
        'w,
        's,
        (),
        (
            With<SceneCamera>,
            Or<(
                Added<SceneCamera>,
                Changed<Name>,
                Changed<UsdPrimPath>,
                Changed<LocalEmbodiment>,
            )>,
        ),
    >,
    tracks: Query<
        'w,
        's,
        (),
        (
            With<crate::camera_track::CameraTrack>,
            Or<(
                Added<crate::camera_track::CameraTrack>,
                Changed<UsdPrimPath>,
                Added<crate::camera_track::CameraTrackPlan>,
            )>,
        ),
    >,
    removed_pending: RemovedComponents<'w, 's, lunco_usd_bevy_scene::UsdSceneAwaitingStage>,
    removed_cameras: RemovedComponents<'w, 's, SceneCamera>,
    removed_tracks: RemovedComponents<'w, 's, crate::camera_track::CameraTrack>,
    removed_plans: RemovedComponents<'w, 's, crate::camera_track::CameraTrackPlan>,
    removed_roots: RemovedComponents<'w, 's, lunco_usd_bevy_scene::UsdSceneRoot>,
}

pub(crate) fn camera_contract_inputs_changed(
    mount: Res<lunco_core::SceneMountState>,
    revision: Res<lunco_usd_bevy_scene::UsdStageRevision>,
    contract: Res<CameraContractStatus>,
    presentation: Res<StandalonePresentationState>,
    selection: Res<ViewportCameraSelection>,
    mut queries: CameraContractInputQueries,
    mut required: Local<Option<bool>>,
    mut last_revision: Local<Option<u64>>,
    mut last_presentation: Local<Option<StandalonePresentationState>>,
    mut last_active_root: Local<Option<Entity>>,
    mut last_selection: Local<Option<ViewportCameraSelection>>,
    mut last_policy_generation: Local<Option<u64>>,
) -> bool {
    let first_validation = required.is_none();
    let required_changed = required
        .replace(contract.required)
        .is_some_and(|previous| previous != contract.required);
    // `bump_usd_stage_revision` owns a `ResMut` because it may publish a new
    // revision. Bevy consequently marks that resource access as changed even
    // when the monotonic value stays the same. Compare the value itself so a
    // harmless mutable borrow cannot reopen this structural scan every frame.
    let revision_changed = last_revision
        .replace(revision.0)
        .is_some_and(|previous| previous != revision.0);
    let presentation_changed = last_presentation
        .replace((*presentation).clone())
        .is_some_and(|previous| previous != *presentation);
    // Resource change ticks are not semantic change signals here: the
    // standalone presentation owner borrows SceneMountState and camera
    // selection mutably while reconciling generated presentation state. Track
    // the values that affect this validator so those harmless borrows cannot
    // reopen the structural scan every frame.
    let active_root = mount.active_root();
    let mount_changed = *last_active_root != active_root;
    *last_active_root = active_root;
    let selection_changed = last_selection
        .replace((*selection).clone())
        .is_some_and(|previous| previous != *selection);
    let policy_generation = lunco_hooks::generation();
    let policy_changed = last_policy_generation
        .replace(policy_generation)
        .is_some_and(|previous| previous != policy_generation);

    // Drain every removal reader before evaluating the result. A short-circuit
    // here would leave an event unread and re-open the structural pass later.
    let removed_pending = queries.removed_pending.read().next().is_some();
    let removed_cameras = queries.removed_cameras.read().next().is_some();
    let removed_tracks = queries.removed_tracks.read().next().is_some();
    let removed_plans = queries.removed_plans.read().next().is_some();
    let removed_roots = queries.removed_roots.read().next().is_some();

    first_validation
        || required_changed
        || mount_changed
        || revision_changed
        || presentation_changed
        || selection_changed
        || policy_changed
        || !queries.scene_roots.is_empty()
        || !queries.pending_added.is_empty()
        || !queries.cameras.is_empty()
        || !queries.tracks.is_empty()
        || removed_pending
        || removed_cameras
        || removed_tracks
        || removed_plans
        || removed_roots
}

/// Validate the window presentation contract after USD camera-track plans are
/// derived. Transient avatar cameras participate without a USD identity.
/// Duplicate tracks, absent cameras, unresolved
/// names, and duplicate active-root tracks are errors owned by the camera
/// domain. Additive roots do not participate in the single viewport contract.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuthoredAvatarSelection<'w, 's> {
    ownership: CameraSceneOwnership<'w, 's>,
    local_avatar: Res<'w, TheLocalEmbodiment>,
    retiring: Query<'w, 's, (), With<lunco_render::CameraRetiring>>,
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct AuthoredPresentationQueries<'w> {
    time: Res<'w, Time<Real>>,
}

pub(crate) fn validate_authored_camera_contract(
    mount: Res<lunco_core::SceneMountState>,
    presentation: Res<StandalonePresentationState>,
    scene_roots: Query<
        Has<lunco_usd_bevy_scene::UsdSceneProjected>,
        With<lunco_usd_bevy_scene::UsdSceneRoot>,
    >,
    pending_projection: Query<Entity, With<lunco_usd_bevy_scene::UsdSceneAwaitingStage>>,
    q_scene_root: Query<(), With<lunco_usd_bevy_scene::UsdSceneRoot>>,
    q_child_of: Query<&ChildOf>,
    q_entities: Query<Entity>,
    tracks: Query<
        (
            Entity,
            &UsdPrimPath,
            Option<&crate::camera_track::CameraTrackPlan>,
        ),
        With<crate::camera_track::CameraTrack>,
    >,
    cameras: Query<(Entity, &Name, Option<&UsdPrimPath>, Has<LocalEmbodiment>), With<SceneCamera>>,
    presentation_queries: AuthoredPresentationQueries,
    selection: Res<ViewportCameraSelection>,
    avatar_selection: AuthoredAvatarSelection,
    mut commands: Commands,
    mut contract: ResMut<CameraContractStatus>,
    mut status: ResMut<CameraSelectionStatus>,
    mut diagnostics: Option<ResMut<lunco_core::RuntimeDiagnostics>>,
) {
    if !contract.required {
        publish_camera_contract_status(
            &mut contract,
            &mut diagnostics,
            &mut status,
            &mut commands,
            true,
            Vec::new(),
        );
        return;
    }
    if mount.active_root().is_none() {
        publish_camera_contract_status(
            &mut contract,
            &mut diagnostics,
            &mut status,
            &mut commands,
            false,
            Vec::new(),
        );
        return;
    }

    let active_root = mount.active_root();
    let projection_pending = active_root.is_some_and(|root| {
        !scene_roots.get(root).is_ok_and(|synced| synced)
            || pending_projection.iter().any(|entity| {
                lunco_usd_bevy_scene::scene_root_ancestor(
                    entity,
                    &q_scene_root,
                    &q_child_of,
                    &q_entities,
                )
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
        publish_camera_contract_status(
            &mut contract,
            &mut diagnostics,
            &mut status,
            &mut commands,
            false,
            Vec::new(),
        );
        return;
    }

    if presentation.root == active_root {
        if presentation.pending {
            publish_camera_contract_status(
                &mut contract,
                &mut diagnostics,
                &mut status,
                &mut commands,
                false,
                Vec::new(),
            );
            return;
        }
        if let Some(error) = presentation.error.as_deref() {
            let findings = vec![camera_contract_finding(
                lunco_core::DiagnosticSeverity::Error,
                "standalone-presentation",
                format!("[camera-contract] standalone presentation: {error}"),
            )];
            publish_camera_contract_status(
                &mut contract,
                &mut diagnostics,
                &mut status,
                &mut commands,
                false,
                findings,
            );
            return;
        }
    }

    let active_root = active_root.expect("active root was checked above");
    let mut camera_candidates: Vec<(Entity, String, Option<String>, bool)> = Vec::new();
    let mut user_camera_selected = false;
    for (entity, name, prim, local_avatar) in &cameras {
        if avatar_selection
            .ownership
            .belongs_to(entity, active_root)
            .unwrap_or_else(|| {
                entity_belongs_to_root(entity, active_root, &q_scene_root, &q_child_of, &q_entities)
            })
        {
            user_camera_selected |= selection.owner() == CameraSelectionOwner::User
                && selection.matches_requested(entity, prim);
            camera_candidates.push((
                entity,
                name.as_str().to_string(),
                prim.map(|prim| prim.path.clone()),
                local_avatar,
            ));
        }
    }
    camera_candidates.sort_by(|left, right| left.2.cmp(&right.2));
    let camera_names: Vec<(Entity, String)> = camera_candidates
        .iter()
        .map(|(entity, name, _, _)| (*entity, name.clone()))
        .collect();
    let local_avatar_names: Vec<String> = camera_candidates
        .iter()
        .filter(|(_, _, _, local_avatar)| *local_avatar)
        .map(|(_, name, _, _)| name.clone())
        .collect();
    let active_tracks: Vec<_> = tracks
        .iter()
        .filter(|(entity, _, _)| {
            entity_belongs_to_root(
                *entity,
                active_root,
                &q_scene_root,
                &q_child_of,
                &q_entities,
            )
        })
        .collect();

    let track_severity = if user_camera_selected {
        lunco_core::DiagnosticSeverity::Warning
    } else {
        lunco_core::DiagnosticSeverity::Error
    };
    let mut findings = Vec::new();
    if camera_candidates.is_empty() {
        findings.push(camera_contract_finding(
            lunco_core::DiagnosticSeverity::Error,
            "window-presentation",
            "[camera-contract] scene has no authored SceneCamera for the window presentation"
                .to_string(),
        ));
    }

    if active_tracks.is_empty() && selection.requested.is_none() {
        if local_avatar_names.len() > 1 {
            findings.push(camera_contract_finding(
                lunco_core::DiagnosticSeverity::Error,
                "window-presentation",
                format!(
                "[camera-contract] scene has multiple LocalEmbodiment initial presentations: {}",
                local_avatar_names.join(", ")
                ),
            ));
        } else {
            match default_presentation_action(
                camera_candidates
                    .iter()
                    .filter(|(_, _, path, _)| path.is_some())
                    .count(),
                0,
                local_avatar_names.len(),
                presentation_runtime_context(&presentation_queries.time),
            ) {
                Ok(DefaultPresentationAction::Embodiment) => {
                    let candidate = camera_candidates
                        .iter()
                        .find(|(_, _, _, local_avatar)| *local_avatar)
                        .map(|(entity, _, _, _)| *entity);
                    match candidate {
                        Some(candidate) => {
                            if let Err(error) = request_authored_local_avatar_view(
                                candidate,
                                &avatar_selection.local_avatar,
                                &avatar_selection.retiring,
                                &mut commands,
                            ) {
                                findings.push(camera_contract_finding(
                                    lunco_core::DiagnosticSeverity::Error,
                                    "window-presentation",
                                    format!("[camera-policy] {error}"),
                                ));
                            }
                        }
                        None => findings.push(camera_contract_finding(
                            lunco_core::DiagnosticSeverity::Error,
                            "window-presentation",
                            "[camera-policy] the default presentation policy selected avatar without an active LocalEmbodiment camera",
                        )),
                    }
                }
                Ok(DefaultPresentationAction::FirstAuthored) => {
                    let authored_cameras =
                        camera_candidates.iter().filter_map(|(entity, _, path, _)| {
                            path.clone().map(|path| (*entity, path))
                        });
                    if let Some(camera) = first_authored_camera(authored_cameras) {
                        commands.trigger(ActivateCamera::policy(camera));
                    } else {
                        findings.push(camera_contract_finding(
                            lunco_core::DiagnosticSeverity::Error,
                            "window-presentation",
                            "[camera-policy] the default presentation policy selected the first camera, but the active scene has none",
                        ));
                    }
                }
                Ok(DefaultPresentationAction::None) => findings.push(camera_contract_finding(
                    lunco_core::DiagnosticSeverity::Error,
                    "window-presentation",
                    "[camera-contract] the default presentation policy selected no initial camera",
                )),
                Err(error) => findings.push(camera_contract_finding(
                    lunco_core::DiagnosticSeverity::Error,
                    "window-presentation",
                    format!("[camera-policy] {error}"),
                )),
            }
        }
    } else if active_tracks.len() > 1 {
        findings.push(camera_contract_finding(
            track_severity,
            "window-presentation",
            "[camera-contract] scene has multiple CameraTrack providers without an explicit viewport scope",
        ));
    }

    for (_, prim, plan) in &active_tracks {
        let Some(plan) = plan else {
            findings.push(camera_contract_finding(
                track_severity,
                prim.path.clone(),
                format!(
                    "[camera-contract] CameraTrack '{}' has not finished projection",
                    prim.path
                ),
            ));
            continue;
        };
        if plan.keys.is_empty() {
            findings.push(camera_contract_finding(
                track_severity,
                prim.path.clone(),
                format!(
                    "[camera-contract] CameraTrack '{}' has no activeCamera keys",
                    prim.path
                ),
            ));
        }
        for (_, want) in &plan.keys {
            if let Err(reason) = resolve_camera_names(want, &camera_names) {
                findings.push(camera_contract_finding(
                    track_severity,
                    prim.path.clone(),
                    format!(
                        "[camera-contract] CameraTrack '{}' cannot resolve '{want}': {reason}",
                        prim.path
                    ),
                ));
            }
        }
    }

    let ready = findings
        .iter()
        .all(|finding| finding.severity != lunco_core::DiagnosticSeverity::Error);
    publish_camera_contract_status(
        &mut contract,
        &mut diagnostics,
        &mut status,
        &mut commands,
        ready,
        findings,
    );
}

/// Resolve the policy-selected active-root LocalEmbodiment camera through its
/// authoritative role slot before activation.
fn request_authored_local_avatar_view(
    candidate: Entity,
    local_avatar: &TheLocalEmbodiment,
    retiring: &Query<(), With<lunco_render::CameraRetiring>>,
    commands: &mut Commands,
) -> Result<(), String> {
    let target = local_avatar.0.ok_or_else(|| {
        "the LocalEmbodiment camera is present but the authoritative role slot is empty".to_string()
    })?;
    if target != candidate {
        return Err(
            "the LocalEmbodiment role slot does not identify the active scene camera candidate"
                .to_string(),
        );
    }
    if retiring.get(target).is_ok() {
        return Err("the LocalEmbodiment camera is retiring".to_string());
    }
    commands.trigger(ActivateCamera::policy(target));
    Ok(())
}

/// Scene teardown is the ownership boundary for camera selection. A stale
/// authored key must not select a camera from the next scene.
pub fn reset_camera_selection(
    mut selection: ResMut<ViewportCameraSelection>,
    mut viewport: ResMut<SceneViewport>,
    mut status: ResMut<CameraSelectionStatus>,
    mut presentation: ResMut<StandalonePresentationState>,
    q_generated_cameras: Query<Entity, With<StandalonePresentationCamera>>,
    q_generated_lights: Query<Entity, With<StandalonePresentationLight>>,
    mut commands: Commands,
) {
    let generated_entities: Vec<Entity> = q_generated_cameras
        .iter()
        .chain(q_generated_lights.iter())
        .collect();
    despawn_generated_presentation(
        &generated_entities,
        &mut selection,
        &mut viewport,
        &mut commands,
    );
    *selection = ViewportCameraSelection::default();
    viewport.active_camera = None;
    let enabled = presentation.enabled;
    *presentation = StandalonePresentationState {
        enabled,
        ..default()
    };
    if *status != CameraSelectionStatus::default() {
        *status = CameraSelectionStatus::default();
        commands.trigger(CameraSelectionStatusChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    #[derive(Resource, Default)]
    struct CameraContractGateRuns(u32);

    fn count_camera_contract_gate_runs(mut runs: ResMut<CameraContractGateRuns>) {
        runs.0 += 1;
    }

    fn touch_camera_track_plans(mut plans: Query<&mut crate::camera_track::CameraTrackPlan>) {
        // The production sampler owns a mutable runtime cursor on this plan.
        // Merely fetching it mutably must not make the authored contract dirty.
        for _ in &mut plans {}
    }

    fn touch_stage_revision(revision: ResMut<lunco_usd_bevy_scene::UsdStageRevision>) {
        // A producer may borrow the revision mutably while checking its
        // structural inputs without actually advancing the value.
        let _ = revision.0;
    }

    fn touch_scene_mount(mut mount: ResMut<lunco_core::SceneMountState>) {
        // A producer may borrow mount state mutably while inspecting or
        // reconciling scene ownership without changing the active root.
        let _ = &mut *mount;
    }

    fn touch_camera_selection(mut selection: ResMut<ViewportCameraSelection>) {
        // A presentation owner may need mutable access to clear a generated
        // camera, but an unchanged selection is not a contract input change.
        let _ = &mut *selection;
    }

    struct TestPresentationPolicy;

    impl lunco_hooks::ScriptHook for TestPresentationPolicy {
        fn invoke(&self, invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            let context = invocation.context;
            if context.route
                != Some(lunco_core::RuntimeRoute::application(
                    lunco_core::RuntimeCycle::Presentation,
                ))
                || context.phase != lunco_core::RuntimePhase::Preparation
                || context.clock != lunco_core::RuntimeClock::Presentation
                || context.time_seconds.is_none()
                || context.delta_seconds.is_none()
                || context.sequence.is_some()
                || context.producer.is_some()
                || context.validate().is_err()
            {
                return Err(lunco_hooks::HookError(
                    "camera policy requires Application/Presentation/Preparation context with its presentation clock"
                        .into(),
                ));
            }
            let context = invocation
                .args
                .first()
                .ok_or_else(|| lunco_hooks::HookError("missing presentation facts".into()))?;
            let avatar_count = context
                .get("local_avatar_camera_count")
                .and_then(lunco_hooks::HookValue::as_i64)
                .ok_or_else(|| lunco_hooks::HookError("missing avatar count".into()))?;
            Ok(HookValue::str(if avatar_count == 1 {
                "avatar"
            } else {
                "first"
            }))
        }
    }

    #[derive(Resource, Default)]
    struct AvatarProvisionGateRuns(u32);

    fn count_avatar_provision_gate(mut runs: ResMut<AvatarProvisionGateRuns>) {
        runs.0 += 1;
    }

    #[test]
    fn scene_avatar_gate_ignores_settled_camera_movement() {
        let mut app = standalone_test_app();
        standalone_root_with_bounds(&mut app);
        app.init_resource::<AvatarProvisionGateRuns>().add_systems(
            Update,
            count_avatar_provision_gate.run_if(scene_avatar_inputs_changed),
        );
        for _ in 0..4 {
            app.update();
        }
        let before = app.world().resource::<AvatarProvisionGateRuns>().0;
        let avatar = app
            .world()
            .resource::<StandalonePresentationState>()
            .camera
            .unwrap();
        app.world_mut()
            .get_mut::<Transform>(avatar)
            .unwrap()
            .translation
            .x += 1.0;
        app.update();
        assert_eq!(app.world().resource::<AvatarProvisionGateRuns>().0, before);
        app.world_mut()
            .resource_mut::<StandalonePresentationState>()
            .enabled = false;
        app.update();
        assert!(app.world().resource::<AvatarProvisionGateRuns>().0 > before);
    }

    #[test]
    fn scene_avatar_decision_validates_typed_controls_without_coercion() {
        let result = |create, rows| {
            parse_scene_avatar_decision(HookValue::map([
                ("create", create),
                ("bindings", HookValue::Array(rows)),
            ]))
        };
        let binding = |intent, factor| {
            HookValue::Array(vec![
                HookValue::str(intent),
                HookValue::str("forward"),
                factor,
            ])
        };
        assert!(result(HookValue::Bool(false), vec![]).unwrap().is_none());
        assert!(
            result(
                HookValue::Bool(true),
                vec![binding("forward", HookValue::Float(1.0))]
            )
            .unwrap()
            .is_some()
        );
        assert!(result(HookValue::Int(1), vec![]).is_err());
        assert!(
            result(
                HookValue::Bool(true),
                vec![binding("forward", HookValue::Int(1))]
            )
            .is_err()
        );
        assert!(result(HookValue::Bool(true), vec![]).is_err());
        assert!(
            result(
                HookValue::Bool(true),
                vec![binding("invalid", HookValue::Float(1.0))]
            )
            .is_err()
        );
        assert!(
            result(
                HookValue::Bool(true),
                vec![binding("forward", HookValue::Float(f64::NAN))]
            )
            .is_err()
        );
    }

    struct TestAvatarPresencePolicy;
    impl lunco_hooks::ScriptHook for TestAvatarPresencePolicy {
        fn invoke(&self, invocation: &lunco_hooks::HookInvocation<'_>) -> lunco_hooks::HookResult {
            lunco_hooks::ScriptHook::invoke(&TestPresentationPolicy, invocation)?;
            Ok(HookValue::map([
                ("create", HookValue::Bool(true)),
                (
                    "bindings",
                    HookValue::Array(vec![HookValue::Array(vec![
                        HookValue::str("forward"),
                        HookValue::str("forward"),
                        HookValue::Float(1.0),
                    ])]),
                ),
            ]))
        }
    }

    fn install_test_presentation_policy() {
        static INSTALLED: Once = Once::new();
        INSTALLED.call_once(|| {
            lunco_hooks::register(lunco_hooks::RegisteredHook {
                id: DEFAULT_PRESENTATION_HOOK.into(),
                backend: "test".into(),
                deterministic: true,
                hook: std::sync::Arc::new(TestPresentationPolicy),
            });
            lunco_hooks::register(lunco_hooks::RegisteredHook {
                id: SCENE_AVATAR_HOOK.into(),
                backend: "test".into(),
                deterministic: false,
                hook: std::sync::Arc::new(TestAvatarPresencePolicy),
            });
        });
    }

    fn standalone_test_app() -> App {
        install_test_presentation_policy();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<SceneViewport>()
            .init_resource::<TheLocalEmbodiment>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .init_resource::<StandalonePresentationState>()
            .init_resource::<StandalonePresentationSettings>()
            .add_observer(on_activate_camera)
            .add_systems(
                Update,
                ensure_standalone_presentation.in_set(lunco_core::RuntimeCycleSet::Presentation),
            )
            .add_systems(lunco_core::SceneTeardown, reset_camera_selection);
        app.world_mut()
            .resource_mut::<StandalonePresentationState>()
            .enabled = true;
        app
    }

    fn standalone_root_with_bounds(app: &mut App) -> (Entity, Entity) {
        let root = app
            .world_mut()
            .spawn((
                lunco_usd_bevy_scene::UsdSceneRoot,
                lunco_core::GlobalEntityId::from_raw(42),
                lunco_usd_bevy_scene::UsdSceneProjected,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        let visual = app
            .world_mut()
            .spawn((
                Aabb::from_min_max(Vec3::new(-2.0, -1.0, -3.0), Vec3::new(2.0, 1.0, 3.0)),
                Transform::from_translation(Vec3::new(0.0, 0.0, -2.0)),
                GlobalTransform::from(Transform::from_translation(Vec3::new(0.0, 0.0, -2.0))),
                ChildOf(root),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);
        (root, visual)
    }

    #[test]
    fn standalone_presentation_frames_bounds_and_becomes_generated_owner() {
        let mut app = standalone_test_app();
        let (root, _visual) = standalone_root_with_bounds(&mut app);

        app.update();
        let presentation = app.world().resource::<StandalonePresentationState>();
        let camera = presentation.camera.expect("camera is queued");
        let light = presentation.light.expect("light is queued");
        assert_eq!(presentation.root, Some(root));
        assert!(presentation.pending);

        app.update();

        let presentation = app.world().resource::<StandalonePresentationState>();
        assert_eq!(presentation.camera, Some(camera));
        assert_eq!(presentation.light, Some(light));
        assert!(!presentation.pending);
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner(),
            CameraSelectionOwner::Generated
        );
        let transform = app.world().get::<Transform>(camera).unwrap();
        assert!(transform.translation.is_finite());
        assert!(transform.translation.length() > 1.0);
        assert!(app.world().get::<DirectionalLight>(light).is_some());
    }

    #[test]
    fn authored_avatar_survives_reparenting_outside_scene_hierarchy() {
        let mut app = standalone_test_app();
        let (root, _) = standalone_root_with_bounds(&mut app);
        app.world_mut().entity_mut(root).insert(UsdPrimPath {
            path: "/Scene".into(),
            ..default()
        });
        let world_grid = app.world_mut().spawn_empty().id();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                LocalEmbodiment,
                Name::new("Operator"),
                UsdPrimPath {
                    path: "/Scene/Operator".into(),
                    ..default()
                },
                ChildOf(world_grid),
            ))
            .id();

        app.update();
        app.update();

        assert_eq!(app.world().resource::<TheLocalEmbodiment>().0, Some(camera));
        assert!(
            app.world()
                .resource::<StandalonePresentationState>()
                .camera
                .is_none()
        );
        assert!(app.world().get::<LocalEmbodiment>(camera).is_some());
    }

    #[test]
    fn generated_avatar_survives_reparenting_outside_scene_hierarchy() {
        let mut app = standalone_test_app();
        standalone_root_with_bounds(&mut app);
        app.update();
        app.update();
        let camera = app
            .world()
            .resource::<StandalonePresentationState>()
            .camera
            .unwrap();
        let world_grid = app.world_mut().spawn_empty().id();
        app.world_mut()
            .entity_mut(camera)
            .insert(ChildOf(world_grid));

        app.update();
        app.update();

        assert_eq!(
            app.world().resource::<StandalonePresentationState>().camera,
            Some(camera)
        );
        assert_eq!(app.world().resource::<TheLocalEmbodiment>().0, Some(camera));
    }

    #[test]
    fn empty_scene_uses_explicit_avatar_framing() {
        let mut app = standalone_test_app();
        let root = app
            .world_mut()
            .spawn((
                lunco_usd_bevy_scene::UsdSceneRoot,
                lunco_core::GlobalEntityId::from_raw(42),
                lunco_usd_bevy_scene::UsdSceneProjected,
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);

        app.update();

        let state = app.world().resource::<StandalonePresentationState>();
        let avatar = state.camera.expect("empty scene has an avatar");
        assert!(app.world().get::<LocalEmbodiment>(avatar).is_some());
        assert!(state.error.is_none());
    }

    #[test]
    fn standalone_presentation_reports_invalid_settings_without_faking_a_camera() {
        let mut app = standalone_test_app();
        standalone_root_with_bounds(&mut app);
        app.world_mut()
            .resource_mut::<StandalonePresentationSettings>()
            .camera_direction = Vec3::ZERO;

        app.update();

        let state = app.world().resource::<StandalonePresentationState>();
        assert!(state.camera.is_none());
        assert!(!state.pending);
        assert_eq!(
            state.error.as_deref(),
            Some("standalone presentation camera_direction must be finite and non-zero")
        );
        assert!(
            app.world_mut()
                .query_filtered::<Entity, With<StandalonePresentationCamera>>()
                .iter(app.world())
                .next()
                .is_none()
        );
    }

    #[test]
    fn authored_director_keeps_a_transient_avatar_available() {
        let mut app = standalone_test_app();
        let (root, _visual) = standalone_root_with_bounds(&mut app);
        app.world_mut()
            .spawn((crate::camera_track::CameraTrack, ChildOf(root)));

        app.update();

        app.update();
        let state = app.world().resource::<StandalonePresentationState>();
        let avatar = state.camera.expect("director scene has a transient avatar");
        assert!(app.world().get::<LocalEmbodiment>(avatar).is_some());
        assert!(app.world().get::<UsdPrimPath>(avatar).is_none());
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner(),
            CameraSelectionOwner::None
        );
    }

    #[test]
    fn explicit_camera_selection_cannot_be_reclaimed_by_generated_policy() {
        let mut app = standalone_test_app();
        let (_root, _visual) = standalone_root_with_bounds(&mut app);
        app.update();
        app.update();
        let generated = app
            .world()
            .resource::<StandalonePresentationState>()
            .camera
            .expect("generated camera is ready");
        let explicit = app
            .world_mut()
            .spawn((SceneCamera::default(), Name::new("Operator camera")))
            .id();

        app.world_mut().trigger(ActivateCamera::user(explicit));
        app.update();
        app.update();

        assert!(app.world().get::<LocalEmbodiment>(generated).is_some());
        let selection = app.world().resource::<ViewportCameraSelection>();
        assert_eq!(selection.owner(), CameraSelectionOwner::User);
        assert_eq!(selection.requested, Some(RequestedCamera::Entity(explicit)));
    }

    #[test]
    fn scene_teardown_reclaims_generated_presentation_and_selection() {
        let mut app = standalone_test_app();
        standalone_root_with_bounds(&mut app);
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner(),
            CameraSelectionOwner::Generated
        );

        lunco_core::run_scene_teardown(app.world_mut());

        assert!(
            app.world_mut()
                .query_filtered::<Entity, With<StandalonePresentationCamera>>()
                .iter(app.world())
                .next()
                .is_none()
        );
        assert_eq!(
            app.world().resource::<StandalonePresentationState>().camera,
            None
        );
        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().owner(),
            CameraSelectionOwner::None
        );
    }

    fn window_cam(is_active: bool, name: &str) -> impl Bundle + use<> {
        window_cam_with_target(is_active, name, true)
    }

    fn window_cam_with_target(
        is_active: bool,
        name: &str,
        target_ready: bool,
    ) -> impl Bundle + use<> {
        let mut camera = Camera {
            is_active,
            ..default()
        };
        if target_ready {
            camera.computed.target_info = Some(bevy::camera::RenderTargetInfo {
                physical_size: UVec2::new(1280, 720),
                scale_factor: 1.0,
            });
        }
        (
            SceneCamera::default(),
            Camera3d::default(),
            camera,
            // A `Projection` stands in for the bound 3D pipeline: the reconciler
            // only activates cameras whose pipeline is present (guards the shadow
            // cascade-unwrap panic), so a test camera must carry one to be eligible.
            bevy::camera::Projection::default(),
            RenderTarget::Window(bevy::window::WindowRef::Primary),
            Name::new(name.to_string()),
        )
    }

    #[test]
    fn camera_contract_gate_is_quiet_until_an_input_changes() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<lunco_usd_bevy_scene::UsdStageRevision>()
            .init_resource::<CameraContractStatus>()
            .init_resource::<StandalonePresentationState>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraContractGateRuns>()
            .add_systems(
                Update,
                (
                    touch_stage_revision,
                    touch_scene_mount,
                    touch_camera_selection,
                    count_camera_contract_gate_runs.run_if(camera_contract_inputs_changed),
                )
                    .chain(),
            );

        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<CameraContractGateRuns>().0,
            1,
            "the initial admission is followed by a quiet settled frame"
        );

        let camera = app
            .world_mut()
            .spawn((SceneCamera::default(), Name::new("Wide")))
            .id();
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 2);
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 2);

        app.world_mut().despawn(camera);
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 3);

        app.world_mut()
            .resource_mut::<CameraContractStatus>()
            .required = true;
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 4);
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 4);
    }

    #[test]
    fn pending_projection_does_not_disable_required_camera_admission() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<CameraContractStatus>()
            .init_resource::<StandalonePresentationState>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .init_resource::<TheLocalEmbodiment>()
            .add_systems(Update, validate_authored_camera_contract);

        let root = app
            .world_mut()
            .spawn(lunco_usd_bevy_scene::UsdSceneRoot)
            .id();
        let _ = app
            .world_mut()
            .spawn((lunco_usd_bevy_scene::UsdSceneAwaitingStage, ChildOf(root)))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);
        app.world_mut()
            .resource_mut::<CameraContractStatus>()
            .required = true;

        app.update();

        let contract = app.world().resource::<CameraContractStatus>();
        assert!(contract.required);
        assert!(!contract.ready);
        assert!(contract.findings.is_empty());
    }

    #[test]
    fn mutable_camera_track_cursor_does_not_reopen_contract_scan() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<lunco_usd_bevy_scene::UsdStageRevision>()
            .init_resource::<CameraContractStatus>()
            .init_resource::<StandalonePresentationState>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraContractGateRuns>()
            .add_systems(
                Update,
                (
                    touch_camera_track_plans,
                    count_camera_contract_gate_runs.run_if(camera_contract_inputs_changed),
                )
                    .chain(),
            );

        app.update();
        assert_eq!(
            app.world().resource::<CameraContractGateRuns>().0,
            1,
            "the initial admission runs once"
        );

        app.world_mut().spawn((
            crate::camera_track::CameraTrack,
            crate::camera_track::CameraTrackPlan::default(),
            lunco_usd_bevy_scene::UsdPrimPath::default(),
        ));
        app.update();
        assert_eq!(app.world().resource::<CameraContractGateRuns>().0, 2);

        app.update();
        assert_eq!(
            app.world().resource::<CameraContractGateRuns>().0,
            2,
            "sampler-owned cursor writes are not structural camera changes"
        );
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
    fn reconciler_waits_for_a_positive_physical_viewport() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<ViewportCameraSelection>()
            .add_systems(Update, reconcile_scene_viewport);
        let camera = app
            .world_mut()
            .spawn(window_cam_with_target(false, "A", false))
            .id();
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(camera);
        app.world_mut()
            .resource_mut::<ViewportCameraSelection>()
            .requested = Some(RequestedCamera::Entity(camera));

        app.update();

        assert!(!app.world().get::<Camera>(camera).unwrap().is_active);
        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            None,
            "the requested camera stays pending until Bevy computes its target"
        );

        app.world_mut()
            .get_mut::<Camera>(camera)
            .unwrap()
            .computed
            .target_info = Some(bevy::camera::RenderTargetInfo {
            physical_size: UVec2::new(1280, 720),
            scale_factor: 1.0,
        });
        app.update();

        assert!(app.world().get::<Camera>(camera).unwrap().is_active);
        assert_eq!(
            app.world().resource::<SceneViewport>().active_camera,
            Some(camera)
        );
    }

    #[test]
    fn selected_camera_projects_origin_anchor_without_claiming_floating_origin() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<SceneViewport>()
            .init_resource::<lunco_core::RuntimeDiagnostics>();
        let world_grid = lunco_spatial::ensure_world_root(app.world_mut());
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
        assert!(
            app.world()
                .get::<big_space::prelude::FloatingOrigin>(camera)
                .is_none()
        );

        assert!(
            app.world()
                .resource::<lunco_core::RuntimeDiagnostics>()
                .findings
                .is_empty()
        );
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
        let stage = Handle::<UsdStageAsset>::default();
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
        let stage = Handle::<UsdStageAsset>::default();
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
            .init_resource::<TheLocalEmbodiment>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_systems(Update, reconcile_scene_viewport);

        app.world_mut()
            .spawn((window_cam(false, "Embodiment"), LocalEmbodiment));
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
            .init_resource::<TheLocalEmbodiment>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .add_observer(on_activate_camera)
            .add_observer(on_request_local_avatar_view);

        let old = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalEmbodiment))
            .id();
        let new = app
            .world_mut()
            .spawn((SceneCamera::default(), LocalEmbodiment))
            .id();

        // The old role removal is deferred by the component hook, but the
        // authoritative slot already names the newest claimant.
        assert_eq!(app.world().resource::<TheLocalEmbodiment>().0, Some(new));
        assert!(app.world().get::<SceneCamera>(old).is_some());
        app.world_mut()
            .trigger(lunco_camera_core::RequestLocalEmbodimentView);
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().requested,
            Some(RequestedCamera::Entity(new))
        );
    }

    #[test]
    fn authored_initial_request_uses_the_role_slot_not_camera_query_order() {
        install_test_presentation_policy();
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<lunco_core::SceneMountState>()
            .init_resource::<CameraContractStatus>()
            .init_resource::<StandalonePresentationState>()
            .init_resource::<ViewportCameraSelection>()
            .init_resource::<CameraSelectionStatus>()
            .init_resource::<TheLocalEmbodiment>()
            .add_observer(on_activate_camera)
            .add_systems(Update, validate_authored_camera_contract);
        app.world_mut()
            .resource_mut::<CameraContractStatus>()
            .required = true;

        let root = app
            .world_mut()
            .spawn((
                lunco_usd_bevy_scene::UsdSceneRoot,
                UsdPrimPath {
                    path: "/Scene".into(),
                    ..default()
                },
                lunco_core::GlobalEntityId::from_raw(42),
                lunco_usd_bevy_scene::UsdSceneProjected,
            ))
            .id();
        app.world_mut()
            .resource_mut::<lunco_core::SceneMountState>()
            .register_root(root, true);

        let _old = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                ChildOf(root),
                Name::new("Old"),
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/Scene/Old".into(),
                },
                LocalEmbodiment,
            ))
            .id();
        let new = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                ChildOf(root),
                Name::new("New"),
                UsdPrimPath {
                    stage_handle: Handle::default(),
                    path: "/Scene/New".into(),
                },
                LocalEmbodiment,
            ))
            .id();

        assert_eq!(
            app.world().resource::<TheLocalEmbodiment>().0,
            Some(new),
            "the role hook has already published the newest claimant"
        );

        app.update();

        assert_eq!(
            app.world().resource::<ViewportCameraSelection>().requested,
            Some(RequestedCamera::Authored(UsdCameraKey {
                stage: Handle::<UsdStageAsset>::default().id(),
                path: "/Scene/New".into(),
            })),
            "initial presentation follows the authoritative role slot"
        );
    }
}
