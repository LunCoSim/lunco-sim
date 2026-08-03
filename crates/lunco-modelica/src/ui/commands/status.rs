//! Periodic update/sync systems: status-bus and UnsavedDocs.

use crate::state::ModelicaDocumentRegistry;
use bevy::prelude::*;
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
    workspace: Option<Res<lunco_workspace::WorkspaceResource>>,
    compile_states: Res<DocumentDiagnostics>,
    registry: Res<ModelicaDocumentRegistry>,
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
    mut last: Local<Option<String>>,
    mut busy: Local<Option<lunco_workbench::status_bus::BusyHandle>>,
) {
    let Some(mut bus) = bus else { return };
    let any_change = compile_states.is_changed()
        || registry.is_changed()
        || workspace.as_ref().map(|w| w.is_changed()).unwrap_or(false);
    if !any_change && last.is_some() {
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

    let (text, level, compiling) = match active_doc {
        None => (
            "ready".to_string(),
            lunco_workbench::status_bus::StatusLevel::Info,
            false,
        ),
        Some(doc) => match compile_states.state_of(doc) {
            lunco_doc::CompileState::Compiling => (
                format!("⏳ Compiling {model_name}…"),
                lunco_workbench::status_bus::StatusLevel::Progress,
                true,
            ),
            lunco_doc::CompileState::Error => (
                format!("⚠ Compile error in {model_name}"),
                lunco_workbench::status_bus::StatusLevel::Error,
                false,
            ),
            lunco_doc::CompileState::Ready => (
                format!("✓ Compiled {model_name}"),
                lunco_workbench::status_bus::StatusLevel::Info,
                false,
            ),
            lunco_doc::CompileState::Idle => (
                format!("● {model_name}"),
                lunco_workbench::status_bus::StatusLevel::Info,
                false,
            ),
        },
    };
    if last.as_deref() == Some(text.as_str()) {
        return;
    }
    *last = Some(text.clone());

    const SOURCE: &str = lunco_workbench::status_bus::MODELICA_EDITOR_SOURCE;
    if compiling {
        let handle = busy.get_or_insert_with(|| {
            bus.begin(
                lunco_workbench::status_bus::BusyScope::Global,
                SOURCE,
                text.clone(),
            )
        });
        bus.with_label(handle, text);
        bus.with_progress(handle, 0, 0);
    } else {
        if let Some(mut handle) = busy.take() {
            if level == lunco_workbench::status_bus::StatusLevel::Error {
                handle.set_outcome(lunco_workbench::status_bus::BusyOutcome::Failed(
                    text.clone(),
                ));
            }
            drop(handle);
        }
        bus.push(SOURCE, level, text);
    }
}
