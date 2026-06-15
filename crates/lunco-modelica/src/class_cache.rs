//! Engine-backed MSL class loader.
//!
//! Routes all MSL class lookups through the workbench's single
//! [`crate::engine_resource::ModelicaEngineHandle`] (workspace +
//! libraries unified). Misses on `peek_or_load_msl_class_blocking` resolve a
//! qualified name to a file via [`crate::library_fs`], read source
//! bytes from `lunco_assets::msl::global_msl_source`, and feed the
//! result into the workspace engine's session via `add_document`.
//!
//! ## Why one engine
//!
//! Earlier the MSL class cache lived in a separate process-wide
//! `Session` (a `Mutex<ModelicaEngine>` here in `class_cache.rs`)
//! disjoint from the workspace engine that holds user docs. That
//! split made `class_inherited_annotations_query` for a user class
//! that extends an MSL base return empty — the workspace engine
//! couldn't see the base. Routing both into one session resolves
//! cross-tier inheritance walks naturally.
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

/// MSL class-lookup behaviour for resolver helpers in `diagram` and
/// `canvas_projection`. Replaces the `&dyn Fn(&str) -> Option<...>`
/// parameter that used to thread one of two static fn pointers
/// through every helper.
///
/// Both modes route through the workspace [`crate::engine::ModelicaEngine`]
/// (engine consolidation) — they differ only in what to do on a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslLookupMode {
    /// Cache-only: a miss returns `None`. Use from off-thread tasks
    /// that must not block on rumoca parses (notably the canvas
    /// projection task on `AsyncComputeTaskPool`). The icon
    /// resolver falls back to defaults until a later edit / drill-in
    /// warms the engine session.
    Cached,
    /// Load on miss: the call blocks the thread to read + parse the
    /// missing file into the engine session. Safe from the main
    /// thread / tests / observers; risky from off-thread tasks
    /// where the lock contention can stall the deadline.
    Loading,
}

impl MslLookupMode {
    /// Resolve `qualified` using this mode's policy.
    pub fn lookup(
        self,
        qualified: &str,
    ) -> Option<Arc<rumoca_compile::parsing::ast::ClassDef>> {
        match self {
            Self::Cached => peek_msl_class_cached(qualified),
            Self::Loading => peek_or_load_msl_class_blocking(qualified),
        }
    }
}

/// Read MSL source bytes for a relative path, going through the
/// process-wide [`lunco_assets::msl::MslAssetSource`]. Returns
/// `None` if the source hasn't been installed yet (web boot before
/// fetch completes) or the path isn't present.
fn read_msl_source_bytes(path: &std::path::Path) -> Option<String> {
    let bytes = lunco_assets::msl::msl_read(path)?;
    String::from_utf8(bytes).ok()
}

/// Resolve a fully-qualified MSL class name to its `Arc<ClassDef>`
/// against the workbench's workspace engine. Loads the containing
/// file into the session on first miss; cheap (HashMap hit) on
/// every subsequent call once warm.
///
/// Returns `None` if the engine handle isn't installed yet (early
/// boot) or the file can't be located. Behaviour at the call sites
/// matches the previous static-MSL-engine implementation: a None
/// during boot lets icon/connector resolvers fall back to defaults
/// until MSL lands.
pub fn peek_or_load_msl_class_blocking(
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

    // Phase 2: locate + parse OUTSIDE the lock. This is the slow
    // step (file I/O + rumoca parse + extends-chain resolution can
    // take seconds for MSL classes with deep inheritance). Holding
    // the engine mutex across this step froze the UI: every
    // main-thread system that touches the engine
    // (`drive_engine_sync`, icon lookups, inspector queries) would
    // block until the parse completed. Parse first, install second.
    let path = resolve_class_path_indexed(qualified)
        .or_else(|| locate_library_file(qualified))?;
    let uri = path.to_string_lossy().replace('\\', "/");

    // Pre-parsed MSL bundle: AST is parsed by the indexer, no rumoca
    // work here. `parsed_msl_bundle` lazily materialises it from
    // `parsed-msl.bin` on native (and reuses the wasm-decoded slot),
    // so a drill-in is an in-memory lookup on both targets instead of a
    // per-file parse.
    let cached_ast = crate::msl_remote::parsed_msl_bundle()
        .and_then(|b| b.iter().find(|(k, _)| k == &uri).map(|(_, ast)| ast.clone()));

    let parsed_ast: Option<rumoca_compile::parsing::ast::StoredDefinition> = match cached_ast {
        Some(ast) => Some(ast),
        None => {
            #[cfg(target_arch = "wasm32")]
            {
                bevy::log::warn!(
                    "[class_cache] MSL cache miss for {qualified} (uri={uri}); \
                     wasm refuses sync parse — rendering default until worker fills"
                );
                return None;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let source = read_msl_source_bytes(&path)?;
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
        engine.session_mut().add_parsed_batch(vec![(uri, parsed_ast)]);
    }
    engine.class_def(qualified).map(Arc::new)
}

/// Non-blocking variant of [`peek_or_load_msl_class_blocking`] — returns the
/// `Arc<ClassDef>` if the engine session already holds it, and
/// `None` *without triggering a load* on a miss.
///
/// Use this from hot paths that must not block on rumoca parse —
/// notably the projection task running on Bevy's AsyncComputeTaskPool,
/// where a sync MSL parse from inside a worker that's already serving
/// a parent rumoca parse stalls for the projection deadline.
pub fn peek_msl_class_cached(
    qualified: &str,
) -> Option<Arc<rumoca_compile::parsing::ast::ClassDef>> {
    let handle = crate::engine_resource::global_engine_handle()?;
    let mut engine = handle.lock();
    if !engine.has_class(qualified) {
        return None;
    }
    engine.class_def(qualified).map(Arc::new)
}
