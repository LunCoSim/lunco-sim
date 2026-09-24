//! Generic authoring evidence panel.
//!
//! This is a presentation consumer of existing owners. It does not classify
//! vehicles, interpret Twin policy, or calculate authored geometry. USD/Rhai
//! provide the authored evidence; the scene diagnostics and camera/selection
//! owners provide live state; this panel only makes the identity chain and
//! temporary inspection layers visible together.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui;
use lunco_camera_core::{OrbitCamera, SpringArmCamera};
use lunco_control_core::ControlLink;
use lunco_core::{entity_display_name, CatalogEntryId, GlobalEntityId, RuntimeDiagnostic};
use lunco_core::{RuntimeDiagnostics, RuntimeFaults, SceneMountState};
use lunco_embodiment_core::roles::{Embodiment, LocalEmbodiment, TheLocalEmbodiment};
use lunco_render::SceneCamera;
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_viewport_core::SceneViewport;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};
use std::collections::HashMap;

use crate::diagnostic_visuals::{DiagnosticVisualKind, DiagnosticVisualStore};
use crate::selection::SelectEntity;

/// One live diagnostic copied into a render-ready panel model.
#[derive(Clone, Debug, Default)]
pub struct AuthoringReviewFinding {
    /// Producer-owned stable diagnostic code.
    pub code: String,
    /// Lowercase severity from the shared diagnostic owner.
    pub severity: String,
    /// Owning subsystem label.
    pub producer: String,
    /// Exact authored/runtime subject, when the producer supplied one.
    pub subject: String,
    /// Human-readable owner message.
    pub message: String,
    /// Live entity identity when the subject is currently projected.
    pub target_id: Option<u64>,
}

/// Change-independent read model consumed by [`AuthoringReviewPanel`].
#[derive(Resource, Clone, Debug, Default)]
pub struct AuthoringReviewView {
    /// Current editor/live selection, kept separate from control authority.
    pub selected: Option<Entity>,
    pub selected_label: String,
    pub selected_path: Option<String>,
    /// Vessel/entity controlled by the local avatar, if any.
    pub controlled: Option<Entity>,
    pub controlled_label: String,
    pub controlled_path: Option<String>,
    /// Target of the active scene camera, if the camera follows something.
    pub camera_target: Option<Entity>,
    pub camera_target_label: String,
    pub camera_target_path: Option<String>,
    pub camera_mode: String,
    /// Non-terminal diagnostics retained by the owning subsystem.
    pub findings: Vec<AuthoringReviewFinding>,
    /// First terminal runtime fault, if the scene is faulted.
    pub fault: Option<(String, String, String)>,
}

/// Coalesces USD identity lifecycle events for the diagnostic target lookup.
#[derive(Resource)]
pub(crate) struct AuthoringReviewTargetIndexDirty(pub bool);

impl Default for AuthoringReviewTargetIndexDirty {
    fn default() -> Self {
        Self(true)
    }
}

/// Coalesces changes to the evidence consumed by the authoring-review panel.
#[derive(Resource)]
pub(crate) struct AuthoringReviewViewDirty(pub bool);

