//! Inspection commands for Modelica documents.

use bevy::prelude::*;
use lunco_core::{Command, on_command};
use crate::ui::ModelicaDocumentRegistry;

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
        let registry = world.resource::<ModelicaDocumentRegistry>();
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
            fn dump(
                name: &str,
                class: &rumoca_compile::parsing::ast::ClassDef,
                depth: usize,
            ) {
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
