//! Independent camera windows over the capture owner's live render targets.
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiTextureHandle, egui};
use lunco_capture::camera_feed::{
    CameraFeeds, CreateCameraFeed, MAX_CAMERA_FEEDS, RemoveCameraFeed, SetCameraFeed,
    SetCameraFeedResolution, SetCameraImageEffects,
};
use lunco_command_contracts::{Ack, OpId, Reject};
use lunco_core::{Command, MarkClientLocalExt, on_command, register_commands};
use lunco_core::{
    GlobalEntityId, SceneResource, entity_display_name,
    markers::{Callsign, CatalogEntryId},
};
use lunco_render::camera_image::{CameraImageEffects, CameraImageMode};
use lunco_workbench_core::scene::CurrentScenePath;
use lunco_workbench_state::{
    WorkspaceCameraViewState, WorkspaceGraphSignalState, WorkspaceGraphViewState,
    WorkspaceRuntimeViewSnapshot, WorkspaceStateViewProvider,
};
use std::collections::BTreeMap;

/// Open live image controls. Omitted source follows the selected main camera.
#[Command(default)]
pub struct ShowCameraImageSettings {
    pub source: Option<GlobalEntityId>,
}
pub(super) fn install(app: &mut App) {
    register_all_commands(app);
    app.mark_client_local::<ShowCameraImageSettings>();
    app.init_resource::<PendingRuntimeViewRestore>()
        .init_resource::<SignalDescriptorRestoreCursor>()
        .add_systems(Update, restore_pending_runtime_views)
        .add_observer(request_camera_view_restore_on_identity)
        .add_observer(request_camera_view_restore_on_projection)
        .add_observer(reset_runtime_view_restore_after_transition)
        .add_observer(clear_runtime_view_restore_on_twin_closed);
    lunco_workbench_state::install_workspace_state_view_provider(app, RuntimeViewStateProvider);
}

fn clear_runtime_view_restore_on_twin_closed(
    trigger: On<lunco_workspace::TwinClosed>,
    mut pending: ResMut<PendingRuntimeViewRestore>,
) {
    if trigger.event().was_active {
        pending.0 = None;
    }
}

fn reset_runtime_view_restore_after_transition(
    _: On<lunco_core::SceneTransitionCompleted>,
    mut pending: ResMut<PendingRuntimeViewRestore>,
) {
    if let Some(pending) = pending.0.as_mut() {
        pending.progress_scene_path = None;
        pending.cameras_restored = false;
        pending.camera_retry_requested = true;
        pending.graphs_restored.clear();
        pending.graph_retry_requested = true;
    }
}

fn request_camera_view_restore(pending: &mut PendingRuntimeViews) {
    if !pending.cameras_restored {
        pending.camera_retry_requested = true;
    }
}

fn request_camera_view_restore_on_identity(
    _: On<Insert<GlobalEntityId>>,
    mut pending: ResMut<PendingRuntimeViewRestore>,
) {
    if let Some(pending) = pending.0.as_mut() {
        request_camera_view_restore(pending);
    }
}

fn request_camera_view_restore_on_projection(
    _: On<Insert<Projection>>,
    mut pending: ResMut<PendingRuntimeViewRestore>,
) {
    if let Some(pending) = pending.0.as_mut() {
        request_camera_view_restore(pending);
    }
}

#[derive(Resource, Default)]
struct SignalDescriptorRestoreCursor(MessageCursor<lunco_viz::SignalDescriptorsChanged>);

fn signal_descriptors_changed(world: &mut World) -> bool {
    world.resource_scope(|world, mut cursor: Mut<SignalDescriptorRestoreCursor>| {
        world
            .get_resource::<Messages<lunco_viz::SignalDescriptorsChanged>>()
            .is_some_and(|messages| cursor.0.read(messages).next().is_some())
    })
}