impl Default for AuthoringReviewViewDirty {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Default)]
pub(crate) struct AuthoringReviewResourceSnapshot {
    selected: Option<Option<Entity>>,
    controlled: Option<Option<Entity>>,
    camera: Option<(Option<Entity>, &'static str)>,
    diagnostics: Option<Vec<RuntimeDiagnostic>>,
    fault: Option<Option<(&'static str, String, String)>>,
}

pub(crate) fn mark_target_index_dirty_on_insert<T: Component>(
    _trigger: On<Insert, T>,
    mut dirty: ResMut<AuthoringReviewTargetIndexDirty>,
    mut view_dirty: ResMut<AuthoringReviewViewDirty>,
) {
    dirty.0 = true;
    view_dirty.0 = true;
}

pub(crate) fn mark_target_index_dirty_on_remove<T: Component>(
    _trigger: On<Remove, T>,
    mut dirty: ResMut<AuthoringReviewTargetIndexDirty>,
    mut view_dirty: ResMut<AuthoringReviewViewDirty>,
) {
    dirty.0 = true;
    view_dirty.0 = true;
}

#[derive(SystemParam)]
pub(crate) struct ReviewTargetQueries<'w, 's> {
    links: Query<'w, 's, &'static ControlLink>,
    spring: Query<'w, 's, &'static SpringArmCamera>,
    orbit: Query<'w, 's, &'static OrbitCamera>,
    scene_cameras: Query<'w, 's, (), With<SceneCamera>>,
}

fn is_review_target(
    entity: Entity,
    selected: &SelectedEntities,
    local_avatar: &TheLocalEmbodiment,
    viewport: Option<&SceneViewport>,
    q: &ReviewTargetQueries,
) -> bool {
    if selected.primary() == Some(entity) || local_avatar.0 == Some(entity) {
        return true;
    }
    let controlled = local_avatar
        .0
        .and_then(|avatar| q.links.get(avatar).ok())
        .map(|link| link.target);
    if controlled == Some(entity) {
        return true;
    }
    let Some(camera) = viewport.and_then(|viewport| viewport.active_camera) else {
        return false;
    };
    if camera == entity {
        return true;
    }
    q.spring.get(camera).is_ok_and(|arm| arm.target == entity)
        || q.orbit
            .get(camera)
            .is_ok_and(|orbit| orbit.target == entity)
}

fn mark_authoring_review_view_dirty_on_insert<T: Component>(
    trigger: On<Insert, T>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalEmbodiment>,
    viewport: Option<Res<SceneViewport>>,
    targets: ReviewTargetQueries,
    mut dirty: ResMut<AuthoringReviewViewDirty>,
) {
    if is_review_target(
        trigger.entity,
        &selected,
        &local_avatar,
        viewport.as_deref(),
        &targets,
    ) {
        dirty.0 = true;
    }
}

fn mark_authoring_review_view_dirty_on_remove<T: Component>(
    trigger: On<Remove, T>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalEmbodiment>,
    viewport: Option<Res<SceneViewport>>,
    targets: ReviewTargetQueries,
    mut dirty: ResMut<AuthoringReviewViewDirty>,
) {
    if is_review_target(
        trigger.entity,
        &selected,
        &local_avatar,
        viewport.as_deref(),
        &targets,
    ) {
        dirty.0 = true;
    }
}

pub(crate) fn authoring_review_view_due(
    dirty: Res<AuthoringReviewViewDirty>,
    target_index_dirty: Res<AuthoringReviewTargetIndexDirty>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalEmbodiment>,
    viewport: Option<Res<SceneViewport>>,
    targets: ReviewTargetQueries,
    diagnostics: Res<RuntimeDiagnostics>,
    faults: Res<RuntimeFaults>,
    mut observed: Local<AuthoringReviewResourceSnapshot>,
) -> bool {
    // Several diagnostic producers hold `ResMut` and may republish identical
    // facts on a cadence. Compare semantic content so those no-op writes do not
    // keep this presentation view awake.
    let diagnostics_changed =
        observed.diagnostics.as_deref() != Some(diagnostics.findings.as_slice());
    if diagnostics_changed {
        observed.diagnostics = Some(diagnostics.findings.clone());
    }
    let fault_changed = match (observed.fault.as_ref(), faults.first.as_ref()) {
        (None, _) => true,
        (Some(None), None) => false,
        (Some(Some((kind, subject, detail))), Some(fault)) => {
            *kind != fault.kind || subject != &fault.subject || detail != &fault.detail
        }
        (Some(Some(_)), None) | (Some(None), Some(_)) => true,
    };
    if fault_changed {
        observed.fault = Some(
            faults
                .first
                .as_ref()
                .map(|fault| (fault.kind, fault.subject.clone(), fault.detail.clone())),
        );
    }

    let selected_entity = selected.primary();
    let selected_changed = observed.selected != Some(selected_entity);
    if selected_changed {
        observed.selected = Some(selected_entity);
    }

    let controlled = local_avatar
        .0
        .and_then(|avatar| targets.links.get(avatar).ok().map(|link| link.target));
    let controlled_changed = observed.controlled != Some(controlled);
    if controlled_changed {
        observed.controlled = Some(controlled);
    }

    // SceneViewport is mutably borrowed by camera reconciliation every frame.
    // Compare only the camera facts this view displays, including a followed
    // target that may change in place on the camera component.
    let camera = viewport
        .as_deref()
        .and_then(|viewport| viewport.active_camera)
        .filter(|camera| targets.scene_cameras.get(*camera).is_ok())
        .map(|camera| target_for_camera(camera, &targets.spring, &targets.orbit))
        .unwrap_or((None, "not bound"));
    let camera_changed = observed.camera != Some(camera);
    if camera_changed {
        observed.camera = Some(camera);
    }

    dirty.0
        || target_index_dirty.0
        || selected_changed
        || controlled_changed
        || camera_changed
        || diagnostics_changed
        || fault_changed
}

pub(crate) fn install_view_model_tracking(app: &mut App) {
    app.init_resource::<AuthoringReviewTargetIndexDirty>()
        .init_resource::<AuthoringReviewViewDirty>()
        .add_observer(mark_target_index_dirty_on_insert::<UsdPrimPath>)
        .add_observer(mark_target_index_dirty_on_remove::<UsdPrimPath>)
        .add_observer(mark_target_index_dirty_on_insert::<GlobalEntityId>)
        .add_observer(mark_target_index_dirty_on_remove::<GlobalEntityId>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<Name>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<Name>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<lunco_core::markers::Callsign>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<lunco_core::markers::Callsign>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<CatalogEntryId>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<CatalogEntryId>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<ControlLink>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<ControlLink>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<SpringArmCamera>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<SpringArmCamera>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<OrbitCamera>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<OrbitCamera>)
        .add_observer(mark_authoring_review_view_dirty_on_insert::<SceneCamera>)
        .add_observer(mark_authoring_review_view_dirty_on_remove::<SceneCamera>);
}

fn index_target_ids_by_path<'a>(
    entities: impl IntoIterator<Item = (&'a str, Option<u64>)>,
) -> HashMap<&'a str, Option<u64>> {
    let mut index: HashMap<&'a str, Option<u64>> = HashMap::new();
    for (path, target_id) in entities {
        index
            .entry(path)
            .and_modify(|existing| *existing = None)
            .or_insert(target_id);
    }
    index
}

fn label(
    entity: Entity,
    q_name: &Query<&Name>,
    q_callsign: &Query<&lunco_core::markers::Callsign>,
    q_catalog_id: &Query<&CatalogEntryId>,
    q_gid: &Query<&GlobalEntityId>,
) -> String {
    let value = entity_display_name(
        q_name.get(entity).ok(),
        q_callsign.get(entity).ok(),
        q_catalog_id.get(entity).ok(),
    );
    if value.is_empty() {
        q_gid
            .get(entity)
            .map(|id| format!("entity #{}", id.get()))
            .unwrap_or_else(|_| format!("{:?}", entity))
    } else {
        value
    }
}

fn path(entity: Entity, q_paths: &Query<&UsdPrimPath>) -> Option<String> {
    q_paths.get(entity).ok().map(|path| path.path.clone())
}

fn target_for_camera(
    camera: Entity,
    q_spring: &Query<&SpringArmCamera>,
    q_orbit: &Query<&OrbitCamera>,
) -> (Option<Entity>, &'static str) {
    if let Ok(follow) = q_spring.get(camera) {
        return (Some(follow.target), "follow");
    }
    if let Ok(orbit) = q_orbit.get(camera) {
        return (Some(orbit.target), "orbit");
    }
    (None, "free flight")
}

#[derive(SystemParam)]
pub(crate) struct AuthoringReviewQueries<'w, 's> {
    avatar: Query<'w, 's, (), (With<Embodiment>, With<LocalEmbodiment>)>,
    links: Query<'w, 's, &'static ControlLink>,
    spring: Query<'w, 's, &'static SpringArmCamera>,
    orbit: Query<'w, 's, &'static OrbitCamera>,
    paths: Query<'w, 's, &'static UsdPrimPath>,
    entities: Query<
        'w,
        's,
        (
            Entity,
            &'static UsdPrimPath,
            Option<&'static GlobalEntityId>,
        ),
    >,
    scene_cameras: Query<'w, 's, (), With<SceneCamera>>,
    names: Query<'w, 's, &'static Name>,
    callsigns: Query<'w, 's, &'static lunco_core::markers::Callsign>,
    catalog_ids: Query<'w, 's, &'static CatalogEntryId>,
    gids: Query<'w, 's, &'static GlobalEntityId>,
}

/// Produce the compact evidence model. The live target chain is O(1); the
/// path-to-entity lookup only scans projected USD paths while a diagnostic
/// resource is changing, and is reused by the panel for one-click selection.
pub(crate) fn populate_authoring_review_view(
    mut view: ResMut<AuthoringReviewView>,
    selected: Res<SelectedEntities>,
    local_avatar: Res<TheLocalEmbodiment>,
    viewport: Option<Res<SceneViewport>>,
    diagnostics: Res<RuntimeDiagnostics>,
    faults: Res<RuntimeFaults>,
    mut target_index_dirty: ResMut<AuthoringReviewTargetIndexDirty>,
    mut view_dirty: ResMut<AuthoringReviewViewDirty>,
    q: AuthoringReviewQueries,
) {
    view_dirty.0 = false;
    let selected_entity = selected.primary();
    view.selected = selected_entity;
    view.selected_label = selected_entity
        .map(|entity| label(entity, &q.names, &q.callsigns, &q.catalog_ids, &q.gids))
        .unwrap_or_default();
    view.selected_path = selected_entity.and_then(|entity| path(entity, &q.paths));

    let controlled = local_avatar
        .0
        .filter(|avatar| q.avatar.get(*avatar).is_ok())
        .and_then(|avatar| q.links.get(avatar).ok().map(|link| link.target));
    view.controlled = controlled;
    view.controlled_label = controlled
        .map(|entity| label(entity, &q.names, &q.callsigns, &q.catalog_ids, &q.gids))
        .unwrap_or_else(|| "free flight".into());
    view.controlled_path = controlled.and_then(|entity| path(entity, &q.paths));

    let camera = viewport.as_deref().and_then(|state| state.active_camera);
    if let Some(camera) = camera.filter(|entity| q.scene_cameras.get(*entity).is_ok()) {
        let (target, mode) = target_for_camera(camera, &q.spring, &q.orbit);
        view.camera_target = target;
        view.camera_mode = mode.into();
        view.camera_target_label = target
            .map(|entity| label(entity, &q.names, &q.callsigns, &q.catalog_ids, &q.gids))
            .unwrap_or_else(|| "none".into());
        view.camera_target_path = target.and_then(|entity| path(entity, &q.paths));
    } else {
        view.camera_target = None;
        view.camera_mode = "not bound".into();
        view.camera_target_label.clear();
        view.camera_target_path = None;
    }

    if diagnostics.is_changed() || target_index_dirty.0 {
        target_index_dirty.0 = false;
        let target_ids_by_path = index_target_ids_by_path(
            q.entities
                .iter()
                .map(|(_, path, id)| (path.path.as_str(), id.map(GlobalEntityId::get))),
        );
        view.findings = diagnostics
            .findings
            .iter()
            .map(|finding| {
                let target_id = target_ids_by_path
                    .get(finding.subject.as_str())
                    .copied()
                    .flatten();
                AuthoringReviewFinding {
                    code: finding.code.clone(),
                    severity: finding.severity.as_str().into(),
                    producer: finding.producer.clone(),
                    subject: finding.subject.clone(),
                    message: finding.message.clone(),
                    target_id,
                }
            })
            .collect();
    }

    view.fault = faults.first.as_ref().map(|fault| {
        (
            fault.kind.into(),
            fault.subject.clone(),
            fault.detail.clone(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource, Default)]
    struct ViewModelRuns(u32);

    fn republish_empty_diagnostics(mut diagnostics: ResMut<RuntimeDiagnostics>) {
        diagnostics.replace_producer("stable", std::iter::empty());
    }

    fn touch_scene_viewport(mut viewport: ResMut<SceneViewport>) {
        let active_camera = viewport.active_camera;
        viewport.active_camera = active_camera;
    }

    fn record_view_model_run(
        mut runs: ResMut<ViewModelRuns>,
        mut dirty: ResMut<AuthoringReviewViewDirty>,
        mut target_index_dirty: ResMut<AuthoringReviewTargetIndexDirty>,
    ) {
        runs.0 += 1;
        dirty.0 = false;
        target_index_dirty.0 = false;
    }

    #[test]
    fn authoring_review_view_gate_sleeps_until_an_owner_or_identity_changes() {
        let mut app = App::new();
        app.init_resource::<SelectedEntities>()
            .init_resource::<TheLocalEmbodiment>()
            .init_resource::<SceneViewport>()
            .init_resource::<RuntimeDiagnostics>()
            .init_resource::<RuntimeFaults>()
            .init_resource::<ViewModelRuns>();
        install_view_model_tracking(&mut app);
        app.add_systems(
            Update,
            (
                republish_empty_diagnostics,
                touch_scene_viewport,
                record_view_model_run.run_if(authoring_review_view_due),
            )
                .chain(),
        );

        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 1);
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 1);

        let selected = app.world_mut().spawn_empty().id();
        app.world_mut()
            .resource_mut::<SelectedEntities>()
            .entities
            .push(selected);
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 2);

        app.world_mut()
            .entity_mut(selected)
            .insert(Name::new("renamed target"));
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 3);

