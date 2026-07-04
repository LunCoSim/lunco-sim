//! Loaded USD stages — the live set of stage roots the user has
//! access to in this session.
//!
//! Mirrors `lunco_modelica::ui::loaded_classes::LoadedModelicaClasses`:
//! a flat registry of [`crate::ui::loaded_stages::LoadedStage`] entries surfaced as siblings in
//! the Twin browser's Models scope, regardless of where they came
//! from. Workspace docs, future bundled stages, future Twin externals
//! — all show up the same way.
//!
//! ## WP-8 view-model split
//!
//! Under the reactive-egui contract the Twin-browser `BrowserCtx` is
//! capability-narrowed (no `&mut World`, no `remove_resource`), so the
//! `UsdSceneSection` can no longer take `LoadedUsdStages` out of the
//! world and parse inline during paint. Parsing + naming now run in a
//! change-gated producer system ([`produce_usd_browser_view`]) that
//! refreshes each entry's parse cache and flattens the result into the
//! cloneable [`UsdBrowserView`] resource. The section reads that
//! view-model immutably and only paints.
//!
//! ## Lifecycle
//!
//! - **Workspace stages** — one [`WorkspaceStage`] per writable / Untitled
//!   USD document the user has open. Registered on
//!   [`DocumentOpened`](lunco_doc_bevy::DocumentOpened) for our kind,
//!   dropped on [`DocumentClosed`](lunco_doc_bevy::DocumentClosed).
//!   Wired in [`UsdUiPlugin`](crate::ui::UsdUiPlugin).
//! - **System stages** *(deferred)* — bundled / Twin-pinned stages
//!   loaded from disk. The trait surface is in place; the loader slots
//!   in alongside Twin externals.

use std::sync::Arc;

use bevy::prelude::*;
use lunco_doc::{Document, DocumentId};
use lunco_usd_bevy::{UsdData, usd_data::UsdDataExt};

use crate::registry::UsdDocumentRegistry;

/// A top-level USD stage loaded into the current session.
///
/// One trait impl per source kind: [`WorkspaceStage`] for writable
/// documents the user is authoring; future system / bundled / remote
/// loaders implement the same trait so the browser doesn't grow
/// per-source branches.
pub trait LoadedStage: Send + Sync + 'static {
    /// Stable id used as egui salt and for unregistration when the
    /// underlying source goes away (document closed, Twin closed).
    fn id(&self) -> &str;

    /// Editable? Drives the row's writable badge. Read-only system
    /// libraries render a lock affordance; Save respects this
    /// independently via document-level origin checks.
    fn writable(&self) -> bool {
        false
    }

    /// Default expand state on first render. Workspace stages default
    /// open (this is what the user is editing); future bundled
    /// libraries stay closed (huge prim trees, user expands on demand).
    fn default_open(&self) -> bool {
        false
    }

    /// If this entry corresponds to an open document, return its id
    /// so the browser can offer "show in viewport" affordances. The
    /// default is `None` for non-document entries (system libraries
    /// etc.); [`WorkspaceStage`] overrides.
    fn doc_id_for_viewport(&self) -> Option<DocumentId> {
        None
    }

    /// Refresh this entry's internal parse cache against the registry
    /// and flatten it into a cloneable [`UsdStageRow`]. Called by the
    /// change-gated [`produce_usd_browser_view`] producer — never during
    /// egui paint. Returns `None` when the backing source is gone (e.g.
    /// the document closed between the gate check and the rebuild).
    fn build_row(&mut self, registry: &UsdDocumentRegistry) -> Option<UsdStageRow>;
}

/// Live registry of [`crate::ui::loaded_stages::LoadedStage`] entries.
/// Maintained by the lifecycle observers in
/// [`UsdUiPlugin`](crate::ui::UsdUiPlugin) and read by the
/// [`produce_usd_browser_view`] producer (never directly by the panel).
#[derive(Resource, Default)]
pub struct LoadedUsdStages {
    /// Render order = registration order.
    pub entries: Vec<Box<dyn LoadedStage>>,
}

impl LoadedUsdStages {
    /// Append a new stage. Order is render order.
    pub fn register(&mut self, stage: Box<dyn LoadedStage>) {
        self.entries.push(stage);
    }

    /// Drop the entry whose [`LoadedStage::id`] matches. Returns
    /// `true` if an entry was removed.
    pub fn unregister(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|s| s.id() != id);
        before != self.entries.len()
    }

    /// True iff no stages are loaded right now.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────
// View-model — the cloneable render snapshot the section reads
// ─────────────────────────────────────────────────────────────────────

