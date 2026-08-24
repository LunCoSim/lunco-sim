//! Engine-backed source-aware Modelica class loader.
//!
//! Routes all class lookups through the workbench's single
//! [`crate::engine_resource::ModelicaEngineHandle`] (workspace +
//! libraries unified). Misses on `peek_or_load_class_blocking` resolve a
//! qualified name to either a bundled package or a library file and
//! feeds the result into the workspace engine's session.
//!
//! ## Why one engine
//!
//! Earlier the MSL class cache lived in a separate process-wide
//! `Session` (a `Mutex<ModelicaEngine>` here in `class_cache.rs`)
//! disjoint from the workspace engine that holds user docs. That
//! split made inheritance queries for a user class that extends an MSL
//! base return empty — the workspace engine couldn't see the base.
//! Routing both into one session resolves cross-tier inheritance walks
//! naturally.
//!
//! ## Bootstrap timing
//!
//! Web: `engine_resource::drive_msl_bootstrap` calls
//! `replace_parsed_source_set("msl", DurableExternal, …)` once when
//! `MslLoadState::Ready` flips and `GLOBAL_PARSED_MSL` is populated.
//! After that point every MSL class is resolvable without per-class
//! disk I/O.
//!
//! Native: bootstrap stays lazy — the system above logs and idles,
//! and the helpers below pull individual `.mo` files into the
//! session via `add_document` on first miss. Same content-hash
//! cache backs both paths.

use std::sync::Arc;

use crate::library_fs::{locate_library_file, resolve_class_path_indexed};

/// Class-lookup behaviour for resolver helpers in `diagram` and
/// `canvas_projection`. Replaces the `&dyn Fn(&str) -> Option<...>`
/// parameter that used to thread one of two static fn pointers
/// through every helper.
///
/// Both modes route through the workspace [`crate::engine::ModelicaEngine`]
/// (engine consolidation) — they differ only in what to do on a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLookupMode {
    /// Cache-only: a miss returns `None`. Use from off-thread tasks that must
    /// not block on rumoca parses.
    Cached,
    /// Load on miss: the call blocks the thread to read + parse the
    /// missing file into the engine session. Safe from the main
    /// thread / tests / observers; risky from off-thread tasks
    /// where the lock contention can stall the deadline.
    Loading,
}

/// Availability reported by the shared source-root resolver. Rendering uses
/// this to distinguish a class that is still loading from one that is truly
/// absent; neither state is allowed to masquerade as a resolved class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassAvailability {
    Ready,
    Loading,
    Missing,
}

pub fn class_availability(qualified: &str) -> ClassAvailability {
    let Some(handle) = crate::engine_resource::global_engine_handle() else {
        return ClassAvailability::Loading;
    };
    let root = qualified.split('.').next().unwrap_or(qualified);
    let is_bundled_root = lunco_assets::models::package_roots_live()
        .iter()
        .any(|candidate| candidate == root);
    let (has_class, has_root, root_failed) = {
        let Some(mut engine) = handle.try_lock() else {
            // The query is made from the projection worker. Contention is a
            // normal in-flight state, never a reason to wait behind parsing
            // or source-root installation.
            if is_bundled_root {
                let _ = handle.ensure_library_root_async(root);
            }
            return ClassAvailability::Loading;
        };
        let has_class = engine.has_class(qualified);
        let has_root = engine.has_class(root);
        let root_failed = engine.library_root_failure(root).is_some();
        (has_class, has_root, root_failed)
    };
    if has_class {
        return ClassAvailability::Ready;
    }
    let root = qualified.split('.').next().unwrap_or(qualified);
    if is_bundled_root {
        // A qualified reference is itself a normal Modelica search-path
        // request. If the bundled root is not resident yet, start the shared
        // asynchronous load here; callers do not need generated-document
        // metadata or a library-specific prewarm branch to make a class
        // visible in the editor.
        if !root_failed && !has_root {
            let _ = handle.ensure_library_root_async(root);
        }
        return if root_failed || has_root {
            ClassAvailability::Missing
        } else {
            ClassAvailability::Loading
        };
    }
    if resolve_class_path_indexed(qualified).is_some() || locate_library_file(qualified).is_some() {
        ClassAvailability::Loading
    } else {
        ClassAvailability::Missing
    }
}

/// Explain a terminal source-root failure to the diagram resolver without
/// making UI code reach into the engine's private loading state.
pub fn class_resolution_message(qualified: &str) -> Option<String> {
    let handle = crate::engine_resource::global_engine_handle()?;
    let root = qualified.split('.').next().unwrap_or(qualified);
    let engine = handle.try_lock()?;
    engine
        .library_root_failure(root)
        .map(|error| format!("source root `{root}` failed: {error}"))
}

