//! Modelica section of the Twin Browser.
//!
//! ## What it shows
//!
//! 1. Every Modelica document currently loaded in the
//!    [`crate::ui::state::ModelicaDocumentRegistry`] — drafts, duplicates from the
//!    Welcome examples, files opened in earlier sessions. This is the
//!    workspace's authoritative view of "what Modelica content does
//!    the user have right now."
//! 2. *(Future)* Files in the open Twin folder that aren't loaded yet
//!    — surfaced as a separate group so users can click to load.
//!
//! Each row is a Modelica class keyed by its **fully-qualified path**
//! (e.g. `"AnnotatedRocketStage.RocketStage"`). Click → emits
//! [`lunco_workbench::BrowserAction::OpenLoadedClass`] for in-memory docs, dispatched
//! into the existing drill-in machinery so the canvas tab opens
//! directly on the requested class.
//!
//! ## Single source of truth
//!
//! This panel **does not parse**. It reads
//! [`ModelicaDocument::syntax`](crate::document::ModelicaDocument::syntax)
//! — the lenient parse cache that the off-thread refresh in
//! [`crate::ui::ast_refresh`] keeps up to date — and derives the
//! class tree from it on each render. The browser sees exactly the
//! same parse the rest of the workbench sees; no panel-local cache
//! and no panel-local rumoca call.
//!
//! Building the `ClassEntry` tree from a `SyntaxCache` is sub-
//! millisecond on typical Modelica files (just walks the AST and
//! clones short strings), so we re-derive on every render rather
//! than maintain another cache layer.

use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::{BrowserAction, BrowserCtx, BrowserSection};
use rumoca_session::parsing::ClassType;

// `DrilledInClassNames` reads migrated to
// `crate::ui::panels::model_view::drilled_class_for_doc`.
use crate::ui::state::ModelicaDocumentRegistry;

/// One Modelica class entry rendered in the tree.
#[derive(Debug, Clone)]
struct ClassEntry {
    /// Short identifier (e.g. `"Engine"`).
    short_name: String,
    /// Fully-qualified path (e.g. `"AnnotatedRocketStage.Engine"`).
    qualified_path: String,
    /// Modelica class kind — drives the row's letter badge.
    kind: ClassType,
    /// Children — nested classes inside a package / model.
    children: Vec<ClassEntry>,
}

/// The Modelica Twin-Browser section. Stateless — every render
/// derives the class tree from
/// [`ModelicaDocument::syntax`](crate::document::ModelicaDocument::syntax),
/// which is kept up to date off-thread by [`crate::ui::ast_refresh`].
#[derive(Default)]
pub struct ModelicaSection;

impl BrowserSection for ModelicaSection {
    fn id(&self) -> &str {
        "lunco.modelica.classes"
    }

