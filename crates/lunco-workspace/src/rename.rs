//! Twin-entry rename command.

use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

/// Rename a file or folder inside an open Twin.
///
/// The workspace owns this payload because it identifies an entry by its Twin
/// root and relative path, independently of any window, dock, or renderer.
#[Command(default)]
pub struct RenameTwinEntry {
    /// Absolute path of the Twin root containing the entry.
    pub twin_root: String,
    /// Path of the entry relative to the Twin root.
    pub relative_path: String,
    /// New filename; path separators are not accepted.
    pub new_name: String,
}
