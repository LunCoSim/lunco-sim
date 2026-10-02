//! Compact navigation and contextual actions over the cached diagram facts.
use super::{UsdCanvasSessionState, navigation, view_files};
use bevy_egui::egui;
use lunco_doc::Document;
use lunco_workbench_core::PanelCtx;

pub(super) fn source_picker(ui: &mut egui::Ui, show_scene: &mut bool) {
    egui::ComboBox::from_id_salt("connection_source")
        .selected_text(if *show_scene {
            "Scene · Browse"
        } else {
            "Document · Edit"
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(show_scene, true, "Scene · Browse");
            ui.selectable_value(show_scene, false, "Document · Edit");
        });
}

/// Returns a source switch requested by the user, after finishing the current borrow.
pub(super) fn render(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut UsdCanvasSessionState,
    show_scene: bool,
) -> Option<bool> {
    let mut requested_source = show_scene;
    let mut requested_view = state.selected_view.clone();
    let previous_mode = state.diagram_mode;
    if show_scene {
        state.diagram_mode = true;
    }
    ui.horizontal_wrapped(|ui| {
        source_picker(ui, &mut requested_source);
        if let Some(host) = state.view_document.as_ref() {
            let view_id = host.document().id().raw();
            let dirty = host.document().generation() != state.saved_view_generation;
            let names: Vec<_> = host.document().data().views.keys().cloned().collect();
            ui.menu_button("View", |ui| {
                ui.label("Named layout (* = unsaved)");
                egui::ComboBox::from_id_salt(("connection_named_view", view_id))
                    .selected_text(format!(
                        "{}{}",
                        requested_view,
                        if dirty { " *" } else { "" }
                    ))
                    .show_ui(ui, |ui| {
                        for name in &names {
                            ui.selectable_value(&mut requested_view, name.clone(), name);
                        }
                    });
                if ui
                    .checkbox(&mut state.include_descendants, "Include nested prims")
                    .changed()
                {
                    state.rebuild_view();
                }
                if !show_scene && !state.schema_roots.is_empty() {
                    ui.selectable_value(&mut state.diagram_mode, true, "System diagram");
                    ui.selectable_value(&mut state.diagram_mode, false, "Authored schema");
                }
                ui.separator();
                ui.add(
                    egui::TextEdit::singleline(&mut state.new_view_name).hint_text("New view name"),
                );
                if ui.button("Create view of this system").clicked() {
                    ctx.trigger(view_files::CreateConnectionView {
                        view_id,
                        name: state.new_view_name.clone(),
                        scope: state.diagram_root.clone(),
                    });
                    ui.close();
                }
                ui.horizontal(|ui| {
                    if ui.button("Undo layout").clicked() {
                        ctx.trigger(view_files::UndoConnectionView {
                            view_id,
                            redo: false,
                        });
                    }
                    if ui.button("Redo").clicked() {
                        ctx.trigger(view_files::UndoConnectionView {
                            view_id,
                            redo: true,
                        });
                    }
                });
                ui.separator();
                ui.label("Repository view file");
                ui.add(
                    egui::TextEdit::singleline(&mut state.view_file_path)
                        .hint_text("Path to .lunco-view.toml"),
                );
                ui.horizontal(|ui| {
                    for (label, save) in [("Save views", true), ("Load views", false)] {
                        if ui.button(label).clicked() {
                            ctx.trigger(view_files::ConnectionViewFile {
                                view_id,
                                path: state.view_file_path.clone(),
                                save,
                                scope: state.diagram_root.clone(),
                                include_descendants: state.include_descendants,
                            });
                            ui.close();
                        }
                    }
                });
            });
        }
        if ui
            .button("Fit")
            .on_hover_text("Show this entire system")
            .clicked()
        {
            if let Some(host) = &state.view_document {
                ctx.trigger(navigation::FrameConnectionDiagram {
                    view_id: host.document().id().raw(),
                    key: None,
                });
            }
        }
        if ui
            .button("Arrange")
            .on_hover_text("Apply the Rhai layout policy; saved positions stay pinned")
            .clicked()
        {
            state.rebuild_view();
        }
        if show_scene
            && ui
                .button("Edit connections")
                .on_hover_text("Open the USD document to connect ports or add sources")
                .clicked()
        {
            if let Some(uri) = &state.source_uri {
                ctx.trigger(lunco_usd_core::commands::OpenUsdSourceDocument {
                    source: uri.clone(),
                });
                requested_source = false;
            }
        }
        if !show_scene {
            let dirty =
                state
                    .doc
                    .and_then(|id| {
                        ctx.resource::<lunco_doc_bevy::DocumentRegistry<
                            lunco_usd_document::document::UsdDocument,
                        >>()
                        .and_then(|registry| registry.host(id))
                    })
                    .map(|host| host.document().is_dirty());
            ui.label(if dirty == Some(true) {
                "USD modified"
            } else {
                "Editing USD"
            })
            .on_hover_text(format!(
                "Source: {}\nEdit target: {:?}",
                state.source_uri.as_deref().unwrap_or(""),
                state.edit_target
            ));
        }
        ui.toggle_value(&mut state.details_open, "Inspector");
        ui.menu_button("Help", |ui| {
            ui.strong("Explore");
            ui.label("Double-click a system to see its child prims.");
            ui.label("Double-click a leaf model to open its editor.");
            ui.label("Select a prim, then use the inspector to open an attached source.");
            ui.label("Use Back, breadcrumbs or Find to navigate.");
            ui.label("Select a prim and Focus to read its ports; Fit returns to the overview.");
            ui.separator();
            ui.strong("Arrange and author");
            ui.label("Drag nodes to arrange; scroll to zoom; right-drag to pan.");
            ui.label("Edit connections enables port dragging and source drops.");
            ui.label("Drop USD in empty space; Modelica/Rhai onto a host prim.");
            ui.label("View saves separate layouts without changing USD topology.");
            ui.separator();
            let theme = lunco_theme::active(ui.ctx());
            ui.colored_label(
                theme.schematic.wire_signal,
                "Causal: directional signals with arrows",
            );
            ui.colored_label(
                theme.schematic.wire_unknown,
                "Acausal: hollow diamonds, shared physical connectors",
            );
            ui.colored_label(
                theme.schematic.wire_mechanical,
                "Joint: physical attachment",
            );
            ui.label(
                "Inputs are left; outputs are right. Acausal connectors have no signal direction.",
            );
            ui.label(
                "Hierarchy is not a signal connection: cameras and geometry may have no ports.",
            );
            ui.label("Backend badges identify attached models even when their prims are nested.");
        });
    });
    if requested_source != show_scene {
        return Some(requested_source);
    }
    if requested_view != state.selected_view {
        if let Some(host) = &state.view_document {
            let definition = &host.document().data().views[&requested_view];
            state.include_descendants = definition.include_descendants;
            state.diagram_root = definition.scope.clone();
        }
        if !state.diagram_roots.contains(&state.diagram_root) {
            state.last_error = Some(format!(
                "USD scope {} is unavailable; showing the source root",
                state.diagram_root
            ));
            state.diagram_root = "/".into();
        }
        state.selected_view = requested_view;
        state.rebuild_view();
    }
    if previous_mode != state.diagram_mode {
        state.rebuild_view();
    }
    let view_id = state
        .view_document
        .as_ref()
        .map(|host| host.document().id().raw());
    ui.horizontal_wrapped(|ui| {
        if state.diagram_mode {
            if ui
                .add_enabled(
                    !state.navigation_history.is_empty(),
                    egui::Button::new("Back"),
                )
                .clicked()
            {
                if let Some(view_id) = view_id {
                    ctx.trigger(navigation::NavigateConnectionDiagram {
                        view_id,
                        scope: None,
                        back: true,
                    });
                }
            }
            let segments: Vec<_> = state
                .diagram_root
                .split('/')
                .filter(|part| !part.is_empty())
                .collect();
            let compact = ui.available_width() < ui.spacing().interact_size.x * 25.0;
            ui.menu_button("Scene /", |ui| {
                if ui.button("Stage root").clicked() {
                    if let Some(view_id) = view_id {
                        ctx.trigger(navigation::NavigateConnectionDiagram {
                            view_id,
                            scope: Some("/".into()),
                            back: false,
                        });
                    }
                    ui.close();
                }
                let mut path = String::new();
                for part in &segments {
                    path.push('/');
                    path.push_str(part);
                    if ui.button(&path).clicked() {
                        if let Some(view_id) = view_id {
                            ctx.trigger(navigation::NavigateConnectionDiagram {
                                view_id,
                                scope: Some(path.clone()),
                                back: false,
                            });
                        }
                        ui.close();
                    }
                }
            });
            let mut prefix = String::new();
            for (i, segment) in segments.iter().enumerate() {
                prefix.push('/');
                prefix.push_str(segment);
                if compact && i + 1 < segments.len() {
                    continue;
                }
                if ui
                    .selectable_label(prefix == state.diagram_root, *segment)
                    .on_hover_text(&prefix)
                    .clicked()
                {
                    if let Some(view_id) = view_id {
                        ctx.trigger(navigation::NavigateConnectionDiagram {
                            view_id,
                            scope: Some(prefix.clone()),
                            back: false,
                        });
                    }
                }
                if i + 1 < segments.len() {
                    ui.weak("/");
                }
            }
            ui.menu_button("Find", |ui| super::inspection::find(ui, ctx, state));
        } else {
            let mut root = state.active_schema_root.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt("usd_schema_root")
                .selected_text(&root)
                .show_ui(ui, |ui| {
                    for path in &state.schema_roots {
                        ui.selectable_value(&mut root, path.clone(), path);
                    }
                });
            if !root.is_empty() && state.active_schema_root.as_deref() != Some(root.as_str()) {
                state.active_schema_root = Some(root);
                state.rebuild_view();
            }
        }
        let scope = state
            .source_nodes
            .iter()
            .find(|node| node.path == state.diagram_root);
        let variant_error = scope.and_then(|node| node.variants.as_ref().err());
        let details = if variant_error.is_some() {
            egui::RichText::new("Details !").color(lunco_theme::active(ui.ctx()).tokens.warning)
        } else {
            egui::RichText::new("Details")
        };
        ui.menu_button(details, |ui| {
            ui.label(state.source_uri.as_deref().unwrap_or("USD topology"));
            ui.label(format!(
                "{} prims · {} connections",
                state.canvas.scene.node_count(),
                state.canvas.scene.edge_count()
            ));
            if let Some(scope) = scope {
                match &scope.variants {
                    Ok(variants) if !variants.is_empty() => {
                        ui.separator();
                        ui.strong("Composed USD variants");
                        for (name, selection) in variants {
                            ui.label(format!("{name} = {selection}"));
                        }
                    }
                    Err(error) => {
                        ui.colored_label(lunco_theme::active(ui.ctx()).tokens.warning, error);
                    }
                    _ => {}
                }
            }
        });
    });
    ui.separator();
    None
}
