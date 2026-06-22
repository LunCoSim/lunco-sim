//! UI→core bridge: workbench file/tab rename events → `RenameModelicaClass`.
//!
//! This observer reacts to a workbench (UI) event
//! ([`lunco_workbench::file_ops::RenameOpenDocument`]) and chains it into the
//! core [`crate::api::class::RenameModelicaClass`] command. It lives in the
//! `ui` module because the trigger is a UI workflow event; the core API plugin
//! stays free of workbench types. A headless server never fires the event, so
//! it simply isn't registered there.
//!
//! The saved-`.mo`-file path (`RenameTwinEntry` → `FileRenamed` →
//! `on_file_renamed_chain_to_modelica`) chains off the now-core
//! [`lunco_workspace::FileRenamed`] event, so that observer stays in
//! `crate::api::class` (it names no UI types).

use bevy::prelude::*;

use crate::api::class::RenameModelicaClass;

/// Chain observer: workbench [`lunco_workbench::file_ops::RenameOpenDocument`]
/// → [`RenameModelicaClass`] for Untitled Modelica drafts.
///
/// The workbench's own observer routes saved files via
/// `lunco_workbench::file_ops::RenameTwinEntry` (which then chains to the
/// saved-file path). Untitled docs have no on-disk presence, so the rename is
/// purely a class-declaration rewrite — that's what this observer handles.
pub fn on_rename_open_document_chain_to_modelica(
    trigger: On<lunco_workbench::file_ops::RenameOpenDocument>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    registry: Res<crate::state::ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    use lunco_doc::DocumentOrigin;
    let ev = trigger.event();
    let Some(entry) = workspace.document(ev.doc) else {
        return;
    };
    // Only handle Untitled drafts; saved files go through the
    // RenameTwinEntry → FileRenamed → on_file_renamed_chain_to_modelica path.
    let DocumentOrigin::Untitled { name } = &entry.origin else {
        return;
    };
    // Confirm the doc is actually Modelica before firing RenameModelicaClass.
    if registry.host(ev.doc).is_none() {
        return;
    }
    let old_name = name.clone();
    let new_name = ev.new_name.trim().to_string();
    if new_name.is_empty() || new_name == old_name {
        return;
    }
    bevy::log::info!(
        "[RenameOpenDocument→Modelica] Untitled doc={} {} → {}",
        ev.doc,
        old_name,
        new_name
    );
    commands.trigger(RenameModelicaClass {
        doc: ev.doc,
        old_name,
        new_name,
    });
}
