//! Loaded USD stages — the live set of stage roots the user has
//! access to in this session.
//!
//! Mirrors `lunco_modelica::ui::loaded_classes::LoadedModelicaClasses`:
//! a flat registry of [`crate::ui::loaded_stages::LoadedStage`] entries surfaced as siblings in
//! the Twin browser's Models scope, regardless of where they came
//! from. Workspace docs, future bundled stages, future Twin externals
//! — all show up the same way.
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
use bevy_egui::egui;
use lunco_doc::{Document, DocumentId};
use lunco_workbench::BrowserCtx;
use openusd::sdf;
use openusd::usda::TextReader;

use crate::commands::ApplyUsdOp;
use crate::document::UsdOp;
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

    /// Display name shown as the top-level row. Takes `&BrowserCtx`
    /// for dynamic naming — workspace stages show their current
    /// `Untitled-N.usda` or file-stem name; future system stages
    /// return a constant.
    fn name(&self, ctx: &BrowserCtx<'_>) -> String;

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

    /// Paint the stage's prim tree inline at the caller's egui cursor —
    /// the caller has already drawn a `CollapsingHeader` for this entry.
    /// Phase 3 paints a placeholder; Phase 4 walks the composed prim
    /// hierarchy from `UsdComposer` output.
    fn render_children(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_>);

    /// If this entry corresponds to an open document, return its id
    /// so the browser can offer "show in viewport" affordances. The
    /// default is `None` for non-document entries (system libraries
    /// etc.); [`WorkspaceStage`] overrides.
    fn doc_id_for_viewport(&self) -> Option<DocumentId> {
        None
    }
}

