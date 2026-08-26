//! Models palette — discover source-backed programs and attach them through the
//! typed USD authoring command.
//!
//! The palette is a front end for [`lunco_usd::AttachProgram`]. It never writes
//! ECS marker components and it never creates a second simulation path. A
//! discovered source with no contract is still attachable as an effects-only
//! program; the author must then declare ports and wires through the USD editor,
//! Rhai, or the HTTP command surface before it becomes a scalar co-simulation
//! participant.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_usd::{LayerId, ProgramAttachSpec, ProgramInput, ProgramOutput, UsdPrimPath};
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

/// A discovered `.mo` or `.py` source that can be offered by the palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgramChoice {
    /// Asset-server address of the source.
    pub asset_path: String,
    /// Display label derived from the discovered path.
    pub label: String,
    /// Source extension without the dot.
    pub extension: String,
}

impl ProgramChoice {
    fn language_label(&self) -> &'static str {
        match self.extension.as_str() {
            "mo" => "Modelica",
            "py" => "Python",
            _ => "Program",
        }
    }

    fn is_python(&self) -> bool {
        self.extension == "py"
    }

    fn source_asset(&self) -> String {
        if self.asset_path.starts_with("lunco://") || self.asset_path.starts_with("twin://") {
            self.asset_path.clone()
        } else {
            lunco_assets::engine_asset_uri(&self.asset_path)
        }
    }

    fn program_name(&self) -> String {
        self.asset_path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .rsplit_once('.')
            .map(|(stem, _)| stem)
            .unwrap_or_default()
            .to_string()
    }

    fn attachment_spec(&self, host_path: &str) -> ProgramAttachSpec {
        let host = host_path.trim_end_matches('/');
        let mut spec = ProgramAttachSpec {
            edit_target: LayerId::root(),
            host_path: host.to_string(),
            name: self.program_name(),
            source_asset: self.source_asset(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            realtime_safe: false,
        };

        // The shipped balloon sources have a documented scalar contract. Keep
        // this content-level convenience here; the Rust command remains generic
        // and arbitrary sources are still attachable without this special case.
        if matches!(
            self.asset_path.as_str(),
            "models/Balloon.mo"
                | "lunco://models/Balloon.mo"
                | "models/GreenBalloon.py"
                | "lunco://models/GreenBalloon.py"
        ) {
            spec.inputs = vec![
                ProgramInput {
                    name: "height".into(),
                    type_name: "float".into(),
                    default_value: None,
                    connection: Some(format!("{host}.outputs:position_y")),
                },
                ProgramInput {
                    name: "velocity".into(),
                    type_name: "float".into(),
                    default_value: None,
                    connection: Some(format!("{host}.outputs:velocity_y")),
                },
                ProgramInput {
                    name: "rho0".into(),
                    type_name: "float".into(),
                    default_value: Some(1.225),
                    connection: None,
                },
                ProgramInput {
                    name: "gravity".into(),
                    type_name: "float".into(),
                    default_value: Some(1.62),
                    connection: None,
                },
            ];
            spec.outputs.push(ProgramOutput {
                name: "netForce".into(),
                type_name: "float".into(),
                connections: vec![format!("{host}.inputs:force_y")],
            });
            spec.realtime_safe = true;
        }

        spec
    }
}

/// Discovered model sources. The resource is rebuilt from the authoritative
/// asset manifest and open-Twin registry, never from a second hardcoded list.
#[derive(Resource, Default)]
pub(crate) struct ProgramCatalog {
    pub ready: bool,
    pub error: Option<String>,
    pub entries: Vec<ProgramChoice>,
}

/// Which program the next scene click will attach.
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachState {
    /// Scene clicks retain their normal behavior.
    #[default]
    Idle,
    /// The selected source will be attached to the next selectable USD prim.
    Pending(ProgramChoice),
}

/// Rebuild the source list from the shared asset discovery owner.
pub(crate) fn refresh_program_catalog(
    manifest: &lunco_assets::discovery::AssetManifest,
    roots: &lunco_assets::twin_source::TwinRoots,
    catalog: &mut ProgramCatalog,
) {
    catalog.ready = manifest.ready();
    catalog.error = None;
    catalog.entries.clear();
    if !catalog.ready {
        return;
    }

    for extension in ["mo", "py"] {
        match lunco_assets::discovery::list_assets(manifest, roots, extension) {
            Ok(entries) => catalog
                .entries
                .extend(entries.into_iter().map(|entry| ProgramChoice {
                    asset_path: entry.asset_path,
                    label: entry.rel,
                    extension: extension.into(),
                })),
            Err(error) => catalog.error = Some(error.to_string()),
        }
    }
    catalog
        .entries
        .sort_by(|a, b| a.asset_path.cmp(&b.asset_path));
}

pub(crate) fn refresh_program_catalog_startup(
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    roots: Res<lunco_assets::twin_source::TwinRoots>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    refresh_program_catalog(&manifest, &roots, &mut catalog);
}

pub(crate) fn refresh_program_catalog_manifest(
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    roots: Res<lunco_assets::twin_source::TwinRoots>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    refresh_program_catalog(&manifest, &roots, &mut catalog);
}

pub(crate) fn refresh_program_catalog_twin_added(
    _trigger: On<lunco_workspace::TwinAdded>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    roots: Res<lunco_assets::twin_source::TwinRoots>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    refresh_program_catalog(&manifest, &roots, &mut catalog);
}

pub(crate) fn refresh_program_catalog_twin_closed(
    _trigger: On<lunco_workspace::TwinClosed>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
    roots: Res<lunco_assets::twin_source::TwinRoots>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    refresh_program_catalog(&manifest, &roots, &mut catalog);
}

