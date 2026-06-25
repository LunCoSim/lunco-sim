//! API handlers for document-level operations.

use bevy::prelude::*;
use lunco_core::{Command, on_command};
use lunco_doc::DocumentId;
use crate::document::ModelicaOp;
use crate::state::ModelicaDocumentRegistry;
use super::util::resolve_doc;

/// Replace an open document's entire source text.
#[Command(default)]
pub struct SetDocumentSource {
    pub doc: DocumentId,
    pub source: String,
}

#[on_command(SetDocumentSource)]
pub fn on_set_document_source(
    trigger: On<SetDocumentSource>,
    mut commands: Commands,
) {
    let doc_raw = trigger.event().doc;
    let source = trigger.event().source.clone();
    commands.queue(move |world: &mut World| {
        let Some(doc) = resolve_doc(world, doc_raw) else {
            bevy::log::warn!("[SetDocumentSource] no doc for id {}", doc_raw);
            return;
        };
        let unchanged = world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc))
            .map(|h| h.document().source() == source)
            .unwrap_or(false);
        if unchanged {
            return;
        }
        match crate::doc_ops::apply_one_op_as(
            world,
            doc,
            ModelicaOp::ReplaceSource { new: source },
            lunco_twin_journal::AuthorTag::for_tool("api"),
        ) {
            Ok(_) => bevy::log::info!("[SetDocumentSource] doc={} replaced", doc.raw()),
            Err(e) => bevy::log::warn!(
                "[SetDocumentSource] doc={} failed: {:?}",
                doc.raw(),
                e
            ),
        }
    });
}