/// One stage's pre-derived render row. Cloneable so the section can
/// snapshot the whole list out of `BrowserCtx` (immutable read) before
/// painting, releasing the borrow ahead of any deferred dispatch.
#[derive(Clone)]
pub struct UsdStageRow {
    /// Stable egui id salt (the entry's [`LoadedStage::id`]).
    pub salt: String,
    /// Backing document, if this row corresponds to an open doc.
    pub doc_id: Option<DocumentId>,
    /// Display name shown on the top-level row.
    pub name: String,
    /// Writable badge driver.
    pub writable: bool,
    /// First-render expand state.
    pub default_open: bool,
    /// Parsed reader for the prim-tree walk. `None` on parse failure
    /// (see `parse_error`) or when the stage has no source yet. `Arc`
    /// so cloning the row is cheap.
    pub reader: Option<Arc<UsdData>>,
    /// Stashed parse error from the most recent failed parse, surfaced
    /// in the body so users see a malformed file instead of an empty
    /// tree.
    pub parse_error: Option<String>,
}

/// Change-gated view-model the [`UsdSceneSection`](crate::ui::browser_section::UsdSceneSection)
/// reads each frame. Rebuilt only when an entry is added/removed or a
/// document's generation advances — see [`produce_usd_browser_view`].
#[derive(Resource, Default)]
pub struct UsdBrowserView {
    /// One row per loaded stage, in registration order.
    pub stages: Vec<UsdStageRow>,
}

/// Producer for [`UsdBrowserView`]. Re-derives the row list (and each
/// entry's parse cache, via [`LoadedStage::build_row`]) only when the
/// `(stage id, document generation)` signature changes — so a static
/// scene costs one cheap signature walk per frame, no parsing and no
/// string churn (`AGENTS.md` §7.1 frame discipline).
pub fn produce_usd_browser_view(
    mut loaded: ResMut<LoadedUsdStages>,
    registry: Res<UsdDocumentRegistry>,
    mut view: ResMut<UsdBrowserView>,
    mut last_sig: Local<Vec<(String, u64)>>,
) {
    // Signature catches both structural changes (entries added/removed
    // → ids differ) and edits (the doc generation bumps on every
    // `ApplyUsdOp` / source change).
    let sig: Vec<(String, u64)> = loaded
        .entries
        .iter()
        .map(|e| {
            let generation = e
                .doc_id_for_viewport()
                .and_then(|d| registry.host(d))
                .map(|h| h.document().generation())
                .unwrap_or(0);
            (e.id().to_string(), generation)
        })
        .collect();

    if *last_sig == sig {
        return;
    }
    *last_sig = sig;

    view.stages = loaded
        .entries
        .iter_mut()
        .filter_map(|e| e.build_row(&registry))
        .collect();
}

// ─────────────────────────────────────────────────────────────────────
// WorkspaceStage — one per writable / Untitled USD document
// ─────────────────────────────────────────────────────────────────────

/// A writable USD document the user is authoring — one
/// [`crate::ui::loaded_stages::LoadedStage`] per document, matching the Modelica WorkspaceClass
/// shape where `Untitled1.mo`, `MyController.mo`, etc. each appear as
/// siblings in the browser.
///
/// Reads source-of-truth from
/// [`UsdDocumentRegistry`]:
/// name + dirty state come from the live document. Stateless beyond
/// the doc id.
pub struct WorkspaceStage {
    doc_id: DocumentId,
    cached_id: String,
    /// Parsed-stage cache. Re-built only when the document generation
    /// advances — keeps the prim-tree walk allocation-free on the
    /// no-op render path (`AGENTS.md` §7.1).
    parsed: Option<ParsedStage>,
    /// Stashed parse error from the most recent failed re-parse.
    /// Cleared on the next successful parse. Surfaced as a label in
    /// the body so users see a malformed file instead of an empty
    /// tree.
    parse_error: Option<String>,
}

/// Cached parse of one document at a specific generation.
struct ParsedStage {
    /// Document generation the cache was built against.
    generation: u64,
    /// Parsed reader. `Arc` so future viewport / property-inspector
    /// consumers can share without re-parsing.
    reader: Arc<UsdData>,
}

impl WorkspaceStage {
    /// Build a workspace-stage entry pointing at the given document id.
    pub fn new(doc_id: DocumentId) -> Self {
        Self {
            cached_id: format!("workspace-usd:{}", doc_id.raw()),
            doc_id,
            parsed: None,
            parse_error: None,
        }
    }

    /// The document this entry surfaces.
    pub fn doc_id(&self) -> DocumentId {
        self.doc_id
    }

