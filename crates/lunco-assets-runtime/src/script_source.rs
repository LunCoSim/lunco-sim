//! Script sources, addressed by canonical asset id.
//!
//! A script that lives outside the engine repo — for example a campaign
//! scenario or a policy synced from a peer and mounted as a Twin root over its
//! cache dir (`twin://<id>/`) — is
//! reached the same way every other asset is: through an [`AssetSource`] scheme,
//! resolved by [`lunco_assets_path::canonicalize`]. This registry is where the
//! loaded TEXT of those scripts lands, keyed by that canonical id.
//!
//! # Why a registry exists at all
//!
//! Script *languages* need to resolve imports **synchronously** — rhai's
//! `ModuleResolver::resolve` returns a module, not a future, and it is called in
//! the middle of evaluating a script. Bevy's asset loading is asynchronous, and on
//! wasm blocking the main thread is illegal. The two cannot be bridged directly.
//!
//! So loading is split from resolution: the asset pipeline fills this registry
//! ahead of time (async, through the normal `AssetServer` path, so every scheme
//! including a networked scenario's `twin://` root works), and resolution is then a pure
//! synchronous lookup. `LuncoUsdResolver` solves the identical problem for USD
//! layer composition the identical way; this is that pattern for scripts.
//!
//! # What deliberately is NOT here
//!
//! Nothing language-specific. No rhai types, no `import` syntax, no module
//! semantics — those belong to the language binding. This crate owns *asset
//! access and path resolution*, which is exactly the part every language would
//! otherwise reimplement (and get subtly different).

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use bevy::prelude::*;

/// Loaded script text, keyed by canonical asset id (`twin://ep1/lib.rhai`).
///
/// `Arc<RwLock<…>>` mirrors [`lunco_assets_core::twin_source::TwinRoots`]: the map is filled
/// by Bevy systems on the main thread and read from a language resolver that must
/// be `Send + Sync`. Cloning the resource clones the handle, not the contents, so
/// a resolver can hold one for its lifetime and see later insertions.
#[derive(Default)]
struct ScriptSourceState {
    revision: u64,
    sources: HashMap<String, String>,
    source_revisions: HashMap<String, u64>,
}

#[derive(Resource, Clone, Default)]
pub struct ScriptSources {
    sources: Arc<RwLock<ScriptSourceState>>,
}

impl ScriptSources {
    /// Canonical id for a script referenced as `path` from inside `importer`.
    ///
    /// Delegates entirely to [`lunco_assets_path::canonicalize`] — the SAME rule
    /// USD references use — then applies `default_ext` if the reference carries no
    /// extension, so `import "lib"` and `import "lib.rhai"` land on one key.
    ///
    /// This is the whole of "how a script reference becomes an id". A language
    /// binding calls it and does no path handling of its own; that is what keeps
    /// an `import` and an asset load from disagreeing about where a file is.
    pub fn canonical_id(path: &str, importer: Option<&str>, default_ext: &str) -> String {
        // rhai hands the importing script's id as an `Option` (absent for a
        // top-level script), so absence maps onto the explicit root case rather
        // than an empty anchor that would silently resolve against another root.
        let id = match importer {
            Some(anchor) => lunco_assets_path::canonicalize(path, anchor),
            None => lunco_assets_path::canonicalize_root(path),
        };
        // Only the final segment can carry the extension; a dot earlier in the
        // path (a versioned directory, say) must not suppress it.
        let has_ext = id.rsplit('/').next().is_some_and(|seg| seg.contains('.'));
        if has_ext {
            id
        } else {
            format!("{id}.{default_ext}")
        }
    }

    /// Text previously registered under `id`, if any.
    pub fn get(&self, id: &str) -> Option<String> {
        self.sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .sources
            .get(id)
            .cloned()
    }

    /// Register (or replace) the text for `id`. Replacement is what makes a
    /// hot-reloaded script visible to the next resolution.
    pub fn insert(&self, id: impl Into<String>, text: impl Into<String>) {
        let id = id.into();
        let text = text.into();
        let mut state = self.sources.write().unwrap_or_else(PoisonError::into_inner);
        if state.sources.get(&id) == Some(&text) {
            return;
        }
        state.sources.insert(id.clone(), text);
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state.source_revisions.insert(id, revision);
    }

    /// Register or replace borrowed source text without allocating when the
    /// canonical id already carries the same bytes.
    pub fn insert_if_changed(&self, id: &str, text: &str) -> bool {
        let mut state = self.sources.write().unwrap_or_else(PoisonError::into_inner);
        if state.sources.get(id).is_some_and(|current| current == text) {
            return false;
        }
        state.sources.insert(id.to_owned(), text.to_owned());
        state.revision = state.revision.wrapping_add(1);
        let revision = state.revision;
        state.source_revisions.insert(id.to_owned(), revision);
        true
    }

    /// Remove a source whose Bevy asset has reached the end of its lifecycle.
    ///
    /// This registry is a synchronous view of Bevy's asset graph, not an
    /// independent cache with a longer lifetime. Retiring the text at this
    /// boundary prevents an unloaded script from remaining importable after its
    /// last owning handle has gone away.
    pub fn remove(&self, id: &str) -> bool {
        let mut state = self.sources.write().unwrap_or_else(PoisonError::into_inner);
        if state.sources.remove(id).is_some() {
            state.source_revisions.remove(id);
            state.revision = state.revision.wrapping_add(1);
            true
        } else {
            false
        }
    }