/// Live registry of [`crate::ui::loaded_stages::LoadedStage`] entries. Iterated by the
/// [`UsdSceneSection`](crate::ui::browser_section::UsdSceneSection)
/// each frame; mutated by the lifecycle observers in
/// [`UsdUiPlugin`](crate::ui::UsdUiPlugin).
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
    reader: Arc<TextReader>,
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
        let mut parser = openusd::usda::parser::Parser::new(source);
        match parser.parse() {
            Ok(data) => {
                self.parsed = Some(ParsedStage {
                    generation,
                    reader: Arc::new(TextReader::from_data(data)),
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

    fn name(&self, ctx: &BrowserCtx<'_>) -> String {
        ctx.world
            .get_resource::<UsdDocumentRegistry>()
            .and_then(|reg| reg.host(self.doc_id))
            .map(|host| host.document().origin().display_name())
            .unwrap_or_else(|| "(closed)".to_string())
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

    fn render_children(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_>) {
        // Snapshot source + generation so subsequent UI calls don't
        // hold a registry borrow across the &mut World lifetime.
        let snapshot = ctx
            .world
            .get_resource::<UsdDocumentRegistry>()
            .and_then(|reg| reg.host(self.doc_id))
            .map(|host| {
                let doc = host.document();
                (doc.source().to_string(), doc.generation())
            });
        let Some((source, generation)) = snapshot else {
            ui.label(
                egui::RichText::new("(document closed)")
                    .weak()
                    .italics(),
            );
            return;
        };

        self.ensure_parsed(&source, generation);

        let error_color = ctx
            .world
            .get_resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.error)
            .unwrap_or(egui::Color32::LIGHT_RED);

        if let Some(err) = &self.parse_error {
            ui.colored_label(error_color, err);
            return;
        }
        let Some(parsed) = &self.parsed else {
            ui.label(egui::RichText::new("(no parse)").weak().italics());
            return;
        };

        // Collect ops emitted by toolbar buttons during render. We
        // can't trigger ApplyUsdOp inside render because dispatching
        // would require &mut World mid-egui-paint; instead we batch
        // and fire after the borrow ends.
        let mut pending_ops: Vec<UsdOp> = Vec::new();

        let root = match sdf::path("/") {
            Ok(p) => p,
            Err(e) => {
                ui.colored_label(error_color, format!("root path: {e}"));
                return;
            }
        };
        // Collapse a redundant single-root-prim wrapper whose name
        // matches the doc filename (case-insensitive). e.g. a stage
        // `artemis_2.usda` with a single `def Xform "Artemis2"` is
        // surfaced as `artemis_2 → Orion` instead of the confusing
        // `artemis_2 → Artemis2 (Xform) → Orion`. Single-root prims
        // with no children are kept (they ARE the content).
        let mut top_paths: Vec<sdf::Path> = parsed.reader.prim_children(&root);
        if top_paths.len() == 1 {
            let only = &top_paths[0];
            let grand = parsed.reader.prim_children(only);
            if !grand.is_empty() {
                top_paths = grand;
            }
        }

        let mut clicked_prim = false;

        if top_paths.is_empty() {
            ui.label(
                egui::RichText::new("(no prims)").weak().italics(),
            );
        } else {
            for path in top_paths {
                render_prim(
                    ui,
                    &parsed.reader,
                    &path,
                    &self.cached_id,
                    &mut pending_ops,
                    &mut clicked_prim,
                );
            }
        }

        // Now that egui's done drawing, fire the queued ops via the
        // typed command bus so undo/redo and change notification go
        // through the canonical path.
        if !pending_ops.is_empty() {
            let doc_id = self.doc_id;
            for op in pending_ops {
                ctx.world.commands().trigger(ApplyUsdOp { doc: doc_id, op });
            }
        }

        // Clicking any internal prim row retargets the shared USD
        // viewport at this doc. Same route as clicking the top-level
        // stage row in browser_section.rs.
        if clicked_prim {
            let doc_id = self.doc_id;
            ctx.world
                .commands()
                .trigger(crate::ui::viewport::SetActiveUsdViewport { doc: doc_id });
            ctx.world.commands().trigger(lunco_workbench::FocusPanel {
                id: crate::ui::viewport::USD_VIEWPORT_PANEL_ID.0.to_string(),
            });
        }
    }
}

/// Recursive prim-tree walker. One `CollapsingHeader` per prim;
/// children fetched via [`TextReader::prim_children`]. Per-prim
/// toolbar buttons append [`UsdOp`]s into `pending_ops` instead of
/// triggering directly, so dispatch happens once egui is done
/// painting.
///
/// Composition arcs (sublayers, references, payloads) are **not**
/// flattened — referenced prims show up only after a future
/// `UsdComposer` integration. Today the walk reflects the raw root
/// layer, which is the source-of-truth most edits target.
fn render_prim(
    ui: &mut egui::Ui,
    reader: &TextReader,
    path: &sdf::Path,
    salt: &str,
    pending_ops: &mut Vec<UsdOp>,
    clicked: &mut bool,
) {
    let name = path.name().unwrap_or("(root)").to_string();
    let type_name = prim_type_name(reader, path);
    let label = match &type_name {
        Some(ty) => format!("{} ({})", name, ty),
        None => name,
    };
    let children = reader.prim_children(path);
    let header_id = ui.make_persistent_id((salt, path.to_string()));

    let row = |ui: &mut egui::Ui, clicked: &mut bool| {
        let resp = ui
            .add(egui::Label::new(&label).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            *clicked = true;
        }
    };

    if children.is_empty() {
        ui.indent(header_id, |ui| {
            row(ui, clicked);
        });
    } else {
        // Clicking the label both focuses the prim in the viewport
        // *and* folds/unfolds the row — same as clicking the triangle.
        // The click flag goes through a local so the header closure
        // doesn't fight the body closure over `clicked`.
        let mut row_clicked = false;
        lunco_ui::helpers::collapsing_row(
            ui,
            header_id,
            false,
            |ui| {
                let resp = ui
                    .add(egui::Label::new(&label).sense(egui::Sense::click()))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                row_clicked = resp.clicked();
                row_clicked
            },
            |ui| {
                for child in children {
                    render_prim(ui, reader, &child, salt, pending_ops, clicked);
                }
            },
        );
        if row_clicked {
            *clicked = true;
        }
    }

    let _ = pending_ops;
}

/// Read the `typeName` field on a prim spec (e.g. `"Xform"`,
/// `"Mesh"`, `"Camera"`). Returns `None` for the pseudo-root or for
/// prims authored without an explicit type.
fn prim_type_name(reader: &TextReader, path: &sdf::Path) -> Option<String> {
    use openusd::sdf::Value;
    for (p, spec) in reader.iter() {
        if p == path {
            if let Some(Value::Token(t) | Value::String(t)) = spec.fields.get("typeName") {
                return Some(t.clone());
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_doc::DocumentId;

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
