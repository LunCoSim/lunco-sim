//! Modelica section of the Twin Browser.
//!
//! ## What it shows
//!
//! 1. Every Modelica document currently loaded in the
//!    [`crate::state::ModelicaDocumentRegistry`] — drafts, duplicates from the
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
use rumoca_compile::parsing::ClassType;

// `DrilledInClassNames` reads migrated to
// `crate::sim_default::drilled_class_for_doc`.
use crate::state::ModelicaDocumentRegistry;

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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        // OMEdit-style flat list — system libraries on top, then
        // writable workspace documents. Both source-of-truth reads:
        //   * libraries come from `PackageTreeCache::roots` (the
        //     same tree the Package Browser panel renders);
        //   * workspace docs come from `ModelicaDocumentRegistry`
        //     filtered for writable / untitled origins.
        // The cache plus document registry are the complete read sources for
        // this section; there is no separate UI-owned class list.

        // ── System library roots ─────────────────────────────────
        // Pull `(id, name)` pairs first so we can re-borrow `world`
        // mutably inside `render_root_subtree` without overlapping
        // an immutable cache borrow.
        let library_rows: Vec<(String, String)> = ctx
            .resource::<crate::package_tree::PackageTreeCache>()
            .map(|cache| {
                cache
                    .roots
                    .iter()
                    .filter_map(|root| match root {
                        crate::package_tree::PackageNode::Category { id, name, .. } => {
                            Some((id.clone(), name.clone()))
                        }
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        for (root_id, root_name) in &library_rows {
            // All libraries start collapsed; user expands the ones
            // they care about. Keeps the browser scannable on startup.
            let _ = root_id;
            let label = format!("[read-only]  {}", root_name);
            let resp = egui::CollapsingHeader::new(label)
                .id_salt(("twin.modelica.library", root_id))
                .default_open(false)
                .show(ui, |ui| {
                    crate::ui::panels::package_browser::render_root_subtree(ui, ctx, root_id);
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
        let workspace_docs: Vec<(DocumentId, String)> = ctx
            .resource::<ModelicaDocumentRegistry>()
            .map(|registry| {
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
                        let label: String = origin.display_name();
                        Some((doc_id, label))
                    })
                    .collect()
            })
            .unwrap_or_default();

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

        // Generated USD-owned islands are ordinary read-only Modelica documents
        // linked to their scene-network entity. They use the same class/view
        // path as every other model — source editor, diagram, and telemetry —
        // rather than opening in the generic text viewer.
        let generated = ctx
            .resource::<crate::state::GeneratedModelicaSources>()
            .map(|sources| sources.entries.clone())
            .unwrap_or_default();
        if !generated.is_empty() {
            let theme = ctx
                .resource::<lunco_theme::Theme>()
                .cloned()
                .unwrap_or_else(lunco_theme::Theme::dark);
            ui.separator();
            let header = egui::CollapsingHeader::new("⚡ Generated scene networks")
                .id_salt("twin.modelica.generated")
                .default_open(false);
            header.show(ui, |ui| {
                for entry in generated {
                    render_generated_network_row(ui, ctx, &theme, &entry);
                }
            });
        }
    }
}

/// Render one generated document as a useful model entry instead of exposing
/// its compiler URI as the primary label. The row keeps the source URI in a
/// tooltip for diagnostics while the visible metadata explains the topology
/// the user will find after opening it.
fn render_generated_network_row(
    ui: &mut egui::Ui,
    ctx: &mut BrowserCtx<'_, '_>,
    theme: &lunco_theme::Theme,
    entry: &crate::state::GeneratedModelicaSourceEntry,
) {
    let display_name = generated_network_display_name(&entry.network_root);
    let member_count = entry
        .units
        .iter()
        .map(|unit| unit.members.len())
        .sum::<usize>();
    let topology_count = if entry.units.is_empty() && !entry.component_paths.is_empty() {
        format!(
            "{} composed USD component{}",
            entry.component_paths.len(),
            plural_suffix(entry.component_paths.len())
        )
    } else {
        format!("{} member{}", member_count, plural_suffix(member_count))
    };
    let open_class = generated_network_open_class(entry);
    let active_doc = ctx
        .resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|workspace| workspace.active_document);
    // The row represents the generated document, not only the preferred first
    // class. Keep it active while the user is viewing the root, a unit, or a
    // dependency class drilled from that same read-only document.
    let is_open = active_doc == Some(entry.document);
    let open_hint = format!(
        "Open generated Modelica\nClass: {}\nDocument: {}\nSource is read-only and rebuilt from the composed USD network.",
        open_class, entry.uri
    );

    ui.push_id(&entry.uri, |ui| {
        ui.horizontal_wrapped(|ui| {
            let can_open = !entry.document.is_unassigned() && !entry.source.is_empty();
            let response = ui
                .selectable_label(
                    is_open,
                    egui::RichText::new(display_name)
                        .strong()
                        .color(if can_open {
                            theme.schematic.text_heading
                        } else {
                            theme.schematic.text_muted
                        }),
                )
                .on_hover_text(if can_open {
                    open_hint
                } else if entry.error.is_some() {
                    "Generated source is unavailable because synthesis failed.".to_string()
                } else {
                    "Generated source is still being projected.".to_string()
                });
            if can_open && response.clicked() {
                ctx.actions.push(BrowserAction::OpenLoadedClass {
                    doc_id: entry.document.raw(),
                    qualified_path: open_class.clone(),
                });
            }
            ui.colored_label(theme.schematic.class_model_badge, "GENERATED");
        });

        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new(&entry.network_root)
                    .small()
                    .color(theme.schematic.text_muted),
            );
            ui.label(
                egui::RichText::new(format!(
                    "class {}",
                    generated_class_display_name(&entry.model_name)
                ))
                .small()
                .color(theme.schematic.text_muted),
            )
            .on_hover_text(format!("Modelica class: {}", entry.model_name));
            ui.label(
                egui::RichText::new(format!(
                    "{} unit{} · {}",
                    entry.units.len(),
                    plural_suffix(entry.units.len()),
                    topology_count,
                ))
                .small()
                .color(theme.schematic.text_muted),
            );
            ui.label(
                egui::RichText::new(format!(
                    "interface {} in / {} out",
                    entry.boundary_inputs.len(),
                    entry.boundary_outputs.len(),
                ))
                .small()
                .color(theme.schematic.text_muted),
            );
            if !entry.member_output_aliases.is_empty() {
                ui.label(
                    egui::RichText::new(format!("telemetry {}", entry.member_output_aliases.len()))
                        .small()
                        .color(theme.schematic.text_muted),
                );
            }
        });

        if let Some(error) = &entry.error {
            ui.colored_label(theme.tokens.error, format!("Projection error: {error}"));
        } else if entry.source.is_empty() {
            ui.colored_label(theme.tokens.warning, "Projection unavailable");
        }

        egui::CollapsingHeader::new("Inspect topology and interface")
            .id_salt(("generated-details", &entry.uri))
            .default_open(false)
            .show(ui, |ui| {
                let inputs = if entry.boundary_inputs.is_empty() {
                    "none".to_string()
                } else {
                    entry.boundary_inputs.join(", ")
                };
                let outputs = if entry.boundary_outputs.is_empty() {
                    "none".to_string()
                } else {
                    entry.boundary_outputs.join(", ")
                };
                ui.label(format!("Inputs: {inputs}"));
                ui.label(format!("Outputs: {outputs}"));
                if !entry.source_roots.is_empty() {
                    ui.label(format!("Source roots: {}", entry.source_roots.join(", ")));
                }
                if entry.units.is_empty() && !entry.component_paths.is_empty() {
                    ui.label("Composed USD components:");
                    for path in &entry.component_paths {
                        ui.label(path);
                    }
                }
                for unit in &entry.units {
                    let unit_label = generated_unit_display_name(unit);
                    ui.collapsing(
                        format!("{} · {} member(s)", unit_label, unit.members.len()),
                        |ui| {
                            ui.label(
                                egui::RichText::new(format!("Modelica class: {}", unit.name))
                                    .small()
                                    .color(theme.schematic.text_muted),
                            );
                            for member in &unit.members {
                                if let Some((_, asset, class)) =
                                    entry.members.iter().find(|(path, _, _)| path == member)
                                {
                                    ui.label(generated_member_display_name(member, class))
                                        .on_hover_text(format!(
                                            "USD: {member}\nSource asset: {asset}"
                                        ));
                                } else {
                                    ui.label(generated_path_leaf(member)).on_hover_text(member);
                                }
                            }
                        },
                    );
                }
                if !entry.member_output_aliases.is_empty() {
                    ui.collapsing(
                        format!("Promoted telemetry ({})", entry.member_output_aliases.len()),
                        |ui| {
                            ui.label(
                                egui::RichText::new("Operator-facing member values")
                                    .small()
                                    .color(theme.schematic.text_muted),
                            );
                            for (member, output, alias) in &entry.member_output_aliases {
                                ui.label(generated_member_output_display_name(member, output))
                                    .on_hover_text(format!(
                                        "Generated alias: {alias}\nUSD: {member}.outputs:{output}"
                                    ));
                            }
                        },
                    );
                }
            });
    });
}

