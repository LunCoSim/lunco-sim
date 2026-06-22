//! API handlers for class-level operations (Rename, etc).

use bevy::prelude::*;
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use crate::document::ModelicaOp;
use crate::state::ModelicaDocumentRegistry;
use super::util::resolve_doc;

/// Rename a top-level class within an open Modelica document.
#[Command(default)]
pub struct RenameModelicaClass {
    pub doc: DocumentId,
    pub old_name: String,
    pub new_name: String,
}

#[on_command(RenameModelicaClass)]
pub fn on_rename_modelica_class(
    trigger: On<RenameModelicaClass>,
    mut commands: Commands,
) {
    let ev = trigger.event().clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, ev.doc) else {
            bevy::log::warn!("[RenameModelicaClass] no doc for id {}", ev.doc);
            return;
        };
        if ev.old_name.is_empty() || ev.new_name.is_empty() {
            bevy::log::warn!("[RenameModelicaClass] old/new must be non-empty");
            return;
        }
        if !ev.new_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            bevy::log::warn!(
                "[RenameModelicaClass] new_name `{}` must be a valid identifier",
                ev.new_name
            );
            return;
        }

        let registry = world.resource::<ModelicaDocumentRegistry>();
        let Some(host) = registry.host(doc) else {
            return;
        };
        let source = host.document().source().to_string();
        let new_source = match rewrite_class_name(&source, &ev.old_name, &ev.new_name) {
            Some(s) => s,
            None => {
                bevy::log::warn!(
                    "[RenameModelicaClass] no `<keyword> {}` declaration found in doc {}",
                    ev.old_name,
                    doc.raw()
                );
                return;
            }
        };

        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::ReplaceSource { new: new_source },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => {}
            Err(e) => {
                bevy::log::warn!(
                    "[RenameModelicaClass] doc={} apply failed: {:?}",
                    doc.raw(),
                    e
                );
                return;
            }
        }

        if let Some(mut registry) = world.get_resource_mut::<ModelicaDocumentRegistry>() {
            if let Some(host) = registry.host_mut(doc) {
                let doc_obj = host.document_mut();
                if doc_obj.origin().is_untitled() {
                    doc_obj.set_origin(lunco_doc::DocumentOrigin::untitled(ev.new_name.clone()));
                }
            }
        }
        bevy::log::info!(
            "[RenameModelicaClass] doc={} {} → {}",
            doc.raw(),
            ev.old_name,
            ev.new_name
        );
    });
}

fn rewrite_class_name(source: &str, old: &str, new: &str) -> Option<String> {
    const KEYWORDS: &[&str] = &[
        "model", "class", "package", "connector", "record", "block", "type", "function",
    ];
    let bytes = source.as_bytes();
    let mut decl_pos = None;
    let mut decl_len = 0;
    let mut decl_kw = "";
    'outer: for (i, _) in source.char_indices() {
        for kw in KEYWORDS {
            let pat_len = kw.len() + 1 + old.len();
            if i + pat_len > source.len() { continue; }
            if !source[i..].starts_with(kw) { continue; }
            if bytes[i + kw.len()] != b' ' { continue; }
            if !source[i + kw.len() + 1..].starts_with(old) { continue; }
            let before_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
            let after = i + pat_len;
            let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
            if before_ok && after_ok {
                decl_pos = Some(i);
                decl_len = pat_len;
                decl_kw = kw;
                break 'outer;
            }
        }
    }
    let pos = decl_pos?;
    let mut out = String::with_capacity(source.len() + new.len());
    out.push_str(&source[..pos]);
    out.push_str(&format!("{decl_kw} {new}"));
    out.push_str(&source[pos + decl_len..]);
    let end_pat = format!("end {old};");
    let new_end = format!("end {new};");
    Some(out.replacen(&end_pat, &new_end, 1))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// `on_rename_open_document_chain_to_modelica` (Untitled-draft rename via the
// workbench `RenameOpenDocument` UI event) moved to `crate::ui::rename_chain`
// — it names a workbench type, so it can't live in the core API plugin.

