//! Script sources, addressed by canonical asset id.
//!
//! A script that lives outside the engine repo — a campaign scenario in
//! `lunco-marketing`, a policy synced from a peer and mounted as a Twin root over
//! its cache dir (`twin://<id>/`) — is
//! reached the same way every other asset is: through an [`AssetSource`] scheme,
//! resolved by [`crate::asset_path::canonicalize`]. This registry is where the
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
use std::sync::{Arc, RwLock};

use bevy::prelude::*;

/// Loaded script text, keyed by canonical asset id (`twin://ep1/lib.rhai`).
///
/// `Arc<RwLock<…>>` mirrors [`crate::twin_source::TwinRoots`]: the map is filled
/// by Bevy systems on the main thread and read from a language resolver that must
/// be `Send + Sync`. Cloning the resource clones the handle, not the contents, so
/// a resolver can hold one for its lifetime and see later insertions.
#[derive(Resource, Clone, Default)]
pub struct ScriptSources {
    sources: Arc<RwLock<HashMap<String, String>>>,
}

impl ScriptSources {
    /// Canonical id for a script referenced as `path` from inside `importer`.
    ///
    /// Delegates entirely to [`crate::asset_path::canonicalize`] — the SAME rule
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
            Some(anchor) => crate::asset_path::canonicalize(path, anchor),
            None => crate::asset_path::canonicalize_root(path),
        };
        // Only the final segment can carry the extension; a dot earlier in the
        // path (a versioned directory, say) must not suppress it.
        let has_ext = id
            .rsplit('/')
            .next()
            .is_some_and(|seg| seg.contains('.'));
        if has_ext {
            id
        } else {
            format!("{id}.{default_ext}")
        }
    }

    /// Text previously registered under `id`, if any.
    pub fn get(&self, id: &str) -> Option<String> {
        self.sources.read().ok()?.get(id).cloned()
    }

    /// Register (or replace) the text for `id`. Replacement is what makes a
    /// hot-reloaded script visible to the next resolution.
    pub fn insert(&self, id: impl Into<String>, text: impl Into<String>) {
        if let Ok(mut map) = self.sources.write() {
            map.insert(id.into(), text.into());
        }
    }

    /// Every registered id. Used to report what WAS available when a lookup
    /// misses — a bare "module not found" is nearly useless for diagnosing a
    /// scheme or anchoring mistake.
    pub fn ids(&self) -> Vec<String> {
        self.sources
            .read()
            .map(|m| {
                let mut v: Vec<String> = m.keys().cloned().collect();
                v.sort();
                v
            })
            .unwrap_or_default()
    }

    pub fn len(&self) -> usize {
        self.sources.read().map(|m| m.len()).unwrap_or(0)
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
        assert_eq!(s.get("twin://ep1/lib.rhai").as_deref(), Some("fn f() { 1 }"));
        assert_eq!(s.ids(), vec!["twin://ep1/lib.rhai".to_string()]);
    }
}