        app.world_mut()
            .spawn(Name::new("unrelated streamed entity"));
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 3);

        app.world_mut()
            .resource_mut::<RuntimeDiagnostics>()
            .replace_producer(
                "test",
                [RuntimeDiagnostic {
                    code: "test-finding".into(),
                    severity: lunco_core::DiagnosticSeverity::Warning,
                    producer: "test".into(),
                    subject: "/Target".into(),
                    message: "changed".into(),
                }],
            );
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 4);
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 4);

        app.world_mut().resource_mut::<RuntimeFaults>().raise(
            "test-fault",
            Some(selected),
            "/Target",
            "changed",
        );
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 5);
    }

    #[test]
    fn authoring_review_view_gate_tracks_camera_target_not_camera_or_viewport_writes() {
        let mut app = App::new();
        app.init_resource::<SelectedEntities>()
            .init_resource::<TheLocalEmbodiment>()
            .init_resource::<SceneViewport>()
            .init_resource::<RuntimeDiagnostics>()
            .init_resource::<RuntimeFaults>()
            .init_resource::<ViewModelRuns>();
        install_view_model_tracking(&mut app);
        app.add_systems(
            Update,
            (
                touch_scene_viewport,
                record_view_model_run.run_if(authoring_review_view_due),
            )
                .chain(),
        );

        let first_target = app.world_mut().spawn_empty().id();
        let next_target = app.world_mut().spawn_empty().id();
        let camera = app
            .world_mut()
            .spawn((
                SceneCamera::default(),
                SpringArmCamera {
                    target: first_target,
                    distance: 20.0,
                    yaw: 0.0,
                    pitch: 0.0,
                    damping: None,
                    vertical_offset: 0.0,
                    track_heading: false,
                    attitude: Default::default(),
                },
            ))
            .id();
        app.world_mut()
            .resource_mut::<SceneViewport>()
            .active_camera = Some(camera);

        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 1);
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 1);

        app.world_mut()
            .get_mut::<SpringArmCamera>(camera)
            .unwrap()
            .yaw = 0.5;
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 1);

        app.world_mut()
            .get_mut::<SpringArmCamera>(camera)
            .unwrap()
            .target = next_target;
        app.update();
        assert_eq!(app.world().resource::<ViewModelRuns>().0, 2);
    }

    #[test]
    fn diagnostic_targets_use_one_path_index_and_leave_duplicate_instances_ambiguous() {
        let index = index_target_ids_by_path([
            ("/Scene/Unique", Some(12)),
            ("/Scene/Pending", None),
            ("/Scene/Repeated", Some(34)),
            ("/Scene/Repeated", Some(56)),
        ]);

        assert_eq!(index.get("/Scene/Unique"), Some(&Some(12)));
        assert_eq!(index.get("/Scene/Pending"), Some(&None));
        assert_eq!(index.get("/Scene/Repeated"), Some(&None));
        assert_eq!(index.get("/Scene/Missing"), None);
    }
}