    /// Refresh the parsed-stage cache if the document generation has
    /// advanced past the cached one. No-op when caches match — this
    /// is the frame-discipline gate.
    fn ensure_parsed(&mut self, source: &str, generation: u64) {
        if self.parsed.as_ref().map(|p| p.generation) == Some(generation) {
            return;
        }
        match openusd::usda::parse(source) {
            Ok(data) => {
                self.parsed = Some(ParsedStage {
                    generation,
                    reader: Arc::new(data),
                });
                self.parse_error = None;
            }
            Err(e) => {
                self.parse_error = Some(format!("parse error: {}", e));
                // Drop any stale cache so we don't render outdated
                // prims while the user is fixing the source.
                self.parsed = None;
            }
        }
    }
}

impl LoadedStage for WorkspaceStage {
    fn id(&self) -> &str {
        &self.cached_id
    }

    fn writable(&self) -> bool {
        true
    }

    fn default_open(&self) -> bool {
        // Workspace items are what the user is actively editing —
        // expand by default so the prim hierarchy is one click away.
        true
    }

    fn doc_id_for_viewport(&self) -> Option<DocumentId> {
        Some(self.doc_id)
    }

    fn build_row(&mut self, registry: &UsdDocumentRegistry) -> Option<UsdStageRow> {
        // Snapshot source + generation + name; the host borrow ends
        // before we mutate `self` (parse cache) below.
        let (source, generation, name) = {
            let host = registry.host(self.doc_id)?;
            let doc = host.document();
            (
                doc.source().to_string(),
                doc.generation(),
                doc.origin().display_name(),
            )
        };

        self.ensure_parsed(&source, generation);

        Some(UsdStageRow {
            salt: self.cached_id.clone(),
            doc_id: Some(self.doc_id),
            name,
            writable: true,
            default_open: true,
            reader: self.parsed.as_ref().map(|p| p.reader.clone()),
            parse_error: self.parse_error.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DocumentId;
    use openusd::sdf;

    /// `unregister` matches by id and reports whether anything was
    /// removed — small-but-load-bearing because lifecycle observers
    /// rely on it for idempotency.
    const TINY_USDA: &str = "#usda 1.0\ndef Xform \"World\" {\n  def Sphere \"Ball\" { }\n}\n";

    /// ensure_parsed builds a cache on first call, reuses it on the
    /// same generation, and rebuilds when the generation advances.
    /// Exercises the §7.1 frame-discipline gate.
    #[test]
    fn ensure_parsed_caches_per_generation() {
        let mut stage = WorkspaceStage::new(DocumentId::new(1));
        assert!(stage.parsed.is_none());

        stage.ensure_parsed(TINY_USDA, 0);
        let first = stage.parsed.as_ref().expect("parsed").reader.clone();

        // Same generation → no re-parse, Arc identity preserved.
        stage.ensure_parsed(TINY_USDA, 0);
        let second = stage.parsed.as_ref().unwrap().reader.clone();
        assert!(Arc::ptr_eq(&first, &second));

        // Bumped generation → fresh parse, new Arc.
        stage.ensure_parsed(TINY_USDA, 1);
        let third = stage.parsed.as_ref().unwrap().reader.clone();
        assert!(!Arc::ptr_eq(&first, &third));
    }

    /// A malformed source surfaces as a `parse_error` and clears any
    /// stale cache.
    #[test]
    fn parse_error_is_recorded_and_cache_dropped() {
        let mut stage = WorkspaceStage::new(DocumentId::new(2));
        stage.ensure_parsed(TINY_USDA, 0);
        assert!(stage.parsed.is_some());

        stage.ensure_parsed("not a usda file at all {{{", 1);
        assert!(stage.parsed.is_none());
        assert!(stage.parse_error.is_some());
    }

    /// Parsed reader exposes the top-level prim under `/`. Walks the
    /// same path the `render_prim` recursion uses without needing
    /// egui plumbing.
    #[test]
    fn prim_children_walks_root_layer() {
        let mut stage = WorkspaceStage::new(DocumentId::new(3));
        stage.ensure_parsed(TINY_USDA, 0);
        let reader = stage.parsed.as_ref().unwrap().reader.clone();
        let root = sdf::path("/").unwrap();
        // TODO(usd-read-migration): switch to the generic UsdRead surface (`children`)
        // instead of the legacy `prim_children`, matching production (doc 21).
        let top = reader.prim_children(&root);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].name(), Some("World"));
        let nested = reader.prim_children(&top[0]);
        assert_eq!(nested.len(), 1);
        assert_eq!(nested[0].name(), Some("Ball"));
    }

    #[test]
    fn unregister_removes_matching_entry() {
        let mut loaded = LoadedUsdStages::default();
        loaded.register(Box::new(WorkspaceStage::new(DocumentId::new(7))));
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.unregister("workspace-usd:7"));
        assert!(loaded.is_empty());
        assert!(!loaded.unregister("workspace-usd:7"));
    }
}
