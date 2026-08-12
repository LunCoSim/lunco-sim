//! Periodic update/sync systems: StatusBar and UnsavedDocs.

use crate::state::{ModelicaDocumentRegistry, WorkbenchState};
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
    bus: Option<ResMut<lunco_workbench::status_bus::StatusBus>>,
    registry: Res<ModelicaDocumentRegistry>,
    mut last_status: Local<Option<String>>,
) {
    let Some(mut bus) = bus else { return };
    let any_change = workbench.is_changed()
        || compile_states.is_changed()
        || registry.is_changed()
        || workspace.as_ref().map(|w| w.is_changed()).unwrap_or(false);
    let bus_added = bus.is_added();
    if !any_change && !bus_added {
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
    let mut ready_files: Vec<_> = registry
        .iter()
        .filter(|(doc, _)| compile_states.state_of(*doc) == lunco_doc::CompileState::Ready)
        .map(|(_, host)| modelica_file_label(&host.document().origin().session_uri()))
        .collect();
    ready_files.sort_unstable();
    ready_files.dedup();

    let text = match active_doc {
        None => format_ready_files(&ready_files),
        Some(doc) => match compile_states.state_of(doc) {
            lunco_doc::CompileState::Compiling => format!("⏳ Compiling {model_name}…"),
            lunco_doc::CompileState::Error => format!("⚠ Compile error in {model_name}"),
            lunco_doc::CompileState::Ready => format_ready_files(&ready_files),
            lunco_doc::CompileState::Idle => format!("● {model_name}"),
        },
    };
    if !bus_added && last_status.as_deref() == Some(text.as_str()) {
        return;
    }
    *last_status = Some(text.clone());
    bus.push(
        lunco_workbench::status_bus::MODELICA_SOURCE,
        lunco_workbench::status_bus::StatusLevel::Info,
        text,
    );
}

fn modelica_file_label(uri: &str) -> String {
    uri.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .map(str::to_owned)
        .filter(|part| !part.is_empty())
        .unwrap_or_else(|| "(untitled)".to_string())
}

fn format_ready_files(files: &[String]) -> String {
    if files.is_empty() {
        "ready — no compiled Modelica files".to_string()
    } else {
        format!("ready — {}", files.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::{format_ready_files, modelica_file_label, update_status_bar};
    use bevy::prelude::{App, Update};
    use lunco_doc::{CompileState, DocumentOrigin};
    use lunco_doc_bevy::DocumentDiagnostics;
    use lunco_workbench::status_bus::StatusBus;

    use crate::state::{ModelicaDocumentRegistry, WorkbenchState};

    #[test]
    fn file_label_handles_uri_and_windows_separators() {
        assert_eq!(modelica_file_label("lunco://models/lander.mo"), "lander.mo");
        assert_eq!(modelica_file_label(r"C:\\models\\lander.mo"), "lander.mo");
    }

    #[test]
    fn ready_status_lists_files_and_handles_empty_registry() {
        assert_eq!(
            format_ready_files(&["lander.mo".into(), "controller.mo".into()]),
            "ready — lander.mo, controller.mo"
        );
        assert_eq!(
            format_ready_files(&[]),
            "ready — no compiled Modelica files"
        );
    }

    #[test]
    fn status_publishes_ready_files_once_until_the_text_changes() {
        let mut app = App::new();
        app.insert_resource(WorkbenchState::default())
            .insert_resource(ModelicaDocumentRegistry::default())
            .insert_resource(DocumentDiagnostics::default())
            .insert_resource(StatusBus::default())
            .add_systems(Update, update_status_bar);

        app.update();
        let initial_total = app.world().resource::<StatusBus>().history_total();
        assert_eq!(initial_total, 1);

        let doc = app
            .world_mut()
            .resource_mut::<ModelicaDocumentRegistry>()
            .allocate_with_origin(
                "model A end A;".into(),
                DocumentOrigin::bundled("lander.mo"),
            );
        app.world_mut()
            .resource_mut::<DocumentDiagnostics>()
            .set_ok(doc);
        app.update();

        let bus = app.world().resource::<StatusBus>();
        assert_eq!(bus.history_total(), initial_total + 1);
        assert_eq!(
            bus.history().last().map(|event| event.message.as_str()),
            Some("ready — lander.mo")
        );

        app.update();
        assert_eq!(
            app.world().resource::<StatusBus>().history_total(),
            initial_total + 1
        );
        assert_eq!(
            app.world().resource::<DocumentDiagnostics>().state_of(doc),
            CompileState::Ready
        );
    }
}