    fn title(&self) -> &str {
        "Modelica"
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        100
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_>) {
        // OMEdit-style flat list — system libraries on top, then
        // writable workspace documents. Both source-of-truth reads:
        //   * libraries come from `PackageTreeCache::roots` (the
        //     same tree the Package Browser panel renders);
        //   * workspace docs come from `ModelicaDocumentRegistry`
        //     filtered for writable / untitled origins.
        // No parallel `LoadedModelicaClasses` registry, no observer
        // wiring — what's in the cache + registry IS what we show.

        // ── System library roots ─────────────────────────────────
        // Pull `(id, name)` pairs first so we can re-borrow `world`
        // mutably inside `render_root_subtree` without overlapping
        // an immutable cache borrow.
        let library_rows: Vec<(String, String)> = {
            let cache = ctx
                .world
                .resource::<crate::ui::panels::package_browser::PackageTreeCache>();
            cache
                .roots
                .iter()
                .filter_map(|root| match root {
                    crate::ui::panels::package_browser::PackageNode::Category {
                        id,
                        name,
                        ..
                    } => Some((id.clone(), name.clone())),
                    _ => None,
                })
                .collect()
        };

        for (root_id, root_name) in &library_rows {
            // All libraries start collapsed; user expands the ones
            // they care about. Keeps the browser scannable on startup.
            let _ = root_id;
            let label = format!("🔒  {}", root_name);
            let resp = egui::CollapsingHeader::new(label)
                .id_salt(("twin.modelica.library", root_id))
                .default_open(false)
                .show(ui, |ui| {
                    crate::ui::panels::package_browser::render_root_subtree(
                        ctx.world, ui, root_id,
                    );
                });
            resp.header_response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
        }

        // ── Writable / untitled workspace documents ──────────────
        // For Untitled docs prefer the first non-package class name
        // over the origin slug (`Untitled-1`) — the class name is
        // the identity the user sees in the canvas / tab title, so
        // showing a different label in the browser is just confusing.
        // Falls back to the origin slug while no class exists yet
        // (mid-parse, empty draft).
        let workspace_docs: Vec<(DocumentId, String)> = {
            let registry = ctx.world.resource::<ModelicaDocumentRegistry>();
            registry
                .iter()
                .filter_map(|(doc_id, host)| {
                    let document = host.document();
                    let origin = document.origin();
                    if !(origin.is_writable() || origin.is_untitled()) {
                        return None;
                    }
                    // Doc-row label reflects the *container* (origin
                    // slug for Untitled drafts, filename for on-disk
                    // docs). The inner class name is rendered as the
                    // M-badge child row, so the two stay decoupled —
                    // renaming the doc row doesn't rewrite source.
                    let label = origin.display_name();
                    Some((doc_id, label))
                })
                .collect()
        };

        if library_rows.is_empty() && workspace_docs.is_empty() {
            ui.label(
                egui::RichText::new("No Modelica classes loaded.")
                    .weak()
                    .italics(),
            );
        }

        for (doc_id, doc_name) in workspace_docs {
            render_workspace_doc_row(ui, ctx, doc_id, &doc_name);
        }
    }
}

/// Inline-rename state for Twin Browser doc rows. `Some((doc, draft))`
/// → the row for `doc` renders a `TextEdit` instead of a header label;
/// `None` → all rows show their normal collapsing-header. Committed
/// on Enter or focus-loss, cancelled on Escape.
#[derive(bevy::prelude::Resource, Default, Debug)]
pub struct DocRenameState {
    pub editing: Option<(DocumentId, String)>,
    /// Set to `true` for one frame after a rename starts so the
    /// `TextEdit` can grab focus on the first paint; cleared once
    /// focus is delivered. Without this latch, calling
    /// `request_focus()` every frame would re-steal focus the moment
    /// the user clicks elsewhere, making it impossible to cancel or
    /// commit by clicking away.
    pub needs_focus: bool,
}