fn restore_pending_runtime_views(world: &mut World) {
    let signal_descriptors_changed = signal_descriptors_changed(world);
    let Some(mut pending) = world.resource_mut::<PendingRuntimeViewRestore>().0.take() else {
        return;
    };
    let mut retry_cameras = pending.take_camera_retry();
    let mut retry_graphs = pending.take_graph_retry(signal_descriptors_changed);
    if world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .is_some_and(|scene| scene.active().is_some() || scene.has_admitted())
    {
        world.resource_mut::<PendingRuntimeViewRestore>().0 = Some(pending);
        return;
    }
    let current_path = world
        .get_resource::<CurrentScenePath>()
        .map(|path| path.0.as_str())
        .unwrap_or_default();
    let Some(snapshot) = pending
        .snapshots
        .iter()
        .find(|snapshot| snapshot.scene_path == current_path)
    else {
        pending.progress_scene_path = None;
        pending.cameras_restored = false;
        pending.camera_retry_requested = false;
        pending.graphs_restored.clear();
        pending.graph_retry_requested = false;
        world.resource_mut::<PendingRuntimeViewRestore>().0 = Some(pending);
        return;
    };
    if !world
        .get_resource::<lunco_core::SceneMountState>()
        .is_some_and(|mount| mount.active_root().is_some())
    {
        pending.camera_retry_requested = true;
        pending.graph_retry_requested = true;
        world.resource_mut::<PendingRuntimeViewRestore>().0 = Some(pending);
        return;
    }

    if pending.progress_scene_path.as_deref() != Some(current_path) {
        pending.progress_scene_path = Some(current_path.to_owned());
        pending.cameras_restored = false;
        retry_cameras = true;
        pending.graphs_restored.clear();
        retry_graphs = true;
    }

    if retry_cameras && !pending.cameras_restored {
        match camera_view_sources_readiness(world, &snapshot.camera_views) {
            CameraViewSourcesReadiness::Ready => {
                restore_camera_views(world, &snapshot.camera_views);
                pending.cameras_restored = true;
            }
            CameraViewSourcesReadiness::Waiting => {}
            CameraViewSourcesReadiness::Invalid(reason) => {
                warn!(
                    "[workspace-view-state] camera views cannot be restored: {reason}; leaving those views closed"
                );
                pending.cameras_restored = true;
            }
        }
    }

    if retry_graphs {
        for view in &snapshot.graph_views {
            if pending.graphs_restored.contains(&view.id) {
                continue;
            }
            match graph_view_sources_readiness(world, view) {
                GraphViewSourcesReadiness::Ready => {
                    restore_graph_view(world, view);
                    pending.graphs_restored.insert(view.id);
                }
                GraphViewSourcesReadiness::Waiting => {}
                GraphViewSourcesReadiness::Invalid(reason) => {
                    warn!(
                        "[workspace-view-state] graph view {} cannot be restored: {}; leaving that view closed",
                        view.id, reason
                    );
                    pending.graphs_restored.insert(view.id);
                }
            }
        }
    }

    world.resource_mut::<PendingRuntimeViewRestore>().0 = Some(pending);
}

fn restore_camera_views(world: &mut World, views: &[WorkspaceCameraViewState]) {
    for (index, view) in views.iter().enumerate() {
        let source = view.source.map(GlobalEntityId::from_raw);
        if source.is_some_and(|source| {
            !world
                .get_resource::<lunco_api::registry::ApiEntityRegistry>()
                .is_some_and(|registry| registry.resolve(&source).is_some())
        }) {
            warn!(
                "[workspace-view-state] camera view {} references an unavailable camera {}; leaving that view closed",
                view.id,
                source.map_or(0, |source| source.get())
            );
            continue;
        }
        let before = world
            .resource::<CameraFeeds>()
            .instances
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        world.trigger(CreateCameraFeed {});
        let Some(id) = world
            .resource::<CameraFeeds>()
            .instances
            .keys()
            .find(|id| !before.contains(id))
            .copied()
        else {
            warn!(
                "[workspace-view-state] camera feed capacity prevented restoring view {}",
                view.id
            );
            continue;
        };
        world.trigger(SetCameraFeed {
            feed: Some(id),
            source,
            label: view.label.clone(),
            width: None,
            height: None,
            minimum_scene_items: None,
        });
        if let Some(source) = source {
            if world
                .resource::<CameraFeeds>()
                .instances
                .get(&id)
                .is_none_or(|feed| feed.source != Some(source))
            {
                world.trigger(RemoveCameraFeed { feed: id });
                warn!(
                    "[workspace-view-state] camera view {} could not bind to camera {}",
                    view.id,
                    source.get()
                );
                continue;
            }
        }
        world.trigger(SetCameraFeedResolution {
            feed: id,
            match_window: view.match_window,
            width: Some(view.width),
            height: Some(view.height),
        });
        if view.position.is_some() || view.size.is_some() {
            let position = view.position.unwrap_or([
                16.0 + (index % 3) as f32 * 445.0,
                640.0 + (index / 3) as f32 * 310.0,
            ]);
            let size = view.size.unwrap_or([420.0, 300.0]);
            let layout = CameraFeedWindowLayout { position, size };
            if !layout
                .position
                .iter()
                .chain(layout.size.iter())
                .all(|value| value.is_finite())
                || size[0] < 360.0
                || size[1] < 180.0
            {
                warn!(
                    "[workspace-view-state] camera view {} has invalid saved window geometry; using the default layout",
                    view.id
                );
                continue;
            }
            let mut state = world.resource_mut::<CameraFeedUiState>();
            state.layouts.insert(id, layout);
            state.bump_revision();
        }
    }
}

enum GraphViewSourcesReadiness {
    Ready,
    Waiting,
    Invalid(String),
}

enum CameraViewSourcesReadiness {
    Ready,
    Waiting,
    Invalid(String),
}

