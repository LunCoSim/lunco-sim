//! Document-identity rename command.

use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

/// Rename an open document identified by its workspace document id.
///
/// The document layer owns this payload because it addresses a document even
/// when that document is an untitled draft. Windowed hosts may observe it and
/// route filesystem-backed documents into their workspace rename flow.
#[Command(default)]
pub struct RenameOpenDocument {
    /// The document to rename.
    pub doc_id: lunco_doc::DocumentId,
    /// New filename or class identifier; path separators are not accepted.
    pub new_name: String,
}
