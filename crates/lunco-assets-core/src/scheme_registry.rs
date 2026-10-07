//! Which local directory a scheme's references live under — as a REGISTRY, not a
//! match arm.
//!
//! Most of the resolution stack is already scheme-agnostic: [`has_scheme`] tests
//! for any scheme, [`canonicalize`] preserves whichever one it finds, and Bevy's
//! `AssetSource` registry dispatches loads by name. The local-path side was the
//! exception — it was a hardcoded two-way match (`twin://`, else the library), so
//! a scheme that this crate itself registered as an `AssetSource` still had no
//! local path, and adding one meant editing a `match` in a third file.
//!
//! [`has_scheme`]: crate::has_scheme
//! [`canonicalize`]: crate::asset_path::canonicalize
//!
//! # Why local paths need their own registry at all
//!
//! Bevy's `AssetSource` is enough for anything read through the `AssetServer`.
//! But several callers must reach bytes WITHOUT it — scenario sync hashing files,
//! the shader `@fragment` pre-validator, file dialogs — and those need a real
//! filesystem path. This is the read-side mirror of
//! [`register_lunco_asset_sources`](crate::register_lunco_asset_sources): every
//! scheme registered there registers its local root here, so the two cannot
//! disagree about where a scheme's bytes are.
//!
//! # Roots are resolved lazily
//!
//! A handler is a closure over the scheme-relative remainder, not a fixed
//! `PathBuf`, because not every scheme HAS one fixed root: `twin://<name>/<rel>`
//! picks its root per Twin name, and the set of open Twins changes at runtime. A
//! closure covers both the constant case and the stateful one without the
//! registry needing to know which is which.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use bevy::prelude::*;

/// Failure of the local scheme registry. A poisoned lock is an unavailable
/// registry, not an unregistered scheme; callers must keep that distinction in
/// their diagnostics.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SchemeRegistryError {
    /// A registry mutation or lookup observed a poisoned lock.
    #[error("scheme registry is unavailable because its lock is poisoned")]
    RegistryPoisoned,
    /// A registered source could not resolve its configured filesystem root.
    #[error("asset path resolution failed: {0}")]
    AssetResolution(String),
}

impl From<std::io::Error> for SchemeRegistryError {
    fn from(error: std::io::Error) -> Self {
        Self::AssetResolution(error.to_string())
    }
}

/// Maps a scheme's remainder (everything after `scheme://`) to a local path, or
/// `None` when this scheme has no local bytes right now — an unopened Twin, or a
/// scheme like `http(s)://` that has no filesystem form at all.
pub type SchemeRoot =
    Arc<dyn Fn(&str) -> Result<Option<PathBuf>, SchemeRegistryError> + Send + Sync>;

/// Scheme → local root. Cloning shares one registry (same `Arc`), so a handler
/// registered after startup is visible to every holder.
#[derive(Resource, Clone, Default)]
pub struct SchemeRegistry {
    handlers: Arc<RwLock<HashMap<String, SchemeRoot>>>,
}

impl SchemeRegistry {
    /// Register (or replace) the local-root resolver for `scheme`.
    ///
    /// Adding a scheme is this ONE call — there is deliberately no second
    /// central list to edit, so the AssetServer and local-path consumers cannot
    /// drift.
    pub fn register(
        &self,
        scheme: impl Into<String>,
        root: impl Fn(&str) -> Result<Option<PathBuf>, SchemeRegistryError> + Send + Sync + 'static,
    ) -> Result<(), SchemeRegistryError> {
        self.handlers
            .write()
            .map(|mut h| {
                h.insert(scheme.into(), Arc::new(root));
            })
            .map_err(|_| SchemeRegistryError::RegistryPoisoned)
    }

    /// The local filesystem path `reference` resolves to.
    ///
    /// A reference with no scheme is engine-library-relative — the same default
    /// the `lunco://` source applies, so a bare `shaders/wheel.wgsl` and
    /// `lunco://shaders/wheel.wgsl` agree. A reference naming an UNREGISTERED
    /// scheme returns `None` rather than being silently treated as library-
    /// relative, which would resolve it to a path that does not exist.
    pub fn local_path(&self, reference: &str) -> Result<Option<PathBuf>, SchemeRegistryError> {
        let Some((scheme, rest)) = crate::asset_path::split_scheme(reference) else {
            return crate::engine_asset_local_path(reference).map_err(Into::into);
        };
        let handler = self
            .handlers
            .read()
            .map_err(|_| SchemeRegistryError::RegistryPoisoned)?
            .get(scheme)
            .cloned();
        match handler {
            Some(handler) => handler(rest),
            None => Ok(None),
        }
    }

    /// Every registered scheme, sorted — for diagnostics when a lookup misses.
    pub fn schemes(&self) -> Result<Vec<String>, SchemeRegistryError> {
        self.handlers
            .read()
            .map(|h| {
                let mut v: Vec<String> = h.keys().cloned().collect();
                v.sort();
                v
            })
            .map_err(|_| SchemeRegistryError::RegistryPoisoned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_references_are_library_relative() {
        let reg = SchemeRegistry::default();
        assert_eq!(
            reg.local_path("shaders/wheel.wgsl").unwrap(),
            Some(crate::engine_shader_path("wheel").unwrap())
        );
    }

    #[test]
    fn an_unregistered_scheme_resolves_to_nothing_rather_than_the_library() {
        let reg = SchemeRegistry::default();
        assert_eq!(reg.local_path("nope://a/b.usda").unwrap(), None);
    }

    #[test]
    fn a_registered_scheme_dispatches_on_its_remainder() {
        let reg = SchemeRegistry::default();
        reg.register("pack", |rest| Ok(Some(PathBuf::from("/packs").join(rest))))
            .unwrap();
        assert_eq!(
            reg.local_path("pack://a/b.usda").unwrap(),
            Some(PathBuf::from("/packs/a/b.usda"))
        );
        assert_eq!(reg.schemes().unwrap(), vec!["pack".to_string()]);
    }

    #[test]
    fn registered_scheme_reports_resolution_failure() {
        let reg = SchemeRegistry::default();
        reg.register("broken", |_| {
            Err(SchemeRegistryError::AssetResolution(
                "unreadable root".into(),
            ))
        })
        .unwrap();
        assert_eq!(
            reg.local_path("broken://asset"),
            Err(SchemeRegistryError::AssetResolution(
                "unreadable root".into()
            ))
        );
    }

    /// The stateful case: a handler may decline (Twin not open) without the
    /// registry knowing anything about Twins.
    #[test]
    fn a_handler_may_decline() {
        let reg = SchemeRegistry::default();
        reg.register("twin", |_| Ok(None)).unwrap();
        assert_eq!(reg.local_path("twin://ep1/x.usda").unwrap(), None);
    }
}