fn runtime_view_restore_pending(world: &World, scene_path: &str) -> bool {
    let Some(pending) = world
        .get_resource::<PendingRuntimeViewRestore>()
        .and_then(|pending| pending.0.as_ref())
    else {
        return false;
    };
    let Some(snapshot) = pending
        .snapshots
        .iter()
        .find(|snapshot| snapshot.scene_path == scene_path)
    else {
        return false;
    };
    !pending.cameras_restored
        || snapshot
            .graph_views
            .iter()
            .any(|view| !pending.graphs_restored.contains(&view.id))
}

fn camera_view_sources_readiness(
    world: &World,
    views: &[WorkspaceCameraViewState],
) -> CameraViewSourcesReadiness {
    if views.is_empty() {
        return CameraViewSourcesReadiness::Ready;
    }
    let Some(entities) = world.get_resource::<lunco_api::registry::ApiEntityRegistry>() else {
        return CameraViewSourcesReadiness::Waiting;
    };
    let scene_committed = world
        .get_resource::<lunco_core::SceneTransitionCoordinator>()
        .is_some_and(|scene| {
            scene.active().is_none()
                && !scene.has_admitted()
                && scene.completed_generation().is_some()
        });
    for view in views {
        let Some(source) = view.source.map(GlobalEntityId::from_raw) else {
            continue;
        };
        let Some(entity) = entities.resolve(&source) else {
            return if scene_committed {
                CameraViewSourcesReadiness::Invalid(format!(
                    "camera source {} is unavailable",
                    source.get()
                ))
            } else {
                CameraViewSourcesReadiness::Waiting
            };
        };
        if world.get::<Projection>(entity).is_none() {
            return if scene_committed {
                CameraViewSourcesReadiness::Invalid(format!(
                    "source {} has no camera projection",
                    source.get()
                ))
            } else {
                CameraViewSourcesReadiness::Waiting
            };
        }
    }
    CameraViewSourcesReadiness::Ready
}

fn graph_view_sources_readiness(
    world: &World,
    view: &WorkspaceGraphViewState,
) -> GraphViewSourcesReadiness {
    let Some(entities) = world.get_resource::<lunco_api::registry::ApiEntityRegistry>() else {
        return GraphViewSourcesReadiness::Waiting;
    };
    let Some(signals) = world.get_resource::<lunco_viz::SignalRegistry>() else {
        return GraphViewSourcesReadiness::Waiting;
    };
    for signal in &view.signals {
        let target = GlobalEntityId::from_raw(signal.target);
        let Some(entity) = entities.resolve(&target) else {
            return GraphViewSourcesReadiness::Invalid(format!(
                "owner {} is unavailable",
                target.get()
            ));
        };
        let source = lunco_viz::SignalRef::new(entity, signal.signal.clone());
        match signals.signal_type(&source) {
            None => return GraphViewSourcesReadiness::Waiting,
            Some(lunco_viz::SignalType::Scalar) if !signals.is_active(&source) => {
                return GraphViewSourcesReadiness::Invalid(format!(
                    "scalar channel `{}` has no active publisher",
                    signal.signal
                ));
            }
            Some(lunco_viz::SignalType::Scalar) if signals.scalar_history(&source).is_some() => {}
            Some(lunco_viz::SignalType::Scalar) => {
                return GraphViewSourcesReadiness::Waiting;
            }
            Some(_) => {
                return GraphViewSourcesReadiness::Invalid(format!(
                    "channel `{}` is not scalar",
                    signal.signal
                ));
            }
        }
    }
    GraphViewSourcesReadiness::Ready
}

fn restore_graph_view(world: &mut World, view: &WorkspaceGraphViewState) {
    let before = lunco_viz::graph_hud_workspace_ids(world)
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    world.trigger(lunco_viz::CreateGraphHud {});
    let Some(id) = lunco_viz::graph_hud_workspace_ids(world)
        .into_iter()
        .find(|id| !before.contains(id))
    else {
        warn!(
            "[workspace-view-state] graph HUD capacity prevented restoring view {}",
            view.id
        );
        return;
    };
    world.trigger(lunco_viz::SetGraphHud {
        hud: Some(id),
        visible: Some(view.visible),
        signals: Some(
            view.signals
                .iter()
                .map(|signal| lunco_viz::SignalGridEntry {
                    target_gid: GlobalEntityId::from_raw(signal.target),
                    signal: signal.signal.clone(),
                    caption: signal.caption.clone(),
                })
                .collect(),
        ),
        columns: (!view.signals.is_empty()).then_some(view.columns),
        width: Some(view.width),
        height: Some(view.height),
        x: view.position.map(|position| position[0]),
        y: view.position.map(|position| position[1]),
    });
}
#[on_command(ShowCameraImageSettings)]
fn show_image_settings(
    trigger: On<ShowCameraImageSettings>,
    registry: Res<lunco_api::registry::ApiEntityRegistry>,
    cameras: Query<&Projection>,
    mut state: ResMut<CameraFeedUiState>,
) -> Result<Ack, Reject> {
    if let Some(source) = cmd.source {
        if !registry
            .resolve(&source)
            .is_some_and(|entity| cameras.contains(entity))
        {
            return Err(Reject::InvalidOp(
                "Image settings require a live camera source".into(),
            ));
        }
    }
    state.main_source = cmd.source;
    state.main_open = true;
    Ok(Ack::new(OpId::new()))
}
register_commands!(show_image_settings);