/// Renders one writable / Untitled workspace doc row. The header is a
/// `CollapsingHeader` by default; double-click the header label
/// switches to an inline `TextEdit` whose commit dispatches
/// `RenameModelicaClass` on the doc's default class (and that
/// command also updates Untitled origins, so the row label flips
/// alongside the class rename).
fn render_workspace_doc_row(
    ui: &mut egui::Ui,
    ctx: &mut BrowserCtx<'_>,
    doc_id: DocumentId,
    doc_name: &str,
) {
    let editing = ctx
        .world
        .get_resource::<DocRenameState>()
        .and_then(|s| s.editing.clone())
        .filter(|(d, _)| *d == doc_id);

    let mut start_rename: Option<String> = None;
    let mut commit_rename: Option<String> = None;
    let mut cancel_rename = false;
    let mut close_doc = false;
    let update_draft: Option<String> = None;

    if let Some((_, draft)) = editing {
        // Inline edit mode — replaces the CollapsingHeader header
        // with a TextEdit so the doc's child class tree disappears
        // for the moment (consistent with VS Code rename UX in the
        // file explorer).
        let mut buf = draft;
        ui.horizontal(|ui| {
            ui.label("📝");
            let resp = ui.add(
                egui::TextEdit::singleline(&mut buf).desired_width(180.0),
            );
            // One-shot focus grab: only on the first frame after the
            // rename began. Calling `request_focus()` every frame
            // re-steals focus and prevents click-away from working.
            if ctx
                .world
                .get_resource::<DocRenameState>()
                .map(|s| s.needs_focus)
                .unwrap_or(false)
            {
                resp.request_focus();
                if let Some(mut s) = ctx.world.get_resource_mut::<DocRenameState>() {
                    s.needs_focus = false;
                }
            }
            let enter = resp.lost_focus()
                && resp.ctx.input(|i| i.key_pressed(egui::Key::Enter));
            let escape = resp.ctx.input(|i| i.key_pressed(egui::Key::Escape));
            if enter || (resp.lost_focus() && !escape) {
                let trimmed = buf.trim().to_string();
                if trimmed.is_empty() {
                    cancel_rename = true;
                } else {
                    commit_rename = Some(trimmed);
                }
            } else if escape {
                cancel_rename = true;
            }
        });
    } else {
        // Manual CollapsingState so the header *label* gets its own
        // Response — `CollapsingHeader::show` returns a header
        // response whose click is consumed by the toggle, so
        // double-click on the bare API never fires reliably.
        let id = ui.make_persistent_id((
            "twin.modelica.workspace_doc",
            doc_id.raw(),
        ));
        let state =
            egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                id,
                true,
            );
        // Icon prefix: 📝 untitled draft, 📄 saved on disk. Read
        // the origin once before show_header so we don't re-borrow
        // the registry inside the closure.
        let icon: &'static str = ctx
            .world
            .get_resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc_id))
            .map(|h| {
                if h.document().origin().is_untitled() {
                    "📝"
                } else {
                    "📄"
                }
            })
            .unwrap_or("📄");
        let header = state.show_header(ui, |ui| {
            let resp = ui
                .add(
                    egui::Label::new(format!("{icon}  {doc_name}"))
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(
                    "Double-click (or F2 while focused) to rename. \
                     Untitled drafts → renames the top-level class. \
                     Saved files → renames the file on disk.",
                );
            if resp.double_clicked() {
                start_rename = Some(doc_name.to_string());
            }
            // F2 while the label has keyboard focus also starts a
            // rename — mirrors the VS Code / OMEdit shortcut.
            if resp.has_focus()
                && resp.ctx.input(|i| i.key_pressed(egui::Key::F2))
            {
                start_rename = Some(doc_name.to_string());
            }
            resp.context_menu(|ui| {
                if ui.button("✏ Rename").clicked() {
                    start_rename = Some(doc_name.to_string());
                    ui.close();
                }
                if ui.button("✕ Close").clicked() {
                    close_doc = true;
                    ui.close();
                }
            });
        });
        header.body(|ui| render_workspace_doc(ui, ctx, doc_id));
    }

    // Close the document — drops its tabs and (for an autosaved
    // wasm draft) its localStorage entry. Dispatched after the egui
    // closures release their borrow on `ctx`.
    if close_doc {
        ctx.actions.push(BrowserAction::CloseDoc { doc: doc_id });
    }

    // State transitions, priority: commit > cancel > start > update.
    if let Some(new_name) = commit_rename {
        // The doc-row rename is a *container* rename, not a class
        // rename. Untitled draft → update `DocumentOrigin::Untitled`
        // only; File-backed → rename the file on disk via
        // `RenameTwinEntry`. Source is not rewritten and no reparse
        // is triggered — the Modelica class inside keeps its name.
        // Users who want to rename the class itself click the inner
        // M-badge row, which still goes through `RenameModelicaClass`.
        enum RenameTarget {
            File { twin_root: String, relative_path: String },
            UntitledOrigin,
            None,
        }
        let target = {
            let registry = ctx.world.resource::<ModelicaDocumentRegistry>();
            let host = registry.host(doc_id);
            let origin = host.map(|h| h.document().origin().clone());
            match origin {
                Some(lunco_doc::DocumentOrigin::File { path, writable: true }) => {
                    let ws = ctx.world.resource::<lunco_workbench::WorkspaceResource>();
                    let twin_root = ws
                        .active_twin
                        .and_then(|id| ws.twin(id))
                        .map(|t| t.root.clone());
                    if let Some(root) = twin_root {
                        if let Ok(rel) = path.strip_prefix(&root) {
                            RenameTarget::File {
                                twin_root: root.to_string_lossy().into_owned(),
                                relative_path: rel.to_string_lossy().into_owned(),
                            }
                        } else {
                            RenameTarget::None
                        }
                    } else {
                        RenameTarget::None
                    }
                }
                Some(lunco_doc::DocumentOrigin::Untitled { .. }) => {
                    RenameTarget::UntitledOrigin
                }
                _ => RenameTarget::None,
            }
        };
        match target {
            RenameTarget::File { twin_root, relative_path } => {
                let new_file_name = {
                    use std::path::Path;
                    let typed = Path::new(&new_name);
                    if typed.extension().is_some() {
                        new_name.clone()
                    } else if let Some(ext) = Path::new(&relative_path)
                        .extension()
                        .and_then(|s| s.to_str())
                    {
                        format!("{new_name}.{ext}")
                    } else {
                        new_name.clone()
                    }
                };
                ctx.world
                    .commands()
                    .trigger(lunco_workbench::file_ops::RenameTwinEntry {
                        twin_root,
                        relative_path,
                        new_name: new_file_name,
                    });
            }
            RenameTarget::UntitledOrigin => {
                if !new_name.is_empty() {
                    if let Some(mut registry) =
                        ctx.world.get_resource_mut::<ModelicaDocumentRegistry>()
                    {
                        if let Some(host) = registry.host_mut(doc_id) {
                            host.document_mut().set_origin(
                                lunco_doc::DocumentOrigin::untitled(new_name.clone()),
                            );
                        }
                    }
                }
            }
            RenameTarget::None => {}
        }
        if let Some(mut s) = ctx.world.get_resource_mut::<DocRenameState>() {
            s.editing = None;
        }
    } else if cancel_rename {
        if let Some(mut s) = ctx.world.get_resource_mut::<DocRenameState>() {
            s.editing = None;
        }
    } else if let Some(initial) = start_rename {
        if let Some(mut s) = ctx.world.get_resource_mut::<DocRenameState>() {
            s.editing = Some((doc_id, initial));
            s.needs_focus = true;
        }
    } else if let Some(draft) = update_draft {
        if let Some(mut s) = ctx.world.get_resource_mut::<DocRenameState>() {
            s.editing = Some((doc_id, draft));
        }
    }
}

