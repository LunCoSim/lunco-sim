//! Inspection commands for Modelica documents.

use crate::ui::document_context::ModelicaDocuments;
use bevy::prelude::*;
use lunco_core::{Command, on_command};

/// Dump the active document's registry state to the log — id, source length,
/// parse status, linked entities. A debugging verb for "what does the app
/// actually think is open?", taking no parameters because it always targets
/// whatever the user is looking at.
#[Command(default)]
pub struct InspectActiveDoc {}

#[on_command(InspectActiveDoc)]
pub fn on_inspect_active_doc(_trigger: On<InspectActiveDoc>, mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let doc = super::resolve_active_doc(world);
        let Some(doc) = doc else {
            bevy::log::warn!("[InspectActiveDoc] no active document");
            return;
        };
        let registry = world.resource::<ModelicaDocuments>();
        let Some(host) = registry.host(doc) else {
            bevy::log::warn!("[InspectActiveDoc] doc {} not in registry", doc.raw());
            return;
        };
        let document = host.document();
        let cache = document.ast();
        let origin = document.origin();
        bevy::log::info!(
            "[InspectActiveDoc] doc={} origin={:?} source_len={} gen={}",
            doc.raw(),
            origin.display_name(),
            document.source().len(),
            cache.generation,
        );
        if cache.has_errors() {
            for e in &cache.errors {
                bevy::log::warn!("[InspectActiveDoc]   parse ERR: {}", e.message);
            }
        } else if let Some(ast) = document.strict_ast() {
            bevy::log::info!(
                "[InspectActiveDoc]   parse OK; within={:?}",
                ast.within.as_ref().map(|w| w.to_string()),
            );
            fn dump(name: &str, class: &rumoca_compile::parsing::ast::ClassDef, depth: usize) {
                let indent = "  ".repeat(depth + 1);
                let comps: Vec<String> = class
                    .components
                    .iter()
                    .map(|(n, c)| format!("{}: {}", n, c.type_name))
                    .collect();
                bevy::log::info!(
                    "[InspectActiveDoc]{}{} ({:?}) extends={} components=[{}]",
                    indent,
                    name,
                    class.class_type,
                    class.extends.len(),
                    comps.join(", "),
                );
                for (cn, child) in &class.classes {
                    dump(cn, child, depth + 1);
                }
            }
            for (n, c) in &ast.classes {
                dump(n, c, 0);
            }
        } else {
            bevy::log::warn!(
                "[InspectActiveDoc]   parse cache empty — likely worker parse pending"
            );
        }
    });
}

/// Project the selected component identity from the document's visible diagram tab.
pub(crate) struct InspectDiagramSelection;

impl lunco_api::ApiQueryProvider for InspectDiagramSelection {
    fn name(&self) -> &'static str {
        "InspectDiagramSelection"
    }

    fn execute(&self, world: &World, params: &lunco_hooks::HookValue) -> lunco_api::ApiQueryResult {
        use lunco_hooks::HookValue;
        let raw = lunco_api::queries::api_param_u64(params, "doc_id")
            .filter(|id| *id != 0)
            .ok_or_else(|| {
                lunco_api::ApiQueryError::new(
                    lunco_api_core::ApiErrorCode::DeserializationError,
                    "doc_id must identify a Modelica document",
                )
            })?;
        let doc = lunco_doc::DocumentId::new(raw);
        let state = world.get_resource::<crate::ui::panels::canvas_diagram::CanvasDiagramState>();
        let view = super::nav::visible_tab_for_doc(world, doc)
            .zip(state)
            .map(|(tab, state)| state.get_for_tab(tab));
        let primary = view.and_then(|view| match view.canvas.selection.primary() {
            Some(lunco_canvas::SelectItem::Node(id)) => {
                view.canvas.scene.node(id)?.origin.as_deref()
            }
            _ => None,
        });
        Ok(Some(HookValue::map([
            ("doc_id", HookValue::UInt(raw)),
            (
                "class",
                view.and_then(|view| view.last_seen_target.as_deref())
                    .map(HookValue::str)
                    .unwrap_or_default(),
            ),
            ("component", primary.map(HookValue::str).unwrap_or_default()),
        ])))
    }
}