#[derive(Resource, Default)]
pub(super) struct CameraFeedUiState {
    last_match_request: BTreeMap<u64, ([u32; 2], u64)>,
    shader_drafts: BTreeMap<(bool, u64), String>,
    searches: BTreeMap<u64, String>,
    main_open: bool,
    main_source: Option<GlobalEntityId>,
    layouts: BTreeMap<u64, CameraFeedWindowLayout>,
    revision: u64,
}
impl SceneResource for CameraFeedUiState {}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CameraFeedWindowLayout {
    position: [f32; 2],
    size: [f32; 2],
}

impl CameraFeedUiState {
    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

#[derive(Resource, Default)]
struct PendingRuntimeViewRestore(Option<PendingRuntimeViews>);

struct PendingRuntimeViews {
    snapshots: Vec<WorkspaceRuntimeViewSnapshot>,
    progress_scene_path: Option<String>,
    cameras_restored: bool,
    camera_retry_requested: bool,
    graphs_restored: std::collections::BTreeSet<u64>,
    graph_retry_requested: bool,
}

impl PendingRuntimeViews {
    fn take_camera_retry(&mut self) -> bool {
        let retry = self.camera_retry_requested;
        self.camera_retry_requested = false;
        retry
    }

    fn take_graph_retry(&mut self, signal_descriptors_changed: bool) -> bool {
        let retry = self.graph_retry_requested || signal_descriptors_changed;
        self.graph_retry_requested = false;
        retry
    }
}

struct RuntimeViewStateProvider;

impl WorkspaceStateViewProvider for RuntimeViewStateProvider {
    fn revision(&self, world: &World) -> u64 {
        if !lunco_workspace::view_layout_saving_enabled(world) {
            return 0;
        }
        let mut revision = 0xcbf2_9ce4_8422_2325u64;
        if let Some(path) = world.get_resource::<CurrentScenePath>() {
            hash_bytes(&mut revision, path.0.as_bytes());
        }
        if let Some(feeds) = world.get_resource::<CameraFeeds>() {
            revision = revision
                .wrapping_mul(0x100_0000_01b3)
                .wrapping_add(feeds.allocation_revision);
            for (id, feed) in &feeds.instances {
                hash_u64(&mut revision, *id);
                hash_u64(&mut revision, feed.source.map_or(0, |source| source.get()));
                hash_bytes(&mut revision, feed.label.as_bytes());
                hash_u64(&mut revision, u64::from(feed.width));
                hash_u64(&mut revision, u64::from(feed.height));
                hash_u64(&mut revision, u64::from(feed.match_window));
            }
        }
        if let Some(state) = world.get_resource::<CameraFeedUiState>() {
            hash_u64(&mut revision, state.revision);
        }
        hash_u64(
            &mut revision,
            lunco_viz::graph_hud_workspace_revision(world),
        );
        revision
    }

