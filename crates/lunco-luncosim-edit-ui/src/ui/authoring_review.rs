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
use lunco_core::{
    entity_display_name, Avatar, CatalogEntryId, GlobalEntityId, LocalAvatar, RuntimeDiagnostics,
    RuntimeFaults, SceneMountState, SceneViewport, TheLocalAvatar,
};
use lunco_cosim_core::ControlLink;
use lunco_render::SceneCamera;
use lunco_scene_selection::SelectedEntities;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelMenuGroup, PanelSlot};

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
    avatar: Query<'w, 's, (), (With<Avatar>, With<LocalAvatar>)>,
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
    local_avatar: Res<TheLocalAvatar>,
    viewport: Option<Res<SceneViewport>>,
    diagnostics: Res<RuntimeDiagnostics>,
    faults: Res<RuntimeFaults>,
    mount: Option<Res<SceneMountState>>,
    q: AuthoringReviewQueries,
) {
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

    if diagnostics.is_changed()
        || view
            .findings
            .iter()
            .any(|finding| finding.target_id.is_none())
    {
        view.findings = diagnostics
            .findings
            .iter()
            .map(|finding| {
                let target_id = q
                    .entities
                    .iter()
                    .find(|(_, path, _)| path.path == finding.subject)
                    .and_then(|(_, _, id)| id.map(GlobalEntityId::get));
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

    // Keep the optional resource in the signature so the read model remains
    // explicitly scoped to the active Twin mount. It also makes the no-scene
    // state visible to future consumers without inventing a global target.
    let _ = mount;
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
        PanelMenuGroup::Design
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
                });
            }
        });

        ui.separator();
        ui.small("Geometry measurements, tolerances, and requirement policy are authored in Rhai over QueryUsdPrim; this panel never guesses dimensions.");
    }
}
