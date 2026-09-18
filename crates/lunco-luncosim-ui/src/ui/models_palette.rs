//! Models palette — discover source-backed programs and attach them through the
//! typed USD authoring command.
//!
//! The palette is a front end for [`lunco_usd_core::commands::AttachProgram`]. It never writes
//! ECS marker components and it never creates a second simulation path. A
//! discovered source with no contract is still attachable as an effects-only
//! program; the author must then declare ports and wires through the USD editor,
//! Rhai, or the HTTP command surface before it becomes a scalar co-simulation
//! participant.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_usd_bevy_scene::UsdPrimPath;
use lunco_usd_core::program::{ProgramAttachSpec, ProgramInput, ProgramOutput};
use lunco_usd_document::document::LayerId;
use lunco_workbench_core::{Panel, PanelCtx, PanelId, PanelSlot};
use serde::Deserialize;

const PROGRAM_CONTRACTS_KIND: &str = "lunco.program-contracts.v1";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProgramContractManifest {
    #[serde(rename = "kind")]
    _kind: String,
    programs: Vec<ProgramContract>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProgramContract {
    source_asset: String,
    #[serde(default)]
    inputs: Vec<ProgramInputContract>,
    #[serde(default)]
    outputs: Vec<ProgramOutputContract>,
    #[serde(default)]
    realtime_safe: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProgramInputContract {
    name: String,
    type_name: String,
    #[serde(default)]
    default_value: Option<f64>,
    #[serde(default)]
    connection: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProgramOutputContract {
    name: String,
    type_name: String,
    #[serde(default)]
    connections: Vec<String>,
}

/// A discovered `.mo` or `.py` source that can be offered by the palette.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgramChoice {
    /// Asset-server address of the source.
    pub asset_path: String,
    /// Display label derived from the discovered path.
    pub label: String,
    /// Source extension without the dot.
    pub extension: String,
    /// Optional authored port contract for this source.
    contract: Option<ProgramContract>,
}

impl ProgramChoice {
    fn language_label(&self) -> &'static str {
        match self.extension.as_str() {
            "mo" => "Modelica",
            "py" => "Python",
            _ => "Program",
        }
    }

    #[cfg(not(feature = "python"))]
    fn is_python(&self) -> bool {
        self.extension == "py"
    }

    fn source_asset(&self) -> String {
        if self.asset_path.starts_with("lunco://") || self.asset_path.starts_with("twin://") {
            self.asset_path.clone()
        } else {
            lunco_assets_core::engine_asset_uri(&self.asset_path)
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

        if let Some(contract) = &self.contract {
            spec.inputs = contract
                .inputs
                .iter()
                .map(|input| ProgramInput {
                    name: input.name.clone(),
                    type_name: input.type_name.clone(),
                    default_value: input.default_value,
                    connection: input
                        .connection
                        .as_deref()
                        .map(|connection| connection.replace("{host}", host)),
                })
                .collect();
            spec.outputs = contract
                .outputs
                .iter()
                .map(|output| ProgramOutput {
                    name: output.name.clone(),
                    type_name: output.type_name.clone(),
                    connections: output
                        .connections
                        .iter()
                        .map(|connection| connection.replace("{host}", host))
                        .collect(),
                })
                .collect();
            spec.realtime_safe = contract.realtime_safe;
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
    contracts_ready: bool,
    contracts_error: Option<String>,
    contracts: Vec<ProgramContract>,
}

/// Which program the next scene click will attach.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub(crate) enum AttachState {
    /// Scene clicks retain their normal behavior.
    #[default]
    Idle,
    /// The selected source will be attached to the next selectable USD prim.
    Pending(ProgramChoice),
}

/// Publish the program source projection from the shared catalog listing.
pub(crate) fn drain_program_catalog(
    mut scan: ResMut<lunco_scene_catalog::catalog::CatalogScan>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    let Some(result) = lunco_scene_catalog::catalog::take_program_listing(&mut scan) else {
        return;
    };
    match result {
        Ok((manifest_ready, assets)) => {
            catalog.ready = manifest_ready;
            catalog.error = None;
            catalog.entries = assets
                .into_iter()
                .filter_map(|asset| {
                    let extension = asset.rel.rsplit_once('.')?.1.to_string();
                    Some(ProgramChoice {
                        asset_path: asset.asset_path,
                        label: asset.rel,
                        extension,
                        contract: None,
                    })
                })
                .collect();
            catalog
                .entries
                .sort_by(|a, b| a.asset_path.cmp(&b.asset_path));
            apply_program_contracts(&mut catalog);
        }
        Err(error) => {
            catalog.ready = false;
            catalog.entries.clear();
            catalog.error = Some(error.to_string());
        }
    }
}

/// Load authored program contracts through the shared text-asset catalog and
/// attach them to the discovered source choices. A source without a contract
/// remains a valid generic effects-only program; a malformed contract is
/// reported without preventing unrelated sources from being listed.
pub(crate) fn sync_program_contracts(
    text_catalog: Option<Res<lunco_assets_core::TextAssetCatalog>>,
    text_assets: Option<Res<Assets<lunco_assets_core::TextAsset>>>,
    asset_server: Option<Res<AssetServer>>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    if catalog.contracts_ready {
        return;
    }
    let (Some(text_catalog), Some(text_assets), Some(asset_server)) =
        (text_catalog, text_assets, asset_server)
    else {
        return;
    };
    if !text_catalog.ready() {
        return;
    }

    let mut pending = false;
    let mut manifests = Vec::new();
    for entry in text_catalog.entries() {
        let Some(asset) = text_assets.get(&entry.handle) else {
            if !asset_server
                .get_load_state(entry.handle.id())
                .is_some_and(|state| state.is_failed())
            {
                pending = true;
            }
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&asset.text) else {
            continue;
        };
        if value.get("kind").and_then(serde_json::Value::as_str) != Some(PROGRAM_CONTRACTS_KIND) {
            continue;
        }
        match serde_json::from_value::<ProgramContractManifest>(value) {
            Ok(manifest) => manifests.push(manifest),
            Err(error) => {
                catalog.contracts_error = Some(format!(
                    "invalid program contract manifest `{}`: {error}",
                    entry.asset_path
                ));
            }
        }
    }
    if pending {
        return;
    }
    if manifests.len() > 1 {
        catalog.contracts_error = Some(format!(
            "more than one asset is marked {PROGRAM_CONTRACTS_KIND}"
        ));
    } else if let Some(manifest) = manifests.pop() {
        catalog.contracts = manifest.programs;
    } else {
        catalog.contracts_error = Some(format!(
            "runtime asset listing contains no asset marked {PROGRAM_CONTRACTS_KIND}"
        ));
    }
    catalog.contracts_ready = true;
    apply_program_contracts(&mut catalog);
}

fn apply_program_contracts(catalog: &mut ProgramCatalog) {
    for choice in &mut catalog.entries {
        choice.contract = catalog
            .contracts
            .iter()
            .find(|contract| {
                lunco_assets_core::engine_asset_rel(&contract.source_asset)
                    == lunco_assets_core::engine_asset_rel(&choice.asset_path)
            })
            .cloned();
    }
}

/// Retire Twin-derived program choices at the lifecycle boundary. The next
/// shared listing repopulates the picker for the surviving/opened Twin set.
pub(crate) fn clear_program_catalog_on_twin_closed(
    _trigger: On<lunco_workspace::TwinClosed>,
    mut catalog: ResMut<ProgramCatalog>,
) {
    catalog.ready = false;
    catalog.error = None;
    catalog.entries.clear();
    catalog.contracts_ready = false;
    catalog.contracts_error = None;
    catalog.contracts.clear();
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
    fn menu_group(&self) -> lunco_workbench_core::PanelMenuGroup {
        lunco_workbench_core::PanelMenuGroup::Scene
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

    let Some((catalog_ready, catalog_error, contracts_error, entries)) =
        ctx.resource::<ProgramCatalog>().map(|catalog| {
            (
                catalog.ready,
                catalog.error.clone(),
                catalog.contracts_error.clone(),
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
    if let Some(error) = contracts_error {
        ui.colored_label(tokens.warning, format!("Model contract catalog: {error}"));
    }
    if entries.is_empty() {
        ui.label(egui::RichText::new("No .mo or .py sources discovered.").weak());
    }

    for choice in &entries {
        let selected = pending.as_ref() == Some(choice);
        #[cfg(feature = "python")]
        let (label, enabled) = (
            format!("{}  ({})", choice.label, choice.language_label()),
            true,
        );
        #[cfg(not(feature = "python"))]
        let (label, enabled) = {
            let mut label = format!("{}  ({})", choice.label, choice.language_label());
            let enabled = if choice.is_python() {
                label.push_str(" [requires Python backend]");
                false
            } else {
                true
            };
            (label, enabled)
        };
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
    backed: Res<lunco_usd_bevy_twin::DocBackedTwinScenes>,
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
    let Some(doc) =
        lunco_usd_bevy_twin::scene_document_for(&backed, &asset_server, prim.stage_handle.id())
    else {
        bevy::log::warn!(
            "[AttachProgram] `{}` is not backed by an editable Twin document",
            prim.path
        );
        *state = AttachState::Idle;
        return;
    };

    commands.trigger(lunco_usd_core::commands::AttachProgram {
        doc_id: doc,
        spec: choice.attachment_spec(&prim.path),
    });
    *state = AttachState::Idle;
}

/// The `Cancel` intent drops a pending attachment.
pub(crate) fn attach_escape_system(
    mut state: ResMut<AttachState>,
    cancel: lunco_control_core::CancelIntent,
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