    fn capture(&self, world: &World) -> Option<WorkspaceRuntimeViewSnapshot> {
        if !lunco_workspace::view_layout_saving_enabled(world) {
            return None;
        }
        if world
            .get_resource::<lunco_core::SceneTransitionCoordinator>()
            .is_some_and(|scene| scene.active().is_some() || scene.has_admitted())
        {
            return None;
        }
        let scene_path = world
            .get_resource::<CurrentScenePath>()
            .map(|path| path.0.clone())
            .filter(|path| !path.is_empty())?;
        if runtime_view_restore_pending(world, &scene_path) {
            return None;
        }
        let camera_views = world
            .get_resource::<CameraFeeds>()
            .map(|feeds| {
                feeds
                    .instances
                    .iter()
                    .map(|(id, feed)| {
                        let layout = world
                            .get_resource::<CameraFeedUiState>()
                            .and_then(|state| state.layouts.get(id));
                        WorkspaceCameraViewState {
                            id: *id,
                            source: feed.source.map(|source| source.get()),
                            label: feed.label.clone(),
                            width: feed.width,
                            height: feed.height,
                            match_window: feed.match_window,
                            position: layout.map(|layout| layout.position),
                            size: layout.map(|layout| layout.size),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let graph_views = lunco_viz::graph_hud_workspace_snapshots(world)
            .into_iter()
            .map(|hud| WorkspaceGraphViewState {
                id: hud.id,
                visible: hud.visible,
                columns: hud.columns,
                signals: hud
                    .signals
                    .into_iter()
                    .map(|signal| WorkspaceGraphSignalState {
                        target: signal.target_gid.get(),
                        signal: signal.signal,
                        caption: signal.caption,
                    })
                    .collect(),
                width: hud.width,
                height: hud.height,
                position: hud.position,
            })
            .collect();
        Some(WorkspaceRuntimeViewSnapshot {
            scene_path,
            camera_views,
            graph_views,
        })
    }

    fn restore(&self, world: &mut World, snapshots: Vec<WorkspaceRuntimeViewSnapshot>) {
        let snapshots = if lunco_workspace::view_layout_saving_enabled(world) {
            snapshots
        } else {
            Vec::new()
        };
        world.resource_mut::<PendingRuntimeViewRestore>().0 =
            (!snapshots.is_empty()).then_some(PendingRuntimeViews {
                snapshots,
                progress_scene_path: None,
                cameras_restored: false,
                camera_retry_requested: true,
                graphs_restored: std::collections::BTreeSet::new(),
                graph_retry_requested: true,
            });
    }

    fn reset(&self, world: &mut World) {
        let camera_feeds = world
            .get_resource::<CameraFeeds>()
            .map(|feeds| feeds.instances.keys().copied().collect::<Vec<_>>())
            .unwrap_or_default();
        for feed in camera_feeds {
            world.trigger(RemoveCameraFeed { feed });
        }
        let graph_huds = lunco_viz::graph_hud_workspace_ids(world);
        for hud in graph_huds {
            world.trigger(lunco_viz::RemoveGraphHud { hud });
        }
        if let Some(mut state) = world.get_resource_mut::<CameraFeedUiState>() {
            state.layouts.clear();
            state.bump_revision();
        }
        if let Some(mut pending) = world.get_resource_mut::<PendingRuntimeViewRestore>() {
            pending.0 = None;
        }
    }
}

fn hash_u64(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

pub(super) fn draw_camera_feed(
    mut contexts: EguiContexts,
    scene_path: Res<CurrentScenePath>,
    feeds: Res<CameraFeeds>,
    images: Res<Assets<Image>>,
    theme: Res<lunco_theme::Theme>,
    cameras: Query<
        (
            &GlobalEntityId,
            &Name,
            Option<&Callsign>,
            Option<&CatalogEntryId>,
            Option<&CameraImageEffects>,
        ),
        With<Projection>,
    >,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut state: ResMut<CameraFeedUiState>,
    mut commands: Commands,
    selection: Res<lunco_usd_bevy_camera::camera_switch::CameraSelectionStatus>,
    authored_images: Query<(&bevy_hui::prelude::Tags, &InheritedVisibility)>,
) {
    let previous_layout_count = state.layouts.len();
    state
        .layouts
        .retain(|id, _| feeds.instances.contains_key(id));
    if previous_layout_count != state.layouts.len() {
        state.bump_revision();
    }
    state
        .last_match_request
        .retain(|id, _| feeds.instances.contains_key(id));
    let mut candidates: Vec<_> = cameras
        .iter()
        .map(|(id, name, callsign, catalog, _)| {
            (
                *id,
                name.as_str().to_owned(),
                entity_display_name(Some(name), callsign, catalog),
            )
        })
        .collect();
    let paths: Vec<_> = candidates.iter().map(|(_, path, _)| path.clone()).collect();
    let labels = lunco_camera_core::camera_display_labels(&paths);
    for (candidate, label) in candidates.iter_mut().zip(labels) {
        candidate.2 = label;
    }
    let owner_paths: Vec<_> = selection
        .camera_group_paths
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let owner_labels: BTreeMap<_, _> = owner_paths
        .iter()
        .cloned()
        .zip(lunco_camera_core::camera_display_labels(&owner_paths))
        .collect();
    let group = |path: &String| {
        selection
            .camera_group_paths
            .get(path)
            .and_then(|owner| owner_labels.get(owner))
            .cloned()
            .unwrap_or_else(|| "Scene views".into())
    };
    candidates.sort_by(|a, b| {
        group(&a.1)
            .cmp(&group(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.1.cmp(&b.1))
    });
    state
        .searches
        .retain(|id, _| feeds.instances.contains_key(id));
    state.shader_drafts.retain(|(source, id), _| {
        if *source {
            cameras.iter().any(|(gid, ..)| gid.get() == *id)
        } else {
            feeds.instances.contains_key(id)
        }
    });
    let embedded: std::collections::BTreeSet<u64> = authored_images
        .iter()
        .filter(|(_, visibility)| visibility.get())
        .filter_map(|(tags, _)| tags.tags().get("camera_feed")?.parse().ok())
        .collect();
    for (index, (id, feed)) in feeds.instances.iter().enumerate() {
        // An authored HUD owns presentation of this same native image.
        if embedded.contains(id) {
            continue;
        }
        let target = feed.image.as_ref().and_then(|handle| {
            images.get(handle).map(|image| {
                let texture = contexts.add_image(EguiTextureHandle::Weak(handle.id()));
                (texture, image.width() as f32 / image.height() as f32)
            })
        });
        let selected = candidates
            .iter()
            .find(|(source, _, _)| feed.source == Some(*source));
        let title = if feed.source.is_none() {
            format!("Camera view {id}")
        } else if feed.label.starts_with('/') {
            selected.map_or_else(|| feed.label.clone(), |(_, _, label)| label.clone())
        } else {
            feed.label.clone()
        };
        let Ok(ctx) = contexts.ctx_mut() else {
            return;
        };
        let mut open = true;
        let layout = state.layouts.get(id).copied();
        // At most eight views: this is a bounded rendering/layout conversion.
        let column = (index % 3) as f32;
        let row = (index / 3) as f32;
        let response = egui::Window::new(&title).id(egui::Id::new(("live-camera-feed", scene_path.0.as_str(), id)))
            .default_size(layout.map_or(egui::vec2(420.0,300.0), |layout| egui::vec2(layout.size[0], layout.size[1]))).min_size(egui::vec2(360.0,180.0)).default_pos(layout.map_or(egui::pos2(16.0+column*445.0,640.0+row*310.0), |layout| egui::pos2(layout.position[0], layout.position[1])))
            .constrain_to(ctx.content_rect())
            .title_bar(false).collapsible(false).order(lunco_theme::HudTier::ORDER)
            .frame(lunco_theme::hud_window_frame(&theme)).resizable(true).show(ctx,|ui| {
                ui.horizontal(|ui| {ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                    if ui.small_button("×").on_hover_text("Close this HUD").clicked(){open=false;}
                    if ui.add_enabled(feeds.instances.len()<MAX_CAMERA_FEEDS,egui::Button::new("+").small())
                        .on_hover_text("New independent camera HUD (up to eight)").clicked(){commands.trigger(CreateCameraFeed{});}
                    let mut effects=feed.effects.clone();
                    ui.menu_button("Style",|ui| {
                        let draft=state.shader_drafts.entry((false,*id)).or_insert_with(||effects.shader.clone());
                        if image_controls(ui,&mut effects,draft) {
                            commands.trigger(SetCameraImageEffects{feed:Some(*id),source:None,settings:effects.clone()});
                        }
                    });
                    let label=if feed.resolution_error.is_some(){"Res !"}else if feed.match_window{"Window"}else{"Res"};
                    let menu=ui.menu_button(label,|ui| {
                        ui.strong("Render resolution");
                        ui.weak(format!("Current: {} × {} pixels",feed.width,feed.height));
                        if let Some(error)=&feed.resolution_error {ui.colored_label(ui.visuals().error_fg_color,error);}
                        if ui.selectable_label(feed.match_window,"Match view window (live)").clicked() {
                            state.last_match_request.remove(id);
                            commands.trigger(SetCameraFeedResolution{feed:*id,match_window:true,width:None,height:None});ui.close();
                        }
                        if let Ok(window)=windows.single() {
                            let (width,height)=(window.physical_width(),window.physical_height());
                            if ui.button(format!("Use app window: {width} × {height}")).clicked() {
                                commands.trigger(SetCameraFeedResolution{feed:*id,match_window:false,width:Some(width),height:Some(height)});ui.close();
                            }
                        }
                        ui.separator();
                        for (width,height) in [(480,270),(640,360),(1280,720),(1920,1080),(2560,1440),(3840,2160)] {
                            if ui.selectable_label(!feed.match_window && feed.width==width && feed.height==height,format!("{width} × {height}")).clicked() {
                                commands.trigger(SetCameraFeedResolution{feed:*id,match_window:false,width:Some(width),height:Some(height)});ui.close();
                            }
                        }
                    });
                    menu.response.on_hover_text(feed.resolution_error.as_deref().unwrap_or("Render resolution; Window matches this view's image area in physical pixels"));
                    let selected_text=selected.map_or("Choose camera…",|(_,_,label)|label.as_str());
                    let selected_text=format!("Camera · {selected_text}");
                    egui::ComboBox::from_id_salt(("camera-picker",id)).selected_text(selected_text)
                        .width(ui.available_width()).wrap_mode(egui::TextWrapMode::Truncate).show_ui(ui,|ui| {
                        let search=state.searches.entry(*id).or_default();
                        ui.add(egui::TextEdit::singleline(search).hint_text("Find rover or camera…"));
                        let filter=search.to_lowercase();let mut previous=None;
                        for (source,path,label) in &candidates {
                            let owner=group(path);
                            if !filter.is_empty() && !format!("{owner} {path} {label}").to_lowercase().contains(&filter){continue;}
                            if previous.as_ref()!=Some(&owner){ui.separator();ui.strong(&owner);previous=Some(owner);}
                            if ui.selectable_label(feed.source==Some(*source),label).on_hover_text(path).clicked()
                                && feed.source!=Some(*source) {
                                commands.trigger(SetCameraFeed {feed:Some(*id),source:Some(*source),label:label.clone(),width:None,height:None,minimum_scene_items:None});
                            }
                        }
                    }).response.on_hover_text(&title);
                });});
                ui.add_space(theme.spacing.window_padding);
                let available=ui.available_size();
                if feed.match_window && available.x>0.0 && available.y>0.0 {
                    // Logical UI points become physical render-target pixels only here.
                    let physical=available*ctx.pixels_per_point();
                    if physical.x.is_finite() && physical.y.is_finite() && physical.x<(u32::MAX as f32) && physical.y<(u32::MAX as f32) {
                        let measured=[physical.x.round() as u32,physical.y.round() as u32];
                        if measured==[feed.width,feed.height] {state.last_match_request.remove(id);}
                        else if !ui.input(|i|i.pointer.any_down()) && state.last_match_request.get(id)!=Some(&(measured,feeds.allocation_revision)) {
                            state.last_match_request.insert(*id,(measured,feeds.allocation_revision));
                            commands.trigger(SetCameraFeedResolution{feed:*id,match_window:true,width:Some(measured[0]),height:Some(measured[1])});
                        }
                    }
                } else {state.last_match_request.remove(id);}
                if let Some((texture,ratio))=target {
                    let size=if feed.match_window {available} else {let width=available.x.min(available.y*ratio);egui::vec2(width,width/ratio)};
                    if size.x>0.0 && size.y>0.0 {ui.image((texture,size));}
                } else {ui.weak("Select a camera to start this view.");}

            });
        if let Some(response) = response {
            let rect = response.response.rect;
            let measured = CameraFeedWindowLayout {
                position: [rect.min.x, rect.min.y],
                size: [rect.width(), rect.height()],
            };
            if measured
                .position
                .iter()
                .chain(measured.size.iter())
                .all(|value| value.is_finite())
                && state.layouts.get(id) != Some(&measured)
            {
                state.layouts.insert(*id, measured);
                state.bump_revision();
            }
        }
        if !open {
            commands.trigger(RemoveCameraFeed { feed: *id });
        }
    }
    if state.main_open {
        let source = cameras.iter().find(|(id, name, _, _, _)| {
            state.main_source.map_or_else(
                || selection.active_name.as_deref() == Some(name.as_str()),
                |source| source == **id,
            )
        });
        let Ok(ctx) = contexts.ctx_mut() else {
            return;
        };
        let mut open = state.main_open;
        egui::Window::new("Camera image controls")
            .id(egui::Id::new("camera-image-controls"))
            .open(&mut open)
            .default_width(390.0)
            .show(ctx, |ui| {
                if let Some((source, name, callsign, catalog, settings)) = source {
                    ui.strong(entity_display_name(Some(name), callsign, catalog));
                    ui.label(name.as_str());
                    let mut effects = settings.cloned().unwrap_or_default();
                    let draft = state
                        .shader_drafts
                        .entry((true, source.get()))
                        .or_insert_with(|| effects.shader.clone());
                    if image_controls(ui, &mut effects, draft) {
                        commands.trigger(SetCameraImageEffects {
                            feed: None,
                            source: Some(*source),
                            settings: effects,
                        });
                    }
                } else {
                    ui.label("Select a main camera to edit its image.");
                }
            });
        state.main_open = open;
    }
}

fn image_controls(
    ui: &mut egui::Ui,
    effects: &mut CameraImageEffects,
    shader: &mut String,
) -> bool {
    ui.set_max_width(390.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .selectable_value(&mut effects.mode, CameraImageMode::Color, "Color")
            .changed();
        changed |= ui
            .selectable_value(&mut effects.mode, CameraImageMode::Depth, "Depth")
            .changed();
    });
    if effects.mode == CameraImageMode::Depth {
        ui.weak("Opaque axial depth · metres · near red → yellow → far green; sky black");
        ui.horizontal(|ui| {
            ui.label("Near");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut effects.near_m)
                        .speed(0.1)
                        .suffix(" m")
                        .range(0.0..=effects.far_m - 0.001),
                )
                .changed();
            ui.label("Far");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut effects.far_m)
                        .speed(1.0)
                        .suffix(" m")
                        .range(effects.near_m + 0.001..=1.0e7),
                )
                .changed();
        });
    }
    ui.separator();
    ui.strong("Sensor noise");
    for (label, value) in [
        ("Read noise σ", &mut effects.read_noise),
        ("Shot noise", &mut effects.shot_noise),
        ("Row noise", &mut effects.row_noise),
        ("Dropout fraction", &mut effects.dropout),
    ] {
        changed |= ui
            .add(
                egui::Slider::new(value, 0.0..=1.0)
                    .text(label)
                    .fixed_decimals(3),
            )
            .changed();
    }
    changed |= ui
        .checkbox(&mut effects.animated, "Animate noise")
        .changed();
    ui.horizontal(|ui| {
        ui.label("Seed");
        changed |= ui.add(egui::DragValue::new(&mut effects.seed)).changed();
    });
    let mut quantized = effects.quantization_levels > 0;
    if ui.checkbox(&mut quantized, "Quantize intensity").changed() {
        effects.quantization_levels = if quantized { 256 } else { 0 };
        changed = true;
    }
    if quantized {
        changed |= ui
            .add(
                egui::DragValue::new(&mut effects.quantization_levels)
                    .range(2..=65536)
                    .suffix(" levels"),
            )
            .changed();
    }
    if ui.button("Clear noise").clicked() {
        effects.read_noise = 0.0;
        effects.shot_noise = 0.0;
        effects.row_noise = 0.0;
        effects.dropout = 0.0;
        effects.quantization_levels = 0;
        changed = true;
    }
    ui.collapsing("Shader workflow", |ui| {
        ui.weak("Edit a library WESL shader, then ReloadShader to update live views.");
        ui.text_edit_singleline(shader);
        if ui.button("Apply shader asset").clicked() {
            effects.shader = shader.clone();
            changed = true;
        }
    });
    if let Err(error) = effects.validate() {
        ui.colored_label(ui.visuals().error_fg_color, error);
        return false;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_view(target: u64, signal: &str) -> WorkspaceGraphViewState {
        WorkspaceGraphViewState {
            id: 9,
            visible: true,
            columns: 1,
            signals: vec![WorkspaceGraphSignalState {
                target,
                signal: signal.into(),
                caption: signal.into(),
            }],
            width: 480.0,
            height: 320.0,
            position: Some([24.0, 32.0]),
        }
    }

    #[test]
    fn graph_restore_accepts_an_admitted_empty_scalar_and_rejects_retired_sources() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        let owner_gid = GlobalEntityId::from_raw(42);
        let signal = lunco_viz::SignalRef::new(owner, "late_sensor");
        let mut entities = lunco_api::registry::ApiEntityRegistry::default();
        entities.assign(owner, owner_gid);
        let mut signals = lunco_viz::SignalRegistry::default();
        signals.admit_scalar(&signal, 8, 1).unwrap();
        assert_eq!(signals.scalar_history(&signal).unwrap().len(), 0);
        world.insert_resource(entities);
        world.insert_resource(signals);

        assert!(matches!(
            graph_view_sources_readiness(&world, &graph_view(42, "late_sensor")),
            GraphViewSourcesReadiness::Ready
        ));

        world
            .resource_mut::<lunco_viz::SignalRegistry>()
            .deactivate_signal(&signal);
        assert!(matches!(
            graph_view_sources_readiness(&world, &graph_view(42, "late_sensor")),
            GraphViewSourcesReadiness::Invalid(_)
        ));
    }