/// Render the class tree of one writable / Untitled workspace
/// document. Called by [`crate::ui::loaded_classes::WorkspaceClass`] —
/// the outer `CollapsingHeader` row carrying this doc's name has
/// already been drawn; we just paint the children inline.
///
/// Source-of-truth read of [`crate::ui::state::ModelicaDocumentRegistry`] via the doc's
/// [`crate::index::ModelicaIndex`]. Stateless; the registry's
/// off-thread refresh + per-op optimistic patches keep the Index current.
pub(crate) fn render_workspace_doc(
    ui: &mut egui::Ui,
    ctx: &mut BrowserCtx<'_>,
    doc_id: DocumentId,
) {
    let (classes, has_parse_errors) = match ctx
        .world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|reg| reg.host(doc_id))
        .map(|host| classes_from_index(host.document().index()))
    {
        Some(t) => t,
        None => {
            ui.label(
                egui::RichText::new("(document not in registry)")
                    .weak()
                    .italics(),
            );
            return;
        }
    };

    let theme = ctx
        .world
        .get_resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);

    let active_doc: Option<DocumentId> = ctx
        .world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    let active_qualified: Option<String> = active_doc.and_then(|d| {
        crate::ui::panels::model_view::drilled_class_for_doc(ctx.world, d)
    });

    // Collapse the redundant wrapper when the document holds a
    // single top-level class whose short name matches the outer
    // header (e.g. duplicated `AnnotatedRocketStageCopy.mo` whose
    // sole top class is `package AnnotatedRocketStageCopy`). Without
    // this, the browser shows the same name twice — once on the
    // workspace doc row, once on the package row immediately below.
    // We promote the wrapper's children to the top so the inner
    // classes (Airframe, Engine, FluidPort, …) sit directly under
    // the doc header.
    let doc_display_name: Option<String> = ctx
        .world
        .get_resource::<ModelicaDocumentRegistry>()
        .and_then(|reg| reg.host(doc_id))
        .map(|host| host.document().origin().display_name());
    let classes: Vec<ClassEntry> = if classes.len() == 1
        && doc_display_name
            .as_deref()
            .map(|n| n == classes[0].short_name)
            .unwrap_or(false)
        && !classes[0].children.is_empty()
    {
        classes.into_iter().next().unwrap().children
    } else {
        classes
    };

    if classes.is_empty() {
        // Distinguish empty-draft from broken-file. A blank
        // "(no classes yet)" row on a file the user just broke
        // looks identical to a healthy empty draft — the user
        // thinks their classes were deleted. Label the error case
        // explicitly.
        let (text, color) = if has_parse_errors {
            (
                "⚠ parse error".to_string(),
                egui::Color32::from_rgb(220, 160, 60),
            )
        } else {
            (
                "(no classes yet)".to_string(),
                ui.visuals().weak_text_color(),
            )
        };
        ui.label(
            egui::RichText::new(text)
                .color(color)
                .small()
                .italics(),
        );
        return;
    }
    for class in &classes {
        render_class_row(
            ui,
            class,
            doc_id,
            active_doc,
            active_qualified.as_deref(),
            &theme,
            ctx,
        );
    }
}