impl ClassLookupMode {
    /// Resolve `qualified` using this mode's policy.
    pub fn lookup(self, qualified: &str) -> Option<Arc<rumoca_compile::parsing::ast::ClassDef>> {
        match self {
            Self::Cached => peek_class_cached(qualified),
            Self::Loading => peek_or_load_class_blocking(qualified),
        }
    }
}

/// Read library source bytes for a relative path, going through the
/// process-wide [`lunco_assets::msl::MslAssetSource`]. Returns
/// `None` if the source hasn't been installed yet (web boot before
/// fetch completes) or the path isn't present.
fn read_source_bytes(path: &std::path::Path) -> Option<String> {
    let bytes = lunco_assets::msl::msl_read(path)?;
    String::from_utf8(bytes).ok()
}

/// Resolve a fully-qualified class name to its `Arc<ClassDef>` against
/// the workbench's workspace engine. Loads the containing bundled
/// package or library file on first miss; cheap (HashMap hit) on
/// every subsequent call once warm.
///
/// Bundled package roots are resolved before the MSL filesystem index.
/// This is the important no-MSL path for generated `LunCo.*` documents:
/// the normal Modelica source/diagram pipeline can load the native package
/// and then use the same icon, connector, and inheritance queries as MSL.
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
    if let Some(root) = qualified.split('.').next() {
        if lunco_assets::models::package_roots_live()
            .iter()
            .any(|candidate| candidate == root)
        {
            let mut engine = handle.lock();
            if engine.ensure_library_root(root) {
                return engine.class_def(qualified).map(Arc::new);
            }
            return None;
        }
    }

    // Phase 2: locate + parse OUTSIDE the lock. This is the slow
    // step (file I/O + rumoca parse + extends-chain resolution can
    // take seconds for MSL classes with deep inheritance). Holding
    // the engine mutex across this step froze the UI: every
    // main-thread system that touches the engine
    // (`drive_engine_sync`, icon lookups, inspector queries) would
    // block until the parse completed. Parse first, install second.
    let path = resolve_class_path_indexed(qualified).or_else(|| locate_library_file(qualified))?;
    let uri = lunco_assets::asset_path::slashed(&path);

    // Pre-parsed MSL bundle: AST is parsed by the indexer, no rumoca
    // work here. `parsed_msl_bundle` lazily materialises it from
    // `parsed-msl.bin` on native (and reuses the wasm-decoded slot),
    // so a drill-in is an in-memory lookup on both targets instead of a
    // per-file parse.
    let cached_ast = crate::msl_remote::parsed_msl_bundle().and_then(|b| {
        b.iter()
            .find(|(k, _)| k == &uri)
            .map(|(_, ast)| ast.clone())
    });

    let parsed_ast: Option<rumoca_compile::parsing::ast::StoredDefinition> = match cached_ast {
        Some(ast) => Some(ast),
        None => {
            #[cfg(target_arch = "wasm32")]
            {
                bevy::log::warn!(
                    "[class_cache] MSL cache miss for {qualified} (uri={uri}); \
                     wasm refuses sync parse — class remains unresolved until worker fills"
                );
                return None;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let source = read_source_bytes(&path)?;
                // Parse standalone without holding the engine lock.
                // `add_document` would do this internally but inside
                // the lock; the standalone `parse_to_ast` lets us
                // pay the parse cost off-lock and install via
                // `add_parsed_batch` (cheap) afterwards.
                match rumoca_phase_parse::parse_to_ast(&source, &uri) {
                    Ok(ast) => Some(ast),
                    Err(e) => {
                        bevy::log::warn!(
                            "[class_cache] rumoca parse failed for {qualified} (uri={uri}): {e:?}"
                        );
                        return None;
                    }
                }
            }
        }
    };

    let parsed_ast = parsed_ast?;

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

/// Non-blocking variant of [`peek_or_load_class_blocking`] — returns the
/// `Arc<ClassDef>` if the engine session already holds it, and
/// `None` *without triggering a load* on a miss.
///
/// Use this from hot paths that must not block on rumoca parse —
/// notably the projection task running on Bevy's AsyncComputeTaskPool,
/// where a sync MSL parse from inside a worker that's already serving
/// a parent rumoca parse stalls for the projection deadline.
pub fn peek_class_cached(qualified: &str) -> Option<Arc<rumoca_compile::parsing::ast::ClassDef>> {
    let handle = crate::engine_resource::global_engine_handle()?;
    let mut engine = handle.try_lock()?;
    if !engine.has_class(qualified) {
        return None;
    }
    engine.class_def(qualified).map(Arc::new)
}