fn entity_row(ui: &mut egui::Ui, name: &str, label: &str, path: &Option<String>) {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.strong(label);
    });
    if let Some(path) = path {
        ui.small(path);
    }
}

fn toggle_builtin(
    ui: &mut egui::Ui,
    store: &mut DiagnosticVisualStore,
    root: Option<Entity>,
    kind: DiagnosticVisualKind,
    label: &str,
) {
    let mut enabled = store.builtin_enabled(kind);
    if ui.checkbox(&mut enabled, label).changed() {
        store.set_builtin(kind, enabled, root);
    }
}

/// One discoverable read-only evidence surface for Editor and Build users.
pub struct AuthoringReviewPanel;

impl Panel for AuthoringReviewPanel {
    fn id(&self) -> PanelId {
        PanelId("authoring_review")
    }

    fn title(&self) -> String {
        "Authoring Review".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn menu_group(&self) -> PanelMenuGroup {
        PanelMenuGroup::Editor
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        ui.heading("Authoring review");
        ui.small("Selection, control authority, camera target, diagnostics, and inspection layers");
        ui.separator();

        let Some(view) = ctx.resource::<AuthoringReviewView>().cloned() else {
            ui.weak("Review state is unavailable");
            return;
        };

        ui.collapsing("Active target chain", |ui| {
            entity_row(ui, "Selected", &view.selected_label, &view.selected_path);
            entity_row(
                ui,
                "Controlled",
                &view.controlled_label,
                &view.controlled_path,
            );
            ui.horizontal(|ui| {
                ui.label("Camera");
                ui.strong(format!(
                    "{} → {}",
                    view.camera_mode, view.camera_target_label
                ));
            });
            if let Some(path) = &view.camera_target_path {
                ui.small(path);
            }
            ui.small(
                "These are independent states; selection never implies control or camera focus.",
            );
        });

        ui.collapsing("Inspection layers", |ui| {
            let root = ctx
                .resource::<SceneMountState>()
                .and_then(SceneMountState::active_root);
            let Some(mut store) = ctx.resource::<DiagnosticVisualStore>().cloned() else {
                ui.weak("Diagnostic visual owner is unavailable");
                return;
            };
            toggle_builtin(ui, &mut store, root, DiagnosticVisualKind::Joints, "Joints");
            toggle_builtin(
                ui,
                &mut store,
                root,
                DiagnosticVisualKind::PhysicsFrames,
                "Physics frames",
            );
            toggle_builtin(
                ui,
                &mut store,
                root,
                DiagnosticVisualKind::PhysicsMass,
                "Mass / inertia",
            );
            toggle_builtin(
                ui,
                &mut store,
                root,
                DiagnosticVisualKind::PhysicsForces,
                "Forces",
            );
            toggle_builtin(
                ui,
                &mut store,
                root,
                DiagnosticVisualKind::WheelForces,
                "Wheel forces",
            );

            if let Some(target) = view.selected {
                let mut collider = store.targeted_enabled(target, DiagnosticVisualKind::Collider);
                if ui.checkbox(&mut collider, "Collision geometry").changed() {
                    if collider {
                        ctx.trigger(crate::diagnostic_visuals::AcquireDiagnosticVisual {
                            target,
                            kind: "collider".into(),
                            policy: "default".into(),
                        });
                    } else if let Some(lease) =
                        store.targeted_lease(target, DiagnosticVisualKind::Collider)
                    {
                        ctx.trigger(crate::diagnostic_visuals::ReleaseDiagnosticVisual { lease });
                    }
                }
            } else {
                ui.weak("Select a projected scene body to inspect its collision geometry");
            }
            ctx.set_resource(store);
        });

        ui.collapsing("Runtime diagnostics", |ui| {
            if let Some((kind, subject, detail)) = &view.fault {
                ui.colored_label(
                    egui::Color32::LIGHT_RED,
                    format!("FAULT · {kind} · {subject}"),
                );
                ui.small(detail);
            }
            if view.findings.is_empty() {
                ui.weak("No active runtime diagnostics");
                return;
            }
            let mut select = None;
            for finding in &view.findings {
                let color = match finding.severity.as_str() {
                    "error" => egui::Color32::LIGHT_RED,
                    "warning" => egui::Color32::YELLOW,
                    _ => egui::Color32::LIGHT_BLUE,
                };
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        ui.colored_label(color, finding.severity.to_uppercase());
                        ui.strong(&finding.code);
                        ui.weak(&finding.producer);
                    });
                    if !finding.subject.is_empty() {
                        ui.small(&finding.subject);
                    }
                    ui.label(&finding.message);
                    if let Some(target_id) = finding.target_id {
                        if ui.small_button("Select subject").clicked() {
                            select = Some(target_id);
                        }
                    }
                });
            }
            if let Some(entity_id) = select {
                ctx.trigger(SelectEntity {
                    entity_id,
                    extend: false,
                    toggle: false,
                    remove_only: false,
                    inspector_part_entity_id: 0,
                });
            }
        });

        ui.separator();
        ui.small("Geometry measurements, tolerances, and requirement policy are authored in Rhai over QueryUsdPrim; this panel never guesses dimensions.");
    }
}