    #[test]
    fn graph_restore_waits_for_unpublished_signals_and_reports_missing_owners() {
        let mut world = World::new();
        let owner = world.spawn_empty().id();
        let owner_gid = GlobalEntityId::from_raw(42);
        let mut entities = lunco_api::registry::ApiEntityRegistry::default();
        entities.assign(owner, owner_gid);
        world.insert_resource(entities);
        world.insert_resource(lunco_viz::SignalRegistry::default());

        assert!(matches!(
            graph_view_sources_readiness(&world, &graph_view(42, "late_sensor")),
            GraphViewSourcesReadiness::Waiting
        ));
        assert!(matches!(
            graph_view_sources_readiness(&world, &graph_view(404, "late_sensor")),
            GraphViewSourcesReadiness::Invalid(_)
        ));
    }

    #[test]
    fn graph_restore_retries_only_after_a_request_or_signal_descriptor_change() {
        let mut pending = PendingRuntimeViews {
            snapshots: Vec::new(),
            progress_scene_path: None,
            cameras_restored: false,
            camera_retry_requested: true,
            graphs_restored: Default::default(),
            graph_retry_requested: true,
        };

        assert!(pending.take_camera_retry());
        assert!(!pending.take_camera_retry());
        assert!(pending.take_graph_retry(false));
        assert!(!pending.take_graph_retry(false));
        assert!(pending.take_graph_retry(true));
        assert!(!pending.take_graph_retry(false));
        pending.graph_retry_requested = true;
        assert!(pending.take_graph_retry(false));
    }
}