fn generated_network_display_name(network_root: &str) -> String {
    network_root
        .trim_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| format!("{segment} network"))
        .unwrap_or_else(|| "Generated network".to_string())
}

fn generated_path_leaf(path: &str) -> &str {
    path.trim_matches('/')
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
}

fn generated_class_display_name(class_name: &str) -> String {
    let class_name = class_name.strip_suffix("_System").unwrap_or(class_name);
    class_name
        .split("_x2f_")
        .map(|segment| segment.replace("__", "_"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn generated_unit_display_name(unit: &crate::state::GeneratedModelicaUnit) -> String {
    unit.members
        .first()
        .map(|member| format!("{} unit", generated_path_leaf(member)))
        .unwrap_or_else(|| generated_class_display_name(&unit.name))
}

fn generated_member_display_name(member: &str, class: &str) -> String {
    let class_leaf = class.rsplit('.').next().unwrap_or(class);
    format!("{} · {}", generated_path_leaf(member), class_leaf)
}

fn generated_member_output_display_name(member: &str, output: &str) -> String {
    format!("{}.{}", generated_path_leaf(member), output)
}

/// Select the useful first view for a generated network.
///
/// The synthesized root is still the authoritative Modelica wrapper and is
/// intentionally kept in the document. A single-unit network, however, has
/// no useful topology at the wrapper level: its diagram is one legitimate
/// unit instance. Opening the unit class here lets the normal Modelica canvas
/// show the actual composed members while preserving the root for source,
/// interface, and multi-unit navigation.
fn generated_network_open_class(entry: &crate::state::GeneratedModelicaSourceEntry) -> String {
    if entry.units.len() == 1 {
        entry.units[0].name.clone()
    } else {
        entry.model_name.clone()
    }
}

fn plural_suffix(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
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
    ctx: &mut BrowserCtx<'_, '_>,
    doc_id: DocumentId,
    doc_name: &str,
) {
    let editing = ctx
        .resource::<DocRenameState>()
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
            ui.label("Draft");
            let resp = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(180.0));
            // One-shot focus grab: only on the first frame after the
            // rename began. Calling `request_focus()` every frame
            // re-steals focus and prevents click-away from working.
            if ctx
                .resource::<DocRenameState>()
                .map(|s| s.needs_focus)
                .unwrap_or(false)
            {
                resp.request_focus();
                let _ = ctx.resource_scope::<DocRenameState, _>(|_, state| {
                    state.needs_focus = false;
                });
            }
            let enter = resp.lost_focus() && resp.ctx.input(|i| i.key_pressed(egui::Key::Enter));
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
        let id = ui.make_persistent_id(("twin.modelica.workspace_doc", doc_id.raw()));
        // Icon prefix: 📝 untitled draft, 📄 saved on disk. Read
        // the origin once before the header so we don't re-borrow
        // the registry inside the closure.
        let icon: &'static str = ctx
            .resource::<ModelicaDocumentRegistry>()
            .and_then(|r| r.host(doc_id))
            .map(|h| {
                if h.document().origin().is_untitled() {
                    "Draft"
                } else {
                    "File"
                }
            })
            .unwrap_or("File");
        lunco_ui::helpers::collapsing_row(
            ui,
            id,
            true,
            |ui| {
                let resp = ui
                    .add(
                        egui::Label::new(format!("{icon}  {doc_name}")).sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text(
                        "Click to expand/collapse. \
                         Double-click (or F2 while focused) to rename. \
                         Untitled drafts → renames the top-level class. \
                         Saved files → renames the file on disk.",
                    );
                if resp.double_clicked() {
                    start_rename = Some(doc_name.to_string());
                }
                // F2 while the label has keyboard focus also starts a
                // rename — mirrors the VS Code / OMEdit shortcut.
                if resp.has_focus() && resp.ctx.input(|i| i.key_pressed(egui::Key::F2)) {
                    start_rename = Some(doc_name.to_string());
                }
                resp.context_menu(|ui| {
                    if lunco_workbench::icon_text_button(
                        ui,
                        lunco_workbench::UiIcon::Edit,
                        "Rename",
                        "Rename this document",
                    )
                    .clicked()
                    {
                        start_rename = Some(doc_name.to_string());
                        ui.close();
                    }
                    if lunco_workbench::icon_button(ui, lunco_workbench::UiIcon::Close, "Close")
                        .clicked()
                    {
                        close_doc = true;
                        ui.close();
                    }
                });
                // Single click on the label folds/unfolds the row;
                // double-click is reserved for rename.
                resp.clicked() && !resp.double_clicked()
            },
            |ui| render_workspace_doc(ui, ctx, doc_id),
        );
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
            File {
                twin_root: String,
                relative_path: String,
            },
            UntitledOrigin,
            None,
        }
        let target = {
            let origin = ctx
                .resource::<ModelicaDocumentRegistry>()
                .and_then(|r| r.host(doc_id))
                .map(|h| h.document().origin().clone());
            match origin {
                Some(lunco_doc::DocumentOrigin::File {
                    path,
                    writable: true,
                }) => {
                    let twin_root = ctx
                        .resource::<lunco_workspace::WorkspaceResource>()
                        .and_then(|ws| {
                            ws.active_twin
                                .and_then(|id| ws.twin(id))
                                .map(|t| t.root.clone())
                        });
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
                Some(lunco_doc::DocumentOrigin::Untitled { .. }) => RenameTarget::UntitledOrigin,
                _ => RenameTarget::None,
            }
        };
        match target {
            RenameTarget::File {
                twin_root,
                relative_path,
            } => {
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
                ctx.trigger(lunco_workbench::file_ops::RenameTwinEntry {
                    twin_root,
                    relative_path,
                    new_name: new_file_name,
                });
            }
            RenameTarget::UntitledOrigin => {
                if !new_name.is_empty() {
                    let _ = ctx.resource_scope::<ModelicaDocumentRegistry, _>(|_, registry| {
                        if let Some(host) = registry.host_mut(doc_id) {
                            host.document_mut()
                                .set_origin(lunco_doc::DocumentOrigin::untitled(new_name));
                        }
                    });
                }
            }
            RenameTarget::None => {}
        }
        let _ = ctx.resource_scope::<DocRenameState, _>(|_, state| {
            state.editing = None;
        });
    } else if cancel_rename {
        let _ = ctx.resource_scope::<DocRenameState, _>(|_, state| {
            state.editing = None;
        });
    } else if let Some(initial) = start_rename {
        let _ = ctx.resource_scope::<DocRenameState, _>(|_, state| {
            state.editing = Some((doc_id, initial));
            state.needs_focus = true;
        });
    } else if let Some(draft) = update_draft {
        let _ = ctx.resource_scope::<DocRenameState, _>(|_, state| {
            state.editing = Some((doc_id, draft));
        });
    }
}

/// Render the class tree of one writable / Untitled workspace
/// document. Called by the Modelica browser section —
/// the outer `CollapsingHeader` row carrying this doc's name has
/// already been drawn; we just paint the children inline.
///
/// Source-of-truth read of [`crate::state::ModelicaDocumentRegistry`] via the doc's
/// [`crate::index::ModelicaIndex`]. Stateless; the registry's
/// off-thread refresh + per-op optimistic patches keep the Index current.
pub(crate) fn render_workspace_doc(
    ui: &mut egui::Ui,
    ctx: &mut BrowserCtx<'_, '_>,
    doc_id: DocumentId,
) {
    let (classes, has_parse_errors) = match ctx
        .resource::<ModelicaDocumentRegistry>()
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
        .resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);

    let active_doc: Option<DocumentId> = ctx
        .resource::<lunco_workspace::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    let active_qualified: Option<String> =
        active_doc.and_then(|d| crate::sim_default::drilled_class_for_doc_in(ctx, d));

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
        .resource::<ModelicaDocumentRegistry>()
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
                "Parse error".to_string(),
                egui::Color32::from_rgb(220, 160, 60),
            )
        } else {
            (
                "(no classes yet)".to_string(),
                ui.visuals().weak_text_color(),
            )
        };
        ui.label(egui::RichText::new(text).color(color).small().italics());
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
    fn build_subtree(index: &crate::index::ModelicaIndex, qualified: &str) -> Option<ClassEntry> {
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
    let mut doc = crate::document::ModelicaDocument::with_origin(
        DocumentId::new(1),
        source.to_string(),
        DocumentOrigin::untitled("test"),
    );
    // Parsing is lazy — the constructor seeds an empty placeholder index.
    // `refresh_ast_now()` parses (with error recovery, via the single
    // `SyntaxCache::from_source` path) and rebuilds the index. This is also a
    // regression guard: if anyone makes refresh_ast_now parse strictly again,
    // `broken_sibling_class_does_not_wipe_the_others` below will fail.
    doc.refresh_ast_now();
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
    ctx: &mut BrowserCtx<'_, '_>,
) {
    use crate::ui::theme::ModelicaThemeExt;
    let badge = type_badge(&class.kind, theme);
    let is_active =
        Some(doc_id) == active_doc && active_qualified == Some(class.qualified_path.as_str());

    if class.children.is_empty() {
        let resp = ui
            .horizontal(|ui| {
                paint_badge(ui, badge, theme);
                let label = if is_active {
                    egui::RichText::new(&class.short_name).strong()
                } else {
                    egui::RichText::new(&class.short_name)
                };
                ui.add(
                    egui::Label::new(label)
                        .selectable(false)
                        .sense(egui::Sense::click()),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
            })
            .inner;
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
        let mut header_text = egui::RichText::new(format!("{} {}", badge.letter, class.short_name));
        if is_active {
            header_text = header_text.strong();
        }
        let header = egui::CollapsingHeader::new(header_text)
            .id_salt(("modelica_class", &class.qualified_path))
            .default_open(true);
        let resp = header.show(ui, |ui| {
            for child in &class.children {
                render_class_row(ui, child, doc_id, active_doc, active_qualified, theme, ctx);
            }
        });
        let qualified = class.qualified_path.clone();
        let short = class.short_name.clone();
        let muted = theme.text_muted();
        let header_resp = resp
            .header_response
            .clone()
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        header_resp.on_hover_ui(move |ui| {
            ui.strong(&short);
            ui.label(egui::RichText::new(&qualified).small().color(muted));
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
        // Order is the browser's display sort (`browser_sort_group`), not the
        // source order: sub-packages first, then Model/Block/Connector/Record/
        // Function leaves.
        // Don't `use ClassType::*` — `Function` collides with
        // `bevy::reflect::Function` re-exported through other paths.
        assert!(matches!(
            kinds.as_slice(),
            [
                ClassType::Package,
                ClassType::Model,
                ClassType::Block,
                ClassType::Connector,
                ClassType::Record,
                ClassType::Function,
            ]
        ));
    }

    #[test]
    fn fixture_file_parses() {
        let src = crate::models::get_model("AnnotatedRocketStage.mo")
            .expect("bundled AnnotatedRocketStage.mo");
        let (cs, _errors) = parse_classes(src);
        // Top level: one package.
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].short_name, "AnnotatedRocketStage");
        assert!(matches!(cs[0].kind, ClassType::Package));
        // Models in the package: RocketStage + Tank + Valve + Engine + Airframe
        // (plus the FluidPort* / *Signal* connectors and the LunCoAnnotations
        // sub-package, which are children too).
        let child_names: Vec<&str> = cs[0]
            .children
            .iter()
            .map(|c| c.short_name.as_str())
            .collect();
        for expected in ["RocketStage", "Engine", "Tank", "Valve", "Airframe"] {
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

    #[test]
    fn generated_network_name_uses_the_composed_scope_leaf() {
        assert_eq!(
            generated_network_display_name("/Rover/Electrical"),
            "Electrical network"
        );
        assert_eq!(
            generated_network_display_name("/Rover/Electrical/"),
            "Electrical network"
        );
        assert_eq!(generated_network_display_name("/"), "Generated network");
    }

    #[test]
    fn generated_metadata_pluralization_stays_readable() {
        assert_eq!(plural_suffix(1), "");
        assert_eq!(plural_suffix(0), "s");
        assert_eq!(plural_suffix(2), "s");
    }

    #[test]
    fn single_unit_generated_network_opens_member_class() {
        let entry = crate::state::GeneratedModelicaSourceEntry {
            document: DocumentId::new(1),
            uri: "generated://Rover/Electrical.mo".to_string(),
            network_root: "/Rover/Electrical".to_string(),
            model_name: "Rover_Electrical_System".to_string(),
            source: "model Rover_Electrical_System end Rover_Electrical_System;".to_string(),
            component_paths: Vec::new(),
            units: vec![crate::state::GeneratedModelicaUnit {
                name: "Unit_Rover_Battery".to_string(),
                ..Default::default()
            }],
            members: Vec::new(),
            source_roots: Vec::new(),
            boundary_inputs: Vec::new(),
            boundary_outputs: Vec::new(),
            member_output_aliases: Vec::new(),
            error: None,
        };
        assert_eq!(generated_network_open_class(&entry), "Unit_Rover_Battery");
    }

    #[test]
    fn generated_details_use_readable_names_and_keep_technical_names_for_tooltips() {
        assert_eq!(
            generated_path_leaf("/Rover/YawHead/SolarPanel"),
            "SolarPanel"
        );
        assert_eq!(
            generated_class_display_name("SolarRoverTest_x2f_SolarRover_x2f_Electrical_System"),
            "SolarRoverTest / SolarRover / Electrical"
        );
        assert_eq!(
            generated_member_display_name(
                "/SolarRoverTest/SolarRover/Motor_FL",
                "LunCo.Electrical.DCMotor"
            ),
            "Motor_FL · DCMotor"
        );
        assert_eq!(
            generated_member_output_display_name(
                "/SolarRoverTest/SolarRover/Motor_FL",
                "electrical_power"
            ),
            "Motor_FL.electrical_power"
        );
    }

    #[test]
    fn multi_unit_generated_network_opens_root_class() {
        let entry = crate::state::GeneratedModelicaSourceEntry {
            document: DocumentId::new(1),
            uri: "generated://Rover/Electrical.mo".to_string(),
            network_root: "/Rover/Electrical".to_string(),
            model_name: "Rover_Electrical_System".to_string(),
            source: "model Rover_Electrical_System end Rover_Electrical_System;".to_string(),
            component_paths: Vec::new(),
            units: vec![
                crate::state::GeneratedModelicaUnit {
                    name: "Unit_Rover_Battery".to_string(),
                    ..Default::default()
                },
                crate::state::GeneratedModelicaUnit {
                    name: "Unit_Rover_Controller".to_string(),
                    ..Default::default()
                },
            ],
            members: Vec::new(),
            source_roots: Vec::new(),
            boundary_inputs: Vec::new(),
            boundary_outputs: Vec::new(),
            member_output_aliases: Vec::new(),
            error: None,
        };
        assert_eq!(
            generated_network_open_class(&entry),
            "Rover_Electrical_System"
        );
    }
}
