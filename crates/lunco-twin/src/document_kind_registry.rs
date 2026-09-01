//! Plugin-driven registry of document kinds.
//!
//! Provides an open registry: each domain crate (`lunco-modelica`, future
//! `lunco-julia`, `lunco-usd`, …) registers its own kind on plugin
//! `build()`, and consumers iterate the registry rather than matching
//! a fixed enum.
//!
//! Why: AGENTS.md mandates "Hotswappable Plugins — everything must be
//! a plugin." A closed enum forces every new domain to edit
//! `lunco-twin`, which violates the four-layer plugin architecture
//! (Layer 2 domain crates aren't supposed to round-trip through
//! foundation-layer edits to ship). Mirrors the same plugin-driven
//! pattern as [`UriRegistry`](../../lunco_workbench/uri/struct.UriRegistry.html)
//! and the [`BackendRegistry`](https://docs.rs/lunco-cosim) for cosim.
//!
use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use smol_str::SmolStr;

/// Opaque identifier for a document kind.
///
/// Domain crates pick a stable string at registration (`"modelica"`,
/// `"julia"`, `"usd"`). Lower-case ASCII by convention; not enforced.
/// Cheap to clone (`SmolStr` inlines short strings).
#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentKindId(SmolStr);

impl DocumentKindId {
    /// Construct from any string-ish input.
    pub fn new(id: impl Into<SmolStr>) -> Self {
        Self(id.into())
    }

    /// Borrowed access to the underlying id.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for DocumentKindId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.as_str())
    }
}

/// Metadata describing a registered document kind.
///
/// Domain crates fill this in when they register; downstream code
/// (browser sections, File→New menu, file picker classifier, twin.toml
/// parser dispatch) reads from it.
#[derive(Clone, Debug, Default)]
pub struct DocumentKindMeta {
    /// Human-readable name shown in menus and file dialogs
    /// ("Modelica Model", "Julia Script").
    pub display_name: String,

    /// Lower-case file extensions claimed by this kind, *without* the
    /// leading dot. Used by the picker filter and by
    /// [`DocumentKindRegistry::classify`]. The first extension is the
    /// canonical one used when creating a new document.
    pub extensions: Vec<&'static str>,

    /// Whether the File → New menu should expose this kind. False for
    /// kinds that exist only because of files dropped on the app
    /// (Data files, Mission archives).
    pub can_create_new: bool,

    /// Suggested filename when creating a new document of this kind
    /// (e.g. `"NewModel.mo"`). When `None`, the UI falls back to
    /// `"Untitled.<first_extension>"`.
    pub default_filename: Option<&'static str>,

    /// URI scheme this kind contributes to
    /// [`UriRegistry`](../../lunco_workbench/uri/struct.UriRegistry.html)
    /// (e.g. `"modelica"` for `modelica://Modelica.Blocks.Examples.PID`).
    /// Informational here — the actual handler is registered separately
    /// by the same domain plugin.
    pub uri_scheme: Option<&'static str>,

    /// Section name in `twin.toml` (`"modelica"`, `"julia"`, `"usd"`)
    /// that this kind owns. The Twin manifest parser dispatches the
    /// raw TOML table to whichever crate registers this section.
    /// `None` for kinds that don't carry per-Twin manifest config.
    pub manifest_section: Option<&'static str>,
}

/// Registry of document kinds.
///
/// Indexed both by id (for direct lookups) and by extension (for
/// path classification). Domain plugins call [`Self::register`] in
/// `build()`; consumers query via [`Self::classify`], [`Self::meta`],
/// and [`Self::iter`].
///
/// Re-registering the same id replaces the metadata — last-write-wins,
/// matching the convention of the URI and Backend registries. Domain
/// crates are expected to register exactly once per app, so this only
/// matters for tests overriding a stub.
#[cfg_attr(feature = "bevy", derive(bevy::prelude::Resource))]
#[derive(Default, Debug)]
pub struct DocumentKindRegistry {
    kinds: HashMap<DocumentKindId, DocumentKindMeta>,
    /// `lower_case_ext_without_dot` → registered kind id.
    by_extension: HashMap<SmolStr, DocumentKindId>,
}

impl DocumentKindRegistry {
    /// Empty registry. Use [`Self::register`] to add kinds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a kind and its metadata.
    ///
    /// All extensions in `meta.extensions` are added to the
    /// extension-to-id index. If a different kind previously claimed
    /// one of these extensions, the new registration takes over —
    /// matching how downstream tools resolve precedence (last loaded
    /// plugin wins). Apps that want strict semantics can check
    /// `lookup_by_extension` first.
    pub fn register(&mut self, id: DocumentKindId, meta: DocumentKindMeta) {
        for ext in &meta.extensions {
            self.by_extension
                .insert(SmolStr::new(ext.to_ascii_lowercase()), id.clone());
        }
        self.kinds.insert(id, meta);
    }

    /// Classify a path by its extension. Returns `None` for unknown
    /// extensions, paths without an extension, or non-UTF-8 extensions.
    ///
    /// Distinct from [`FileKind::classify`](crate::FileKind::classify) —
    /// that returns the `Document` / `FileReference` / `Unknown`
    /// architectural categorisation. This returns the registered
    /// [`DocumentKindId`] when the file is a known editable document.
    pub fn classify(&self, path: &Path) -> Option<DocumentKindId> {
        let ext = path.extension()?.to_str()?;
        self.lookup_by_extension(ext)
    }

    /// Direct extension lookup. `ext` is matched case-insensitively
    /// without the leading dot.
    pub fn lookup_by_extension(&self, ext: &str) -> Option<DocumentKindId> {
        let key = SmolStr::new(ext.to_ascii_lowercase());
        self.by_extension.get(&key).cloned()
    }

