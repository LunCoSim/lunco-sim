//! UI→core bridge: workbench file/tab rename events → `RenameModelicaClass`.
//!
//! This observer reacts to a workbench (UI) event
//! ([`lunco_doc_bevy::rename::RenameOpenDocument`]) and chains it into the
//! [`lunco_modelica_api::edit::class::RenameModelicaClass`] command. It lives
//! in the `ui` module because the trigger is a UI workflow event; the API edit
//! plugin stays free of workbench types. A headless server never fires the
//! event, so it simply isn't registered there.
//!
//! The saved-`.mo`-file path (`RenameTwinEntry` → `FileRenamed` →
//! `on_file_renamed_chain_to_modelica`) chains off the now-core
//! [`lunco_workspace::FileRenamed`] event, so that observer stays in
//! `lunco_modelica_api::edit::class` (it names no UI types).

use bevy::prelude::*;

use lunco_modelica_api::edit::class::RenameModelicaClass;

/// Chain observer: document [`lunco_doc_bevy::rename::RenameOpenDocument`]
/// → [`RenameModelicaClass`] for Untitled Modelica drafts.
///
/// The workbench's own observer routes saved files via
/// `lunco_workspace::rename::RenameTwinEntry` (which then chains to the
/// saved-file path). Untitled docs have no on-disk presence, so the rename is
/// purely a class-declaration rewrite — that's what this observer handles.
pub fn on_rename_open_document_chain_to_modelica(
    trigger: On<lunco_doc_bevy::rename::RenameOpenDocument>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    registry: Res<crate::ui::document_context::ModelicaDocuments>,
    mut commands: Commands,
) {
    use lunco_doc::DocumentOrigin;
    let ev = trigger.event();
    let Some(entry) = workspace.document(ev.doc_id) else {
        return;
    };
    // Only handle Untitled drafts; saved files go through the
    // RenameTwinEntry → FileRenamed → on_file_renamed_chain_to_modelica path.
    let DocumentOrigin::Untitled { name } = &entry.origin else {
        return;
    };
    // Confirm the doc is actually Modelica before firing RenameModelicaClass.
    if registry.host(ev.doc_id).is_none() {
        return;
    }
    let old_name = name.clone();
    let new_name = ev.new_name.trim().to_string();
    if new_name.is_empty() || new_name == old_name {
        return;
    }
    bevy::log::info!(
        "[RenameOpenDocument→Modelica] Untitled doc={} {} → {}",
        ev.doc_id,
        old_name,
        new_name
    );
    commands.trigger(RenameModelicaClass {
        doc_id: ev.doc_id,
        old_name,
        new_name,
    });
}
