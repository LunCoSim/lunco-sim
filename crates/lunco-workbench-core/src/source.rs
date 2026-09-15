//! Source-view contracts shared by browsers and the concrete source viewer.

use bevy::ecs::reflect::ReflectEvent;
use bevy::reflect::std_traits::ReflectDefault;
use lunco_core::Command;

/// Open a registered asset as read-only text in the source viewer.
#[Command(default)]
pub struct OpenSourceView {
    /// Registered asset path.
    pub asset_path: String,
}

/// Open an ephemeral generated document in the read-only source viewer.
#[Command(default)]
pub struct OpenEphemeralSource {
    /// URI shown as the document identity.
    pub uri: String,
    /// Complete generated source text.
    pub text: String,
}

/// Open one file belonging to an open Twin in the editable source panel.
#[Command(default)]
pub struct OpenTwinSource {
    /// Absolute root of the already-open Twin.
    pub twin_root: String,
    /// File path relative to that root.
    pub relative_path: String,
    /// Keep the file open when another preview is selected.
    pub pinned: bool,
    /// Whether opening the source should focus its tab.
    #[serde(default)]
    pub focus: Option<bool>,
}

/// Persist an editable source buffer, optionally refreshing its owning domain.
#[Command(default)]
pub struct SaveSourceText {
    /// Absolute root of the already-open Twin.
    pub twin_root: String,
    /// File path relative to that root.
    pub relative_path: String,
    /// Complete UTF-8 source text.
    pub text: String,
    /// Re-dispatch the owning document open operation after writing.
    pub update: bool,
}

/// Return whether a path belongs to the generic source-only text viewer.
pub fn is_source_only_text_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "rhai" | "wgsl"))
}
