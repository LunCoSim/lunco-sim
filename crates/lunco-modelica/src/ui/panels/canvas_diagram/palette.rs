//! MSL package palette tree + render-as-context-menu.
//!
//! Builds a static [`MslPackageNode`] tree from
//! [`crate::visual_diagram::msl_class_library`] and renders it as
//! a nested egui submenu so users can pick MSL components without
//! leaving the canvas. Also houses the user-tunable [`PaletteSettings`]
//! and [`crate::ui::panels::canvas_diagram::DiagramProjectionLimits`] resources.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::PanelCtx;

use crate::document::ModelicaOp;

use super::ops::{op_add_component_with_name, pick_add_instance_name};
use super::CanvasDiagramState;

/// One node in the MSL package hierarchy. `classes` are instantiable
/// at this level (instances we'd add to the diagram), `subpackages`
/// are deeper navigation. `BTreeMap` for stable alphabetical order
/// regardless of the source list's order.
pub(super) struct MslPackageNode {
    subpackages: std::collections::BTreeMap<String, MslPackageNode>,
    /// Classes at this level. Pre-sorted alphabetically by short name
    /// once at tree-build time so `render_msl_package_menu` doesn't
    /// clone-and-sort on every render frame (the menu re-renders
    /// every frame the pointer is over it; per-frame O(n log n)
    /// across nested submenus is the cause of the laggy right-click
    /// context-menu navigation).
    classes: Vec<&'static crate::index::ClassEntry>,
    /// Pre-computed: `true` if this subtree contains at least one
    /// non-icon-only class. Lets the menu skip empty branches in O(1)
    /// instead of recursively walking on every render.
    has_non_icon_class: bool,
}

impl MslPackageNode {
    fn new() -> Self {
        Self {
            subpackages: Default::default(),
            classes: Vec::new(),
            has_non_icon_class: false,
        }
    }
}

/// User-facing toggles for the MSL add-component menu. Default
/// values are tuned for the common case ("a user dropping a
/// component expects a functional block, not an icon shell").
/// Persisted as a Bevy resource; the Settings dropdown flips the
/// `show_icon_only_classes` flag to override.
#[derive(Resource, Debug, Clone)]
pub struct PaletteSettings {
    /// When `true`, pure-icon classes (matched by
    /// [`crate::ui::loaded_classes::is_icon_only_class`]) appear in the
    /// MSL add-component submenus. Default `false` — matches
    /// Dymola's "hide `.Icons.*`" default.
    pub show_icon_only_classes: bool,
}

impl Default for PaletteSettings {
    fn default() -> Self {
        Self {
            show_icon_only_classes: false,
        }
    }
}

/// Soft guards for the canvas projection. Prevent accidental
/// attempts to diagram huge packages without getting in the way of
/// deeply composed real models. Exposed via the Settings dropdown.
#[derive(Resource, Debug, Clone)]
pub struct DiagramProjectionLimits {
    /// Maximum component count the projector will accept before
    /// returning `None`. Default
    /// [`crate::ui::panels::canvas_projection::DEFAULT_MAX_DIAGRAM_NODES`]
    /// (1000). Users building power-system or multi-body models
    /// with hundreds of components can raise this in Settings.
    pub max_nodes: usize,
    /// Wall-clock deadline for a single projection task. If the bg
    /// task hasn't resolved within this window, the poll loop
    /// flips the task's `cancel` flag AND drops the handle. Task
    /// finishes (waste, but bounded), result is discarded, canvas
    /// stays empty with a "projection timed out" overlay.
    ///
    /// Deliberately high (60 s default) — only catches truly
    /// catastrophic work, not normal drill-ins. Raise in Settings
    /// if you're profiling something slow on purpose.
    pub max_duration: std::time::Duration,
}

impl Default for DiagramProjectionLimits {
    fn default() -> Self {
        Self {
            max_nodes: crate::ui::panels::canvas_projection::DEFAULT_MAX_DIAGRAM_NODES,
            max_duration: std::time::Duration::from_secs(60),
        }
    }
}

/// True if the subtree contains any class that would be visible
/// with the icon-only filter OFF (i.e. has a real, non-icon-only
/// class somewhere). Reads the precomputed flag set at tree-build
/// time so the menu can skip empty branches in O(1).
///
/// Was previously recursive — fine for one open-frame, expensive
/// when called on every render for every visible submenu (the
/// right-click menu re-runs every frame the pointer is over it).
pub(super) fn package_has_visible_classes(node: &MslPackageNode) -> bool {
    node.has_non_icon_class
}

