//! Inspector panel — shows the canvas's current selection and lets the
//! user edit a component's modifications (parameter overrides).
//!
//! ## Architecture
//!
//! - **Reads** [`crate::ui::panels::canvas_diagram::CanvasDiagramState`] —
//!   the canvas owns selection state. The `primary()` selection's
//!   `Node(id)` is mapped through the scene to the Modelica instance
//!   name (`Node.origin`), then cross-referenced against the doc's AST
//!   to find the matching `Component` declaration.
//! - **Writes** via the unified [`crate::api_edits::ApplyModelicaOps`]
//!   Reflect event with [`crate::api_edits::ApiOp::SetParameter`]. The
//!   GUI never mutates state directly; per AGENTS.md §4.1 every edit
//!   goes through the same command surface an external API caller
//!   would use.
//!
//! ## What it shows
//!
//! For the selected component:
//! - instance name + declared type
//! - list of modifications (`R = 10`, `unit = "kg"`, …) with
//!   editable values
//!
//! Description strings, port lists, and the broader class structure
//! are deliberately not surfaced here — the agent-facing
//! `describe_model` API endpoint is the canonical place for those.
//! The inspector is the lightweight per-selection editor.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_canvas::SelectItem;
use lunco_workbench::{Panel, PanelId, PanelSlot};

use crate::api_edits::{ApiOp, ApplyModelicaOps};

pub struct InspectorPanel;

impl Panel for InspectorPanel {
    fn id(&self) -> PanelId {
        PanelId("modelica_diagram_inspector")
    }

    fn title(&self) -> String {
        "Inspector".into()
    }

    fn default_slot(&self) -> PanelSlot {
        PanelSlot::RightInspector
    }

    fn render(&mut self, ui: &mut egui::Ui, world: &mut World) {
        // ── Resolve target doc ───────────────────────────────────
        let active_doc = world
            .get_resource::<lunco_workbench::WorkspaceResource>()
            .and_then(|ws| ws.active_document);
        let Some(doc_id) = active_doc else {
            placeholder(ui, "No active document.");
            return;
        };

        // ── Resolve the selected node's Modelica instance name ──
        //
        // Edges are not currently inspectable — we only surface
        // component-level edits. Selecting a wire shows a "no editor
        // for this selection" message rather than a blank panel.
        let mut selection_kind = "none";
        let primary_node_origin = world
            .get_resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>()
            .and_then(|cs| {
                let docstate = cs.get(Some(doc_id));
                let primary = docstate.canvas.selection.primary()?;
                match primary {
                    SelectItem::Node(node_id) => {
                        selection_kind = "node";
                        docstate.canvas.scene.node(node_id)?.origin.clone()
                    }
                    SelectItem::Edge(_) => {
                        selection_kind = "edge";
                        None
                    }
                }
            });

        let Some(instance_name) = primary_node_origin else {
            match selection_kind {
                "edge" => placeholder(ui, "Wire editing not supported yet."),
                _ => placeholder(ui, "Select a node on the canvas."),
            }
            return;
        };

        // ── Resolve the active class on this doc ────────────────
        //
        // Mirrors `canvas_diagram::active_class_for_doc`: the
        // drilled-in pin wins when set, otherwise the document's
        // first non-package class (the same default the canvas
        // projects).
        let drilled_in = world
            .get_resource::<crate::ui::panels::canvas_diagram::DrilledInClassNames>()
            .and_then(|m| m.get(doc_id).map(str::to_string));

        // Scope the registry borrow tightly so we can free it before
        // any subsequent `world.commands()` calls.
        let (component_info, class) = {
            let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
            let Some(host) = registry.host(doc_id) else {
                placeholder(ui, "Document not in registry.");
                return;
            };
            let Some(ast) = host.document().ast().result.as_ref().ok().cloned() else {
                placeholder(ui, "Document has no parsed AST.");
                return;
            };
            let Some(class) = drilled_in.or_else(|| {
                crate::ast_extract::extract_model_name_from_ast(&ast)
            }) else {
                placeholder(ui, "Could not resolve target class.");
                return;
            };
            let short = class.rsplit('.').next().unwrap_or(&class).to_string();
            let Some(class_def) = crate::ast_extract::find_class_by_short_name(&ast, &short) else {
                placeholder(
                    ui,
                    &format!("Class `{short}` not found in document."),
                );
                return;
            };
            // Pick the matching component, project to a Reflect-friendly
            // owned struct so we can drop the AST borrow immediately.
            let Some(info) = crate::ast_extract::extract_components_for_class(class_def)
                .into_iter()
                .find(|c| c.name == instance_name)
            else {
                placeholder(
                    ui,
                    &format!(
                        "Selected node `{instance_name}` not declared in `{short}`."
                    ),
                );
                return;
            };
            (info, class)
        };

        // ── Render header ───────────────────────────────────────
        ui.add_space(4.0);
        ui.heading(&component_info.name);
        ui.label(
            egui::RichText::new(&component_info.type_name)
                .size(11.0)
                .color(egui::Color32::GRAY),
        );
        if !component_info.description.is_empty() {
            ui.label(&component_info.description);
        }
        ui.separator();

        // ── Render modifications + collect edits ────────────────
        //
        // `text_edit_singleline` returns a Response; we apply the
        // edit on `lost_focus()` rather than `changed()` so a partial
        // value mid-typing doesn't fire a SetParameter on every
        // keystroke (which would push N undo entries onto the stack).
        let mut edits: Vec<(String, String)> = Vec::new();
        if component_info.modifications.is_empty() {
            ui.label(
                egui::RichText::new("No modifications declared. Edits will append new ones.")
                    .italics()
                    .color(egui::Color32::GRAY),
            );
        }
        // Stable order: sort by name so the inspector layout doesn't
        // jitter as the underlying HashMap iteration order shifts.
        let mut entries: Vec<(&String, &String)> = component_info.modifications.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        ui.collapsing("⚙ Modifications", |ui| {
            egui::Grid::new("modelica_inspector_mods")
                .num_columns(2)
                .spacing([10.0, 4.0])
                .show(ui, |ui| {
                    for (k, v) in entries {
                        ui.label(k);
                        let mut buf = v.clone();
                        let resp = ui.text_edit_singleline(&mut buf);
                        if resp.lost_focus() && buf != *v {
                            edits.push((k.clone(), buf));
                        }
                        ui.end_row();
                    }
                });
        });

        // ── Apply edits as a single batched event ───────────────
        if !edits.is_empty() {
            let ops: Vec<ApiOp> = edits
                .into_iter()
                .map(|(param, value)| ApiOp::SetParameter {
                    class: class.clone(),
                    component: instance_name.clone(),
                    param,
                    value,
                })
                .collect();
            world.commands().trigger(ApplyModelicaOps {
                doc: doc_id.raw(),
                ops,
            });
        }
    }
}

fn placeholder(ui: &mut egui::Ui, msg: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(20.0);
        ui.label(
            egui::RichText::new(msg)
                .italics()
                .color(egui::Color32::GRAY),
        );
    });
}