/// Build the same class tree from the per-doc Index. Reads only the
/// [`crate::index::ClassEntry`]s (no AST walk). Used by the live
/// renderer; `classes_from_syntax` is kept for the test fixtures
/// below until those migrate.
fn classes_from_index(index: &crate::index::ModelicaIndex) -> (Vec<ClassEntry>, bool) {
    use crate::index::ClassKind;
    fn map_kind(k: ClassKind) -> ClassType {
        match k {
            ClassKind::Model => ClassType::Model,
            ClassKind::Block => ClassType::Block,
            ClassKind::Connector => ClassType::Connector,
            ClassKind::Package => ClassType::Package,
            ClassKind::Function => ClassType::Function,
            ClassKind::Class => ClassType::Class,
            ClassKind::Type => ClassType::Type,
            ClassKind::Record => ClassType::Record,
            ClassKind::ExpandableConnector => ClassType::Connector,
            ClassKind::Operator => ClassType::Operator,
            ClassKind::OperatorRecord => ClassType::Record,
        }
    }
    fn build_subtree(
        index: &crate::index::ModelicaIndex,
        qualified: &str,
    ) -> Option<ClassEntry> {
        let entry = index.classes.get(qualified)?;
        let short = entry
            .name
            .rsplit('.')
            .next()
            .unwrap_or(&entry.name)
            .to_string();
        let mut children: Vec<ClassEntry> = entry
            .children
            .iter()
            .filter_map(|child_qual| build_subtree(index, child_qual))
            .collect();
        children.sort_by_key(|c| (browser_sort_group(c), c.short_name.to_lowercase()));
        Some(ClassEntry {
            short_name: short,
            qualified_path: entry.name.clone(),
            kind: map_kind(entry.kind),
            children,
        })
    }

    // Top-level classes: Index keys whose qualified name has no `.`
    let mut top: Vec<ClassEntry> = index
        .classes
        .keys()
        .filter(|k| !k.contains('.'))
        .filter_map(|k| build_subtree(index, k))
        .collect();
    top.sort_by_key(|c| (browser_sort_group(c), c.short_name.to_lowercase()));
    (top, index.has_errors)
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Test-only convenience: build a `ModelicaDocument` from `source`
/// and derive the class tree through the same `classes_from_index`
/// path the production renderer uses. Replaces the old
/// `SyntaxCache → classes_from_syntax` shortcut that was deleted in
/// the index-refactor.
#[cfg(test)]
fn parse_classes(source: &str) -> (Vec<ClassEntry>, bool) {
    use lunco_doc::{DocumentId, DocumentOrigin};
    let doc = crate::document::ModelicaDocument::with_origin(
        DocumentId::new(1),
        source.to_string(),
        DocumentOrigin::untitled("test"),
    );
    classes_from_index(doc.index())
}


/// Sort bucket for `ClassEntry`. Variant order = display order via
/// derived `Ord`, so adding a new bucket is a one-line edit.
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
enum BrowserSortGroup {
    UsersGuide,
    Examples,
    SubPackage,
    LeafModel,
    LeafBlock,
    LeafConnector,
    LeafRecord,
    LeafFunction,
    LeafType,
    LeafClass,
    LeafOperator,
}

fn browser_sort_group(c: &ClassEntry) -> BrowserSortGroup {
    match c.short_name.as_str() {
        "UsersGuide" => BrowserSortGroup::UsersGuide,
        "Examples" => BrowserSortGroup::Examples,
        _ => match c.kind {
            ClassType::Package => BrowserSortGroup::SubPackage,
            ClassType::Model => BrowserSortGroup::LeafModel,
            ClassType::Block => BrowserSortGroup::LeafBlock,
            ClassType::Connector => BrowserSortGroup::LeafConnector,
            ClassType::Record => BrowserSortGroup::LeafRecord,
            ClassType::Function => BrowserSortGroup::LeafFunction,
            ClassType::Type => BrowserSortGroup::LeafType,
            ClassType::Class => BrowserSortGroup::LeafClass,
            ClassType::Operator => BrowserSortGroup::LeafOperator,
        },
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Paint one class row. Recurses into children when the row is
/// expanded. Click → [`lunco_workbench::BrowserAction::OpenLoadedClass`] keyed by the
/// owning document's id.
///
/// `active_doc`/`active_qualified` describe what the foreground tab
/// is currently editing; the matching row paints "selected" so users
/// see at a glance which class they're on.
fn render_class_row(
    ui: &mut egui::Ui,
    class: &ClassEntry,
    doc_id: DocumentId,
    active_doc: Option<DocumentId>,
    active_qualified: Option<&str>,
    theme: &lunco_theme::Theme,
    ctx: &mut BrowserCtx<'_>,
) {
    use crate::ui::theme::ModelicaThemeExt;
    let badge = type_badge(&class.kind, theme);
    let is_active = Some(doc_id) == active_doc
        && active_qualified == Some(class.qualified_path.as_str());

    if class.children.is_empty() {
        let resp = ui.horizontal(|ui| {
            paint_badge(ui, badge, theme);
            let label = if is_active {
                egui::RichText::new(&class.short_name).strong()
            } else {
                egui::RichText::new(&class.short_name)
            };
            ui.add(egui::Label::new(label).selectable(false).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
        }).inner;
        // Explicit highlight band — `selectable_label`'s default
        // selected chrome blends into the panel background under a
        // dark egui theme, leaving the user with no visual cue. We
        // paint the same translucent yellow the package-browser
        // tree's `render_node` uses so the active row matches across
        // both views (Twin sidebar Modelica section + standalone
        // Package Browser).
        if is_active {
            ui.painter().rect_filled(
                resp.rect,
                2.0,
                egui::Color32::from_rgba_unmultiplied(80, 80, 0, 40),
            );
        }
        if resp.clicked() {
            ctx.actions.push(BrowserAction::OpenLoadedClass {
                doc_id: doc_id.raw(),
                qualified_path: class.qualified_path.clone(),
            });
        }
        {
            // Hover stays lightweight — short name + qualified path
            // only. The docstring lives in the Docs view, not on
            // hover, so we don't duplicate content one click away.
            let muted = theme.text_muted();
            resp.on_hover_ui(|ui| {
                ui.strong(&class.short_name);
                ui.label(
                    egui::RichText::new(&class.qualified_path)
                        .small()
                        .color(muted),
                );
            });
        }
    } else {
        let mut header_text =
            egui::RichText::new(format!("{} {}", badge.letter, class.short_name));
        if is_active {
            header_text = header_text.strong();
        }
        let header = egui::CollapsingHeader::new(header_text)
            .id_salt(("modelica_class", &class.qualified_path))
            .default_open(true);
        let resp = header.show(ui, |ui| {
            for child in &class.children {
                render_class_row(
                    ui,
                    child,
                    doc_id,
                    active_doc,
                    active_qualified,
                    theme,
                    ctx,
                );
            }
        });
        let qualified = class.qualified_path.clone();
        let short = class.short_name.clone();
        let muted = theme.text_muted();
        let header_resp = resp.header_response.clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        header_resp.on_hover_ui(move |ui| {
            ui.strong(&short);
            ui.label(
                egui::RichText::new(&qualified)
                    .small()
                    .color(muted),
            );
        });
        if resp.header_response.clicked() {
            ctx.actions.push(BrowserAction::OpenLoadedClass {
                doc_id: doc_id.raw(),
                qualified_path: class.qualified_path.clone(),
            });
        }
    }
}

/// Visual descriptor for a class-kind badge.
pub(crate) struct Badge {
    pub letter: &'static str,
    pub bg: egui::Color32,
}

pub(crate) fn type_badge(kind: &ClassType, theme: &lunco_theme::Theme) -> Badge {
    use crate::ui::theme::ModelicaThemeExt;
    let letter = match kind {
        ClassType::Model => "M",
        ClassType::Block => "B",
        ClassType::Class => "C",
        ClassType::Connector => "X",
        ClassType::Record => "R",
        ClassType::Type => "T",
        ClassType::Package => "P",
        ClassType::Function => "F",
        ClassType::Operator => "O",
    };
    Badge {
        letter,
        bg: theme.class_badge_bg(kind),
    }
}

/// Badge mapping keyed by our typed [`crate::index::ClassKind`].
/// Translates the workbench enum to rumoca's `ClassType` (the
/// shape `type_badge` expects) at the one boundary instead of
/// every consumer rolling its own string match.
pub(crate) fn type_badge_for_kind(
    kind: crate::index::ClassKind,
    theme: &lunco_theme::Theme,
) -> Badge {
    use crate::index::ClassKind;
    let ct = match kind {
        ClassKind::Model => ClassType::Model,
        ClassKind::Block => ClassType::Block,
        // Expandable connectors share the connector badge —
        // the dashed-border distinction lives in the canvas
        // visual, not the tree icon.
        ClassKind::Connector | ClassKind::ExpandableConnector => ClassType::Connector,
        ClassKind::Record | ClassKind::OperatorRecord => ClassType::Record,
        ClassKind::Type => ClassType::Type,
        ClassKind::Package => ClassType::Package,
        ClassKind::Function => ClassType::Function,
        ClassKind::Operator => ClassType::Operator,
        ClassKind::Class => ClassType::Class,
    };
    type_badge(&ct, theme)
}

pub(crate) fn paint_badge(ui: &mut egui::Ui, badge: Badge, theme: &lunco_theme::Theme) {
    use crate::ui::theme::ModelicaThemeExt;
    ui.add(
        egui::Label::new(
            egui::RichText::new(badge.letter)
                .monospace()
                .small()
                .background_color(badge.bg)
                .color(theme.class_badge_fg()),
        )
        .selectable(false),
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_top_level_models() {
        let src = r#"
model A end A;
model B end B;
"#;
        let (cs, errors) = parse_classes(src);
        assert!(!errors);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].short_name, "A");
        assert_eq!(cs[0].qualified_path, "A");
        assert!(matches!(cs[0].kind, ClassType::Model));
        assert_eq!(cs[1].short_name, "B");
    }

    #[test]
    fn parses_nested_classes_with_qualified_paths() {
        let src = r#"
package P
  model Inner end Inner;
  model Other "x" end Other;
end P;
"#;
        let (cs, errors) = parse_classes(src);
        assert!(!errors);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].short_name, "P");
        assert!(matches!(cs[0].kind, ClassType::Package));
        assert_eq!(cs[0].children.len(), 2);
        assert_eq!(cs[0].children[0].qualified_path, "P.Inner");
        assert_eq!(cs[0].children[1].qualified_path, "P.Other");
    }

    #[test]
    fn empty_source_returns_empty() {
        let (cs, errors) = parse_classes("");
        assert!(cs.is_empty());
        assert!(!errors);
    }

    #[test]
    fn broken_sibling_class_does_not_wipe_the_others() {
        // Primary regression guard for the "classes disappear from
        // browser when file invalid" bug: a syntax error in the last
        // class must not remove the preceding healthy ones from the
        // tree. Uses rumoca's error recovery via `parse_to_syntax`.
        let src = r#"
model Good1 end Good1;
model Good2 end Good2;
model Broken
    Real x =   // missing RHS, broken on purpose
end Broken;
"#;
        let (cs, errors) = parse_classes(src);
        assert!(errors, "parse should report errors on the broken class");
        let names: Vec<&str> = cs.iter().map(|c| c.short_name.as_str()).collect();
        assert!(
            names.contains(&"Good1") && names.contains(&"Good2"),
            "healthy sibling classes must survive recovery, got {names:?}"
        );
    }

    #[test]
    fn totally_broken_file_signals_error_even_when_empty() {
        // Second half of the bug fix: when recovery yields zero
        // classes we must still tell the UI it was a parse error so
        // the browser can distinguish "empty draft" from "broken
        // file" in its empty-state label.
        let (_cs, errors) = parse_classes("model ");
        assert!(errors);
    }

    #[test]
    fn class_kind_variants_round_trip() {
        let src = r#"
model M end M;
block B end B;
connector C end C;
record R end R;
package P end P;
function F end F;
"#;
        let (cs, _errors) = parse_classes(src);
        let kinds: Vec<&ClassType> = cs.iter().map(|c| &c.kind).collect();
        // Don't `use ClassType::*` — `Function` collides with
        // `bevy::reflect::Function` re-exported through other paths.
        assert!(matches!(
            kinds.as_slice(),
            [
                ClassType::Model,
                ClassType::Block,
                ClassType::Connector,
                ClassType::Record,
                ClassType::Package,
                ClassType::Function,
            ]
        ));
    }

    #[test]
    fn fixture_file_parses() {
        let src = include_str!("../../../../assets/models/AnnotatedRocketStage.mo");
        let (cs, _errors) = parse_classes(src);
        // Top level: one package.
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].short_name, "AnnotatedRocketStage");
        assert!(matches!(cs[0].kind, ClassType::Package));
        // Children: RocketStage + Engine + Tank + Gimbal.
        let child_names: Vec<&str> = cs[0]
            .children
            .iter()
            .map(|c| c.short_name.as_str())
            .collect();
        for expected in ["RocketStage", "Engine", "Tank", "Gimbal"] {
            assert!(
                child_names.contains(&expected),
                "missing {expected} (have {child_names:?})"
            );
        }
        // Qualified path correctness.
        assert!(cs[0]
            .children
            .iter()
            .any(|c| c.qualified_path == "AnnotatedRocketStage.Engine"));
    }
}