    /// Every registered id. Used to report what WAS available when a lookup
    /// misses — a bare "module not found" is nearly useless for diagnosing a
    /// scheme or anchoring mistake.
    pub fn ids(&self) -> Vec<String> {
        let state = self.sources.read().unwrap_or_else(PoisonError::into_inner);
        let mut ids: Vec<String> = state.sources.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Monotonic revision of the registered source set.
    pub fn revision(&self) -> u64 {
        self.sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .revision
    }

    /// Revision of one currently registered canonical source identity. A
    /// missing id has revision zero; insertion, replacement, and removal all
    /// change a prior live revision. This lets a consumer watch only its
    /// imported sources without invalidating on an unrelated registry edit.
    pub fn source_revision(&self, id: &str) -> u64 {
        self.sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .source_revisions
            .get(id)
            .copied()
            .unwrap_or(0)
    }

    /// One immutable, canonically ordered source-set snapshot and its revision.
    pub fn snapshot(&self) -> (u64, Vec<(String, String)>) {
        let state = self.sources.read().unwrap_or_else(PoisonError::into_inner);
        let mut sources = state
            .sources
            .iter()
            .map(|(id, text)| (id.clone(), text.clone()))
            .collect::<Vec<_>>();
        sources.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        (state.revision, sources)
    }

    pub fn len(&self) -> usize {
        self.sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .sources
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_the_shared_canonicalization_plus_a_default_extension() {
        // Relative import inside a twin-sourced script stays in that twin.
        assert_eq!(
            ScriptSources::canonical_id("lib", Some("twin://ep1/main.rhai"), "rhai"),
            "twin://ep1/lib.rhai"
        );
        // Spelling the extension changes nothing — same key either way.
        assert_eq!(
            ScriptSources::canonical_id("lib.rhai", Some("twin://ep1/main.rhai"), "rhai"),
            "twin://ep1/lib.rhai"
        );
        // An absolute reference ignores the importer.
        assert_eq!(
            ScriptSources::canonical_id("twin://ep1/lib.rhai", Some("lunco://a/b.rhai"), "rhai"),
            "twin://ep1/lib.rhai"
        );
    }

    #[test]
    fn a_dot_in_a_directory_does_not_suppress_the_extension() {
        assert_eq!(
            ScriptSources::canonical_id("v1.2/lib", None, "rhai"),
            "v1.2/lib.rhai"
        );
    }

    #[test]
    fn round_trips_text() {
        let s = ScriptSources::default();
        assert!(s.get("twin://ep1/lib.rhai").is_none());
        s.insert("twin://ep1/lib.rhai", "fn f() { 1 }");
        assert_eq!(
            s.get("twin://ep1/lib.rhai").as_deref(),
            Some("fn f() { 1 }")
        );
        assert_eq!(s.ids(), vec!["twin://ep1/lib.rhai".to_string()]);
        assert!(s.remove("twin://ep1/lib.rhai"));
        assert!(s.get("twin://ep1/lib.rhai").is_none());
        assert!(!s.remove("twin://ep1/lib.rhai"));
    }

    #[test]
    fn source_snapshot_has_one_revision_and_only_changes_for_new_content() {
        let sources = ScriptSources::default();
        let initial = sources.revision();
        assert_eq!(sources.source_revision("twin://ep1/lib.rhai"), 0);
        sources.insert("twin://ep1/lib.rhai", "fn f() { 1 }");
        let added = sources.revision();
        assert_ne!(added, initial);
        let added_source_revision = sources.source_revision("twin://ep1/lib.rhai");
        assert_ne!(added_source_revision, 0);
        sources.insert("twin://ep1/lib.rhai", "fn f() { 1 }");
        assert_eq!(sources.revision(), added);
        assert_eq!(
            sources.source_revision("twin://ep1/lib.rhai"),
            added_source_revision
        );

        let (snapshot_revision, snapshot) = sources.snapshot();
        assert_eq!(snapshot_revision, added);
        assert_eq!(
            snapshot,
            [("twin://ep1/lib.rhai".to_owned(), "fn f() { 1 }".to_owned())]
        );

        sources.insert("twin://ep1/lib.rhai", "fn f() { 2 }");
        assert_ne!(sources.revision(), snapshot_revision);
        let replaced_source_revision = sources.source_revision("twin://ep1/lib.rhai");
        assert_ne!(replaced_source_revision, added_source_revision);
        sources.insert("twin://unrelated/lib.rhai", "fn g() { 3 }");
        assert_eq!(
            sources.source_revision("twin://ep1/lib.rhai"),
            replaced_source_revision
        );
        assert!(sources.remove("twin://ep1/lib.rhai"));
        assert_ne!(
            sources.source_revision("twin://ep1/lib.rhai"),
            replaced_source_revision
        );
        assert_eq!(snapshot[0].1, "fn f() { 1 }");
    }
}