// ─────────────────────────────────────────────────────────────────────
// Panel
// ─────────────────────────────────────────────────────────────────────

pub(crate) struct ModelsPalette;

impl Panel for ModelsPalette {
    fn id(&self) -> PanelId {
        PanelId("rover_models")
    }
    fn title(&self) -> String {
        "Models".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(tokens) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.clone())
        else {
            return;
        };
        ctx.panel_content_frame()
            .show(ui, |ui| models_palette_content(ui, ctx, &tokens));
    }
}

fn models_palette_content(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    tokens: &lunco_theme::DesignTokens,
) {
    ui.heading("Models");

    let pending = ctx.resource::<AttachState>().and_then(|state| match state {
        AttachState::Pending(choice) => Some(choice.clone()),
        AttachState::Idle => None,
    });

    if let Some(choice) = pending.as_ref() {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Attach:").color(tokens.success_subdued.linear_multiply(2.0)),
            );
            ui.label(egui::RichText::new(&choice.label).strong());
            if ui.button("Cancel").clicked() {
                ctx.set_resource(AttachState::Idle);
            }
        });
        ui.label(
            egui::RichText::new("Click a USD body in the scene to author the program.")
                .weak()
                .small(),
        );
        ui.separator();
    }

    let Some((catalog_ready, catalog_error, entries)) =
        ctx.resource::<ProgramCatalog>().map(|catalog| {
            (
                catalog.ready,
                catalog.error.clone(),
                catalog.entries.clone(),
            )
        })
    else {
        ui.label("Model catalog unavailable.");
        return;
    };
    if !catalog_ready {
        ui.label(egui::RichText::new("Loading model sources…").weak());
        return;
    }
    if let Some(error) = catalog_error {
        ui.colored_label(tokens.error, format!("Model catalog error: {error}"));
        return;
    }
    if entries.is_empty() {
        ui.label(egui::RichText::new("No .mo or .py sources discovered.").weak());
    }

    for choice in &entries {
        let selected = pending.as_ref() == Some(choice);
        let mut label = format!("{}  ({})", choice.label, choice.language_label());
        let mut enabled = true;
        if choice.is_python() {
            #[cfg(not(feature = "python"))]
            {
                label.push_str(" [requires Python backend]");
                enabled = false;
            }
        }
        let button = egui::Button::new(label)
            .selected(selected)
            .min_size(egui::vec2(ui.available_width(), 24.0));
        if ui.add_enabled(enabled, button).clicked() {
            ctx.set_resource(if selected {
                AttachState::Idle
            } else {
                AttachState::Pending(choice.clone())
            });
        }
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
        "Select a document-backed USD body. The attachment is authored in the USD scene layer and uses the normal projection; sources without declared ports require explicit wiring before they step.",
        )
        .weak()
        .small(),
    );
}

// ─────────────────────────────────────────────────────────────────────
// Input system — applies the pending attachment on 3D click
// ─────────────────────────────────────────────────────────────────────

/// When `AttachState::Pending`, a primary scene click dispatches the typed USD
/// `AttachProgram` command for the selected USD prim. Raw scene files without a
/// doc-backed source are refused; no ECS-only attachment is created.
pub(crate) fn on_scene_click_attach(
    mut click: On<bevy::picking::events::Pointer<bevy::picking::events::Click>>,
    mut state: ResMut<AttachState>,
    keys: Res<ButtonInput<KeyCode>>,
    q_ground: Query<Entity, With<lunco_core::Ground>>,
    q_selectable: Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: Query<&ChildOf>,
    q_prim: Query<&UsdPrimPath>,
    asset_server: Res<AssetServer>,
    backed: Res<lunco_usd::twin_projection::DocBackedTwinScenes>,
    mut commands: Commands,
) {
    use bevy::picking::pointer::PointerButton;
    let AttachState::Pending(choice) = state.clone() else {
        return;
    };
    click.propagate(false);
    if click.button != PointerButton::Primary
        || click.hit.position.is_none()
        || keys.pressed(KeyCode::ShiftLeft)
        || keys.pressed(KeyCode::ShiftRight)
    {
        return;
    }

    let target = find_selectable(click.entity, &q_selectable, &q_parents).unwrap_or(click.entity);
    if q_ground.get(target).is_ok() {
        return;
    }
    let Some(prim) = q_prim.get(target).ok() else {
        bevy::log::warn!("[AttachProgram] selected entity has no USD prim identity");
        *state = AttachState::Idle;
        return;
    };
    let Some(doc) = lunco_usd::twin_projection::scene_document_for(
        &backed,
        &asset_server,
        prim.stage_handle.id(),
    ) else {
        bevy::log::warn!(
            "[AttachProgram] `{}` is not backed by an editable Twin document",
            prim.path
        );
        *state = AttachState::Idle;
        return;
    };

    commands.trigger(lunco_usd::AttachProgram {
        doc,
        spec: choice.attachment_spec(&prim.path),
    });
    *state = AttachState::Idle;
}

/// The `Cancel` intent drops a pending attachment.
pub(crate) fn attach_escape_system(
    mut state: ResMut<AttachState>,
    cancel: lunco_core::CancelIntent,
) {
    if matches!(*state, AttachState::Pending(_)) && cancel.just_pressed() {
        *state = AttachState::Idle;
    }
}

fn find_selectable(
    mut entity: Entity,
    q_selectable: &Query<Entity, With<lunco_core::SelectableRoot>>,
    q_parents: &Query<&ChildOf>,
) -> Option<Entity> {
    loop {
        if q_selectable.get(entity).is_ok() {
            return Some(entity);
        }
        match q_parents.get(entity) {
            Ok(child_of) => entity = child_of.parent(),
            Err(_) => return None,
        }
    }
}