/// Lazily-built package tree. Walks every entry in
/// [`crate::visual_diagram::msl_class_library`] once and
/// inserts it under its dotted package path. Cached for the life
/// of the process — MSL content doesn't change at runtime.
pub(super) fn msl_package_tree() -> &'static MslPackageNode {
    use std::sync::OnceLock;
    static TREE: OnceLock<MslPackageNode> = OnceLock::new();
    TREE.get_or_init(|| {
        let mut root = MslPackageNode::new();
        for comp in crate::visual_diagram::msl_class_library() {
            // Split the qualified path into package segments + a
            // trailing class name. `Modelica.Electrical.Analog.
            // Basic.Resistor` → walk subpackages
            // [Modelica, Electrical, Analog, Basic], attach class
            // `Resistor`.
            let mut parts: Vec<&str> = comp.name.split('.').collect();
            let Some(_class_name) = parts.pop() else {
                continue;
            };
            let mut node = &mut root;
            for seg in parts {
                node = node
                    .subpackages
                    .entry(seg.to_string())
                    .or_insert_with(MslPackageNode::new);
            }
            node.classes.push(comp);
        }
        // Post-pass: sort classes alphabetically by short name and
        // precompute the `has_non_icon_class` rollup. Done once here
        // so the right-click menu's recursive renderer is purely
        // O(visible-items) per frame instead of repeatedly cloning,
        // sorting, and walking subtrees.
        finalize_tree(&mut root);
        root
    })
}

pub(super) fn finalize_tree(node: &mut MslPackageNode) {
    node.classes.sort_by(|a, b| a.name.cmp(&b.name));
    let mut any_visible = node
        .classes
        .iter()
        .any(|c| !crate::ui::loaded_classes::is_icon_only_class(&c.name));
    for child in node.subpackages.values_mut() {
        finalize_tree(child);
        any_visible = any_visible || child.has_non_icon_class;
    }
    node.has_non_icon_class = any_visible;
}

/// Recursively render a package node as egui submenus.
///
/// Ordering per level: subpackages first (alphabetical via
/// `BTreeMap`), then a thin separator, then classes at this
/// level (own-package classes). Matches how OMEdit's library
/// browser reads: packages above, classes below.
///
/// On click of a class item we emit `AddComponent` through `out`
/// exactly as the flat menu did.
pub(super) fn render_msl_package_menu(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    state: &mut CanvasDiagramState,
    doc_id: Option<lunco_doc::DocumentId>,
    node: &MslPackageNode,
    click_world: lunco_canvas::Pos,
    editing_class: Option<&str>,
    show_icons: bool,
    out: &mut Vec<ModelicaOp>,
) {
    for (name, child) in &node.subpackages {
        // Skip subtrees that would be entirely empty after the
        // icon-only filter. Cheap recursive walk; avoids showing
        // dead-end submenus the user can click into only to find
        // nothing.
        if !show_icons && !package_has_visible_classes(child) {
            continue;
        }
        ui.menu_button(name, |ui| {
            render_msl_package_menu(
                ui,
                ctx,
                state,
                doc_id,
                child,
                click_world,
                editing_class,
                show_icons,
                out,
            );
        });
    }
    if !node.subpackages.is_empty() && !node.classes.is_empty() {
        ui.separator();
    }
    // Classes are pre-sorted at tree-build time (see `finalize_tree`).
    // Iterating directly avoids a clone + sort on every render frame.
    for comp in &node.classes {
        let comp = *comp;
        // Hide icon-only classes unless the user explicitly enabled
        // them in Settings. Path-based detection via `is_icon_only_class`
        // (currently `.Icons.` subpackage check).
        if !show_icons && crate::ui::loaded_classes::is_icon_only_class(&comp.name) {
            continue;
        }
        // Display: icon character (if any) + short name. The
        // icon character gives a quick visual cue without
        // loading the SVG.
        let label = if let Some(ic) = comp.icon_text.as_deref() {
            if !ic.is_empty() {
                format!("{ic}  {}", comp.name)
            } else {
                comp.name.clone()
            }
        } else {
            comp.name.clone()
        };
        if ui
            .button(label)
            .on_hover_text(if comp.description.is_empty() {
                comp.name.clone()
            } else {
                comp.description.clone()
            })
            .clicked()
        {
            if let Some(class) = editing_class {
                let instance_name = {
                    // B.1: route through the per-render tab when one
                    // is in scope. Outside render (no TabRenderContext)
                    // falls back to the first-tab path — unchanged
                    // behaviour for non-render callers.
                    let tab = ctx
                        .resource::<crate::model_tabs_types::TabRenderContext>()
                        .and_then(|c| c.tab_id);
                    pick_add_instance_name(comp, &state.get_for_render(tab, doc_id).canvas.scene)
                };
                // Optimistic scene synthesis (`synthesize_msl_node`) was
                // removed. Now: emit the op, gen bumps in
                // `apply_patch`, the next frame's projection re-derives
                // the scene from the new AST. Same-frame visual
                // response since the projection system runs each tick.
                out.push(op_add_component_with_name(
                    comp,
                    &instance_name,
                    click_world,
                    class,
                ));
            }
            ui.close();
        }
    }
}