    /// Metadata for a registered id, or `None` if unknown.
    pub fn meta(&self, id: &DocumentKindId) -> Option<&DocumentKindMeta> {
        self.kinds.get(id)
    }

    /// Whether `id` is registered.
    pub fn contains(&self, id: &DocumentKindId) -> bool {
        self.kinds.contains_key(id)
    }

    /// Iterate every registered kind. Order is unspecified; UI code
    /// that needs deterministic order should collect + sort.
    pub fn iter(&self) -> impl Iterator<Item = (&DocumentKindId, &DocumentKindMeta)> {
        self.kinds.iter()
    }

    /// Return creatable kinds in the order shared by File → New and the
    /// default NewDocument command.
    pub fn creatable(&self) -> Vec<(&DocumentKindId, &DocumentKindMeta)> {
        let mut kinds: Vec<_> = self
            .iter()
            .filter(|(_, meta)| meta.can_create_new)
            .collect();
        kinds.sort_by(|(left_id, left), (right_id, right)| {
            left.display_name
                .cmp(&right.display_name)
                .then_with(|| left_id.as_str().cmp(right_id.as_str()))
        });
        kinds
    }

    /// Number of registered kinds.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Bevy plugin
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(feature = "bevy")]
mod plugin {
    use bevy::prelude::*;

    use super::DocumentKindRegistry;

    /// Plugin that initialises the [`DocumentKindRegistry`] as a Bevy
    /// resource. Domain plugins observe this is added (e.g. by adding
    /// it themselves via `app.init_resource::<DocumentKindRegistry>()`,
    /// which is idempotent) and then register their own kinds.
    ///
    /// `WorkbenchPlugin` auto-installs this so apps get it for free.
    /// Headless tests and tools that want the registry without the
    /// full workbench can add this plugin directly.
    pub struct DocumentKindRegistryPlugin;

    impl Plugin for DocumentKindRegistryPlugin {
        fn build(&self, app: &mut App) {
            app.init_resource::<DocumentKindRegistry>();
        }
    }
}

#[cfg(feature = "bevy")]
pub use plugin::DocumentKindRegistryPlugin;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn registry_with_modelica() -> DocumentKindRegistry {
        let mut r = DocumentKindRegistry::new();
        r.register(
            DocumentKindId::new("modelica"),
            DocumentKindMeta {
                display_name: "Modelica Model".into(),
                extensions: vec!["mo"],
                can_create_new: true,
                default_filename: Some("NewModel.mo"),
                uri_scheme: Some("modelica"),
                manifest_section: Some("modelica"),
            },
        );
        r
    }

    #[test]
    fn classify_by_extension() {
        let r = registry_with_modelica();
        assert_eq!(
            r.classify(&PathBuf::from("balloon.mo")),
            Some(DocumentKindId::new("modelica"))
        );
        // Case-insensitive.
        assert_eq!(
            r.classify(&PathBuf::from("Balloon.MO")),
            Some(DocumentKindId::new("modelica"))
        );
        // Unknown extension.
        assert_eq!(r.classify(&PathBuf::from("foo.xyz")), None);
        // No extension.
        assert_eq!(r.classify(&PathBuf::from("README")), None);
    }

    #[test]
    fn re_register_replaces_meta() {
        let mut r = registry_with_modelica();
        r.register(
            DocumentKindId::new("modelica"),
            DocumentKindMeta {
                display_name: "Modelica Model (override)".into(),
                extensions: vec!["mo"],
                ..Default::default()
            },
        );
        assert_eq!(
            r.meta(&DocumentKindId::new("modelica"))
                .unwrap()
                .display_name,
            "Modelica Model (override)"
        );
    }

    #[test]
    fn creatable_kinds_have_deterministic_display_order() {
        let mut r = registry_with_modelica();
        r.register(
            DocumentKindId::new("usd"),
            DocumentKindMeta {
                display_name: "USD Stage".into(),
                can_create_new: true,
                ..Default::default()
            },
        );
        r.register(
            DocumentKindId::new("generated"),
            DocumentKindMeta {
                display_name: "Generated Output".into(),
                can_create_new: false,
                ..Default::default()
            },
        );

        let names: Vec<_> = r
            .creatable()
            .into_iter()
            .map(|(_, meta)| meta.display_name.as_str())
            .collect();
        assert_eq!(names, ["Modelica Model", "USD Stage"]);
    }

    #[test]
    fn extensions_route_to_correct_id() {
        let mut r = registry_with_modelica();
        r.register(
            DocumentKindId::new("julia"),
            DocumentKindMeta {
                display_name: "Julia Script".into(),
                extensions: vec!["jl"],
                ..Default::default()
            },
        );
        assert_eq!(
            r.classify(&PathBuf::from("controller.jl")),
            Some(DocumentKindId::new("julia"))
        );
        assert_eq!(
            r.classify(&PathBuf::from("balloon.mo")),
            Some(DocumentKindId::new("modelica"))
        );
    }

    #[test]
    fn iter_yields_all_kinds() {
        let mut r = registry_with_modelica();
        r.register(
            DocumentKindId::new("usd"),
            DocumentKindMeta {
                display_name: "USD Stage".into(),
                extensions: vec!["usda", "usdc", "usd"],
                ..Default::default()
            },
        );
        let ids: std::collections::HashSet<_> = r.iter().map(|(id, _)| id.clone()).collect();
        assert!(ids.contains(&DocumentKindId::new("modelica")));
        assert!(ids.contains(&DocumentKindId::new("usd")));
        assert_eq!(ids.len(), 2);
    }
}
