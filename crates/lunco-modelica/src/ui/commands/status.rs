//! Periodic update/sync systems: StatusBar and UnsavedDocs.

use bevy::prelude::*;
use crate::state::{ModelicaDocumentRegistry, WorkbenchState};
use lunco_doc_bevy::DocumentDiagnostics;

pub fn publish_unsaved_modelica_docs(
    registry: Res<ModelicaDocumentRegistry>,
    unsaved: Option<ResMut<lunco_workbench::UnsavedDocs>>,
) {
    let Some(mut unsaved) = unsaved else { return };
    if !registry.is_changed() && !unsaved.is_added() {
        return;
    }
    unsaved.entries = registry
        .iter()
        .filter(|(_, host)| {
            let o = host.document().origin();
            o.is_writable() || o.is_untitled()
        })
        .map(|(id, host)| {
            let document = host.document();
            let origin = document.origin();
            // `is_unsaved` covers both flavours of "Save before close
            // would lose data": Untitled drafts (never saved) AND
            // dirty saved files (edited since last save). The
            // app-close prompt and the Files-section dirty dot both
            // read this flag — keeping the semantics one place.
            let is_unsaved = origin.is_untitled() || document.is_dirty();
            lunco_workbench::UnsavedDocEntry {
                id,
                display_name: origin.display_name(),
                kind: "Modelica".into(),
                is_unsaved,
            }
        })
        .collect();
}

pub fn update_status_bar(
    workbench: Res<WorkbenchState>,
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    compile_states: Res<DocumentDiagnostics>,
    layout: Option<ResMut<lunco_workbench::WorkbenchLayout>>,
    registry: Res<ModelicaDocumentRegistry>,
) {
    let Some(mut layout) = layout else { return };
    let any_change = workbench.is_changed()
        || compile_states.is_changed()
        || workspace.as_ref().map(|w| w.is_changed()).unwrap_or(false);
    if !any_change && !layout.is_added() {
        return;
    }
    let active_doc = workspace.as_ref().and_then(|w| w.active_document);
    let model_name = active_doc
        .and_then(|d| {
            registry.host(d).and_then(|h| {
                let document = h.document();
                document
                    .strict_ast()
                    .and_then(|ast| crate::ast_extract::extract_model_name_from_ast(&ast))
                    .or_else(|| Some(document.origin().display_name()))
            })
        })
        .unwrap_or_else(|| "(untitled)".to_string());

    let text = match active_doc {
        None => "ready".to_string(),
        Some(doc) => match compile_states.state_of(doc) {
            lunco_doc::CompileState::Compiling => format!("⏳ Compiling {model_name}…"),
            lunco_doc::CompileState::Error => format!("⚠ Compile error in {model_name}"),
            lunco_doc::CompileState::Ready => format!("✓ Compiled {model_name}"),
            lunco_doc::CompileState::Idle => format!("● {model_name}"),
        },
    };
    layout.set_status(text);
}
