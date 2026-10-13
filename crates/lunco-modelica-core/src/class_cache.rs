//! Engine-backed source-aware Modelica class loader.
//!
//! Routes all class lookups through the workbench's single
//! [`crate::engine_resource::ModelicaEngineHandle`] (workspace +
//! libraries unified). Misses on `peek_or_load_class_blocking` resolve a
//! qualified name to either a bundled package or a library file and
//! feeds the result into the workspace engine's session.
//!
//! ## Bootstrap timing
//!
//! Web: `engine_resource::drive_source_bundle_bootstrap` calls
//! `replace_in_memory_source_set("source-bundle", DurableExternal, …)` once the parsed
//! bundle is resident. After that point every source library class is resolvable without
//! per-class disk I/O.
//!
//! Native: bootstrap stays lazy until the first source library class lookup. That lookup
//! seats the complete parsed source set through the same engine boundary as
//! the web bootstrap, so dependent classes observe one shared namespace.

use std::sync::Arc;

use crate::library_fs::{locate_library_file, resolve_class_path_indexed};

/// Read library source bytes for a relative path, going through the
/// process-wide [`lunco_assets_runtime::library::LibraryAssetSource`]. Returns
/// `None` if the source hasn't been installed yet (web boot before
/// fetch completes) or the path isn't present.
#[cfg(not(target_arch = "wasm32"))]
fn read_source_bytes(path: &std::path::Path) -> Option<String> {
    let bytes = lunco_assets_runtime::library::library_read(path)?;
    String::from_utf8(bytes).ok()
}

/// Resolve a fully-qualified class name to its `Arc<ClassDef>` against
/// the workbench's workspace engine. Loads the containing bundled
/// package or library file on first miss; cheap (HashMap hit) on
/// every subsequent call once warm.
///
/// Bundled package roots are resolved before the source library filesystem index.
/// This is the important no-source library path for generated `LunCo.*` documents:
/// the normal Modelica source/diagram pipeline can load the native package
/// and then use the same icon, connector, and inheritance queries as source library.
///
pub fn peek_or_load_class_blocking(
    qualified: &str,
) -> Option<Arc<rumoca_compile::parsing::ast::ClassDef>> {
    let handle = crate::engine_resource::global_engine_handle()?;

    // Phase 1: brief lock to check whether the class is already
    // installed. If yes, just hand it back.
    {
        let mut engine = handle.lock();
        if engine.has_class(qualified) {
            return engine.class_def(qualified).map(Arc::new);
        }
    }

    // Bundled libraries are small source roots and use the engine's
    // source-aware package loader. They are installed as one root so
    // cross-file `within` and `extends` resolution remains canonical.
    if let Some(root) = rumoca_core::qualified_name_segments(qualified).next() {
        if lunco_assets_runtime::models::package_roots_live()
            .is_ok_and(|roots| roots.iter().any(|candidate| candidate == root))
        {
            let mut engine = handle.lock();
            if engine.ensure_source_root(root) {
                return engine.class_def(qualified).map(Arc::new);
            }
            return None;
        }
    }

    // Phase 2: locate + parse OUTSIDE the lock. This is the slow
    // step (file I/O + rumoca parse + extends-chain resolution can
    // take seconds for source library classes with deep inheritance). Holding
    // the engine mutex across this step froze the UI: every
    // main-thread system that touches the engine
    // (`drive_engine_sync`, icon lookups, inspector queries) would
    // block until the parse completed. Parse first, install second.
    let path = resolve_class_path_indexed(qualified).or_else(|| locate_library_file(qualified))?;
    let uri = lunco_assets_path::slashed(&path);

    // A pre-parsed source library document is part of an immutable source set, not an
    // independent workspace document. Seat the complete bundle through the
    // shared engine boundary before returning the requested class. Installing
    // one target AST here leaves sibling types (for example
    // `Continuous.Filter` and `Sources.Step`) absent from the session, which
    // makes diagram projection report permanent Loading placeholders.
    let library_bundle =
        lunco_modelica_library::source_library::parsed_source_bundle().and_then(|bundle| {
            bundle
                .iter()
                .any(|document| document.uri() == uri)
                .then_some(bundle)
        });
    if let Some(bundle) = library_bundle {
        let mut engine = handle.lock();
        if !engine.source_set_installed("source-bundle") {
            let docs = (**bundle).clone();
            let count = engine.replace_in_memory_source_set(
                "source-bundle",
                rumoca_compile::compile::SourceRootKind::DurableExternal,
                docs,
            );
            bevy::log::info!(
                "[class_cache] installed complete source bundle into workspace engine: {count} parsed docs"
            );
        }
        return engine.class_def(qualified).map(Arc::new);
    }

    #[cfg(target_arch = "wasm32")]
    {
        bevy::log::warn!(
            "[class_cache] source library cache miss for {qualified} (uri={uri}); \
                 wasm refuses sync parse — class remains unresolved until worker fills"
        );
        return None;
    }

    #[cfg(not(target_arch = "wasm32"))]
    let parsed_ast: rumoca_compile::parsing::ast::StoredDefinition = {
        let source = read_source_bytes(&path)?;
        // Parse standalone without holding the engine lock.
        // `add_document` would do this internally but inside
        // the lock; the standalone `parse_to_ast` lets us
        // pay the parse cost off-lock and install via
        // `add_parsed_batch` (cheap) afterwards.
        match rumoca_phase_parse::parse_to_ast(&source, &uri) {
            Ok(ast) => ast,
            Err(e) => {
                bevy::log::warn!(
                    "[class_cache] rumoca parse failed for {qualified} (uri={uri}): {e:?}"
                );
                return None;
            }
        }
    };

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Phase 3: re-acquire the lock briefly to install. Another
        // task may have raced ahead and installed the same class
        // while we were parsing — `add_parsed_batch` is idempotent
        // for matching content, and `class_def` returns whatever is
        // current. The wasted parse is acceptable; the alternative
        // (per-class loading mutex) is more state for negligible win.
        let mut engine = handle.lock();
        if !engine.has_class(qualified) {
            engine
                .session_mut()
                .add_parsed_batch(vec![(uri, parsed_ast)]);
        }
        engine.class_def(qualified).map(Arc::new)
    }
}