/// Chain observer: when the workbench fires [`FileRenamed`] after a
/// successful on-disk rename, and the renamed entry is a `.mo` file
/// whose stem also changed, rename the file's top-level class
/// declaration so the Modelica convention `Foo.mo` ⇔ `class Foo` stays
/// intact.
///
/// Skips silently when:
/// - the entry is a directory (no class inside),
/// - either path lacks the `.mo` extension (kind mismatch),
/// - the stem didn't actually change (only extension/case),
/// - no open document corresponds to the renamed file (nothing for
///   [`RenameModelicaClass`] to act on — the on-disk file still has
///   the old class name; a follow-up open + manual rename can fix it).
///
/// Cross-file reference rewrites (`import`, `extends`, qualified type
/// refs in other docs) are deliberately not addressed here — that's a
/// separate slice with its own AST-resolver design.
pub fn on_file_renamed_chain_to_modelica(
    trigger: On<lunco_workspace::FileRenamed>,
    workspace: Res<lunco_workspace::WorkspaceResource>,
    mut registry: ResMut<crate::state::ModelicaDocumentRegistry>,
    mut commands: Commands,
) {
    use lunco_doc::DocumentOrigin;
    let ev = trigger.event();
    bevy::log::info!(
        "[FileRenamed→Modelica] fired old={} new={} is_dir={}",
        ev.old_abs.display(),
        ev.new_abs.display(),
        ev.is_dir
    );
    // Patch every Modelica doc whose origin lay under the renamed
    // path. The workbench observer already patched `Workspace.documents`
    // but the per-domain registry (where `SaveDocument` reads the
    // path) is separate — without this, Save would write to the
    // stale pre-rename path, resurrecting the old file on disk and
    // leaving the renamed file with stale content.
    let doc_ids: Vec<lunco_doc::DocumentId> = registry
        .iter()
        .filter_map(|(id, host)| match host.document().origin() {
            DocumentOrigin::File { path, .. } if path.starts_with(&ev.old_abs) => {
                Some(id)
            }
            _ => None,
        })
        .collect();
    for id in doc_ids {
        if let Some(host) = registry.host_mut(id) {
            let doc = host.document_mut();
            let (new_path, writable) = match doc.origin() {
                DocumentOrigin::File { path, writable } => {
                    let suffix = path
                        .strip_prefix(&ev.old_abs)
                        .expect("starts_with implies strip_prefix");
                    let new = if suffix.as_os_str().is_empty() {
                        ev.new_abs.clone()
                    } else {
                        ev.new_abs.join(suffix)
                    };
                    (new, *writable)
                }
                _ => continue,
            };
            doc.set_origin(DocumentOrigin::File {
                path: new_path,
                writable,
            });
        }
    }
    if ev.is_dir {
        return;
    }
    let old_ext = ev
        .old_abs
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    let new_ext = ev
        .new_abs
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase());
    if old_ext.as_deref() != Some("mo") || new_ext.as_deref() != Some("mo") {
        return;
    }
    let old_stem = match ev.old_abs.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s.to_string(),
        None => return,
    };
    let new_stem = match ev.new_abs.file_stem().and_then(|s| s.to_str()) {
        Some(s) => s.to_string(),
        None => return,
    };
    if old_stem == new_stem {
        return;
    }
    // Look up the open Document by its (already post-rename) origin
    // path. The workbench observer rewrote `DocumentEntry.origin`
    // before firing this event, so we match against `new_abs`.
    let doc_id = workspace.documents().iter().find_map(|d| {
        if let DocumentOrigin::File { path, .. } = &d.origin {
            if path == &ev.new_abs {
                return Some(d.id);
            }
        }
        None
    });
    let Some(doc_id) = doc_id else {
        bevy::log::info!(
            "[FileRenamed→Modelica] no live document for new path {} \
             (open docs: {})",
            ev.new_abs.display(),
            workspace
                .documents()
                .iter()
                .filter_map(|d| match &d.origin {
                    DocumentOrigin::File { path, .. } => {
                        Some(path.display().to_string())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(", ")
        );
        return;
    };
    bevy::log::info!(
        "[FileRenamed→Modelica] chaining RenameModelicaClass + SaveDocument \
         doc={} {} → {}",
        doc_id.raw(),
        old_stem,
        new_stem
    );
    commands.trigger(RenameModelicaClass {
        doc: doc_id,
        old_name: old_stem,
        new_name: new_stem,
    });
    // Persist immediately so the on-disk file's class declaration
    // matches the new filename atomically with the rename — VS Code
    // style "rename = single user-visible operation". Without this,
    // the doc would carry the renamed class only in-memory; closing
    // without save would drop the rename.
    commands.trigger(lunco_doc_bevy::SaveDocument { doc: doc_id });
}
