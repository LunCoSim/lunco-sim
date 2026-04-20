//! Modelica section of the Twin Browser.
//!
//! ## What it shows
//!
//! 1. Every Modelica document currently loaded in the
//!    [`ModelicaDocumentRegistry`] — drafts, duplicates from the
//!    Welcome examples, files opened in earlier sessions. This is the
//!    workspace's authoritative view of "what Modelica content does
//!    the user have right now."
//! 2. *(Future)* Files in the open Twin folder that aren't loaded yet
//!    — surfaced as a separate group so users can click to load.
//!
//! Each row is a Modelica class keyed by its **fully-qualified path**
//! (e.g. `"AnnotatedRocketStage.RocketStage"`). Click → emits
//! [`BrowserAction::OpenLoadedClass`] for in-memory docs, dispatched
//! into the existing drill-in machinery so the canvas tab opens
//! directly on the requested class.
//!
//! ## Caching
//!
//! Per-document parse cache keyed by source content hash. Cheap (typical
//! Modelica files are <10 KB) so we re-parse on every miss; rumoca runs
//! synchronously here. The MSL case will need a task-pool bounce — out
//! of scope for slice 3.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::{BrowserAction, BrowserCtx, BrowserSection};
use rumoca_phase_parse::parse_to_ast;
use rumoca_session::parsing::ast::ClassDef;
use rumoca_session::parsing::ClassType;

use crate::ui::panels::canvas_diagram::DrilledInClassNames;
use crate::ui::state::{ModelicaDocumentRegistry, WorkbenchState};

/// One Modelica class entry rendered in the tree.
#[derive(Debug, Clone)]
struct ClassEntry {
    /// Short identifier (e.g. `"Engine"`).
    short_name: String,
    /// Fully-qualified path (e.g. `"AnnotatedRocketStage.Engine"`).
    qualified_path: String,
    /// Modelica class kind — drives the row's letter badge.
    kind: ClassType,
    /// Description string (the `"…"` after the class header), if present.
    description: Option<String>,
    /// Children — nested classes inside a package / model.
    children: Vec<ClassEntry>,
}

/// Per-document parse cache entry — keyed by source-content hash so
/// edits invalidate naturally.
struct DocCache {
    source_hash: u64,
    classes: Vec<ClassEntry>,
}

/// The Modelica Twin-Browser section.
#[derive(Default)]
pub struct ModelicaSection {
    /// `DocumentId → parsed cache`. Stale entries (for closed
    /// documents) are GC'd whenever a render finds them missing from
    /// the registry.
    cache: HashMap<DocumentId, DocCache>,
}

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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_>) {
        // Snapshot the registry so we can release the borrow before
        // emitting actions (the dispatcher will mutate the registry
        // when opening tabs). Tuple shape: (id, display_name, source).
        let docs: Vec<(DocumentId, String, String)> = {
            let Some(registry) = ctx.world.get_resource::<ModelicaDocumentRegistry>()
            else {
                ui.label(
                    egui::RichText::new("(Modelica document registry not initialised)")
                        .weak()
                        .italics(),
                );
                return;
            };
            let mut entries: Vec<(DocumentId, String, String)> = registry
                .iter()
                // Workspace = user content. Read-only library docs
                // (bundled examples opened via "Open as read-only",
                // MSL classes the user clicked through to inspect)
                // are *not* part of the workspace — they're
                // references. Keep only writable files (User-opened,
                // saved drafts) and untitled drafts (unsaved scratch).
                .filter(|(_, host)| {
                    let origin = host.document().origin();
                    origin.is_writable() || origin.is_untitled()
                })
                .map(|(id, host)| {
                    let doc = host.document();
                    let display = doc.origin().display_name();
                    let source = doc.source().to_string();
                    (id, display, source)
                })
                .collect();
            entries.sort_by(|a, b| a.1.cmp(&b.1));
            entries
        };

        // GC cache entries whose docs are gone.
        let live_ids: std::collections::HashSet<DocumentId> =
            docs.iter().map(|(id, _, _)| *id).collect();
        self.cache.retain(|id, _| live_ids.contains(id));

        if docs.is_empty() {
            ui.label(
                egui::RichText::new("Workspace is empty.")
                    .weak()
                    .italics(),
            );
            ui.label(
                egui::RichText::new(
                    "Add Modelica content via the Welcome tab — \
                     New Model, Open Folder, or Try an example.",
                )
                .weak()
                .small(),
            );
            return;
        }

        // What's currently in the foreground tab? Used to render that
        // (doc, class) pair as selected so users see "I'm editing
        // this." Active doc comes from `WorkbenchState.open_model`,
        // active class from `DrilledInClassNames` keyed by that doc.
        // When no class is drilled in we still highlight every
        // top-level row of the active doc — answers "which doc am I
        // looking at" even before any drill-in.
        let active_doc: Option<DocumentId> = ctx
            .world
            .get_resource::<WorkbenchState>()
            .and_then(|s| s.open_model.as_ref().and_then(|m| m.doc));
        let active_qualified: Option<String> = active_doc.and_then(|d| {
            ctx.world
                .get_resource::<DrilledInClassNames>()
                .and_then(|m| m.get(d).map(str::to_string))
        });

        // Render only the Modelica hierarchy — the document/file is
        // not a Modelica concept and showing it as a parent row
        // duplicates the package name in the common single-class
        // file case. Each top-level class becomes its own root row;
        // the doc binding stays implicit (carried via `doc_id` into
        // the click action). Drafts with no classes show a faint
        // placeholder row so users know which doc is empty.
        egui::ScrollArea::vertical()
            .id_salt("twin_browser_modelica_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for (doc_id, display_name, source) in &docs {
                    self.refresh_doc(*doc_id, source);
                    let Some(entry) = self.cache.get(doc_id) else {
                        continue;
                    };
                    if entry.classes.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "{}  (no classes yet)",
                                display_name
                            ))
                            .weak()
                            .small()
                            .italics(),
                        );
                        continue;
                    }
                    for class in &entry.classes {
                        render_class_row(
                            ui,
                            class,
                            *doc_id,
                            active_doc,
                            active_qualified.as_deref(),
                            ctx,
                        );
                    }
                }
            });
    }
}

impl ModelicaSection {
    /// Refresh the parse for `doc_id` if `source`'s hash differs from
    /// the cached one.
    fn refresh_doc(&mut self, doc_id: DocumentId, source: &str) {
        let hash = hash_source(source);
        if self.cache.get(&doc_id).map(|c| c.source_hash) == Some(hash) {
            return;
        }
        self.cache.insert(
            doc_id,
            DocCache {
                source_hash: hash,
                classes: parse_classes(source),
            },
        );
    }
}

fn hash_source(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse a `.mo` source into a tree of [`ClassEntry`] keyed by
/// qualified path. Recursive — packages with nested classes produce
/// nested children.
fn parse_classes(source: &str) -> Vec<ClassEntry> {
    let Ok(ast) = parse_to_ast(source, "twin.mo") else {
        return Vec::new();
    };
    collect_classes(&ast.classes, "")
}

/// Walk an `IndexMap<String, ClassDef>` building [`ClassEntry`]
/// records. `parent_path` is the dotted prefix to apply to each
/// child's qualified path — empty for top-level classes.
fn collect_classes(
    classes: &indexmap::IndexMap<String, ClassDef>,
    parent_path: &str,
) -> Vec<ClassEntry> {
    let mut out = Vec::new();
    for (short, class_def) in classes {
        let qualified = if parent_path.is_empty() {
            short.clone()
        } else {
            format!("{}.{}", parent_path, short)
        };
        out.push(ClassEntry {
            short_name: short.clone(),
            qualified_path: qualified.clone(),
            kind: class_def.class_type.clone(),
            description: class_def
                .description
                .iter()
                .next()
                .map(|t| t.text.as_ref().trim_matches('"').to_string())
                .filter(|s| !s.is_empty()),
            children: collect_classes(&class_def.classes, &qualified),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Paint one class row. Recurses into children when the row is
/// expanded. Click → [`BrowserAction::OpenLoadedClass`] keyed by the
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
    ctx: &mut BrowserCtx<'_>,
) {
    let badge = type_badge(&class.kind);
    let is_active = Some(doc_id) == active_doc
        && active_qualified == Some(class.qualified_path.as_str());

    if class.children.is_empty() {
        ui.horizontal(|ui| {
            paint_badge(ui, badge);
            // `selectable_label`'s `selected` flag drives egui's own
            // highlight chrome — same look as the active tab in the
            // dock, so the visual language is consistent.
            let label = if is_active {
                egui::RichText::new(&class.short_name).strong()
            } else {
                egui::RichText::new(&class.short_name)
            };
            let resp = ui.selectable_label(is_active, label);
            if resp.clicked() {
                ctx.actions.push(BrowserAction::OpenLoadedClass {
                    doc_id: doc_id.raw(),
                    qualified_path: class.qualified_path.clone(),
                });
            }
            // Description + qualified path move to the hover tooltip.
            // Rendering description inline eats horizontal space,
            // duplicates context already implied by the class name,
            // and wraps awkwardly in a narrow side panel. Hover
            // keeps the tree dense; users who want the blurb get it
            // on demand.
            resp.on_hover_ui(|ui| {
                ui.strong(&class.short_name);
                ui.label(
                    egui::RichText::new(&class.qualified_path)
                        .small()
                        .color(egui::Color32::from_rgb(150, 170, 200)),
                );
                if let Some(desc) = &class.description {
                    ui.separator();
                    ui.label(egui::RichText::new(desc).small());
                }
            });
        });
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
                    ctx,
                );
            }
        });
        let desc = class.description.clone();
        let qualified = class.qualified_path.clone();
        let short = class.short_name.clone();
        resp.header_response.clone().on_hover_ui(move |ui| {
            ui.strong(&short);
            ui.label(
                egui::RichText::new(&qualified)
                    .small()
                    .color(egui::Color32::from_rgb(150, 170, 200)),
            );
            if let Some(desc) = &desc {
                ui.separator();
                ui.label(egui::RichText::new(desc).small());
            }
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
struct Badge {
    letter: &'static str,
    bg: egui::Color32,
}

fn type_badge(kind: &ClassType) -> Badge {
    use egui::Color32 as C;
    match kind {
        ClassType::Model => Badge {
            letter: "M",
            bg: C::from_rgb(80, 130, 200),
        },
        ClassType::Block => Badge {
            letter: "B",
            bg: C::from_rgb(100, 160, 110),
        },
        ClassType::Class => Badge {
            letter: "C",
            bg: C::from_rgb(120, 130, 160),
        },
        ClassType::Connector => Badge {
            letter: "X",
            bg: C::from_rgb(220, 160, 80),
        },
        ClassType::Record => Badge {
            letter: "R",
            bg: C::from_rgb(170, 120, 180),
        },
        ClassType::Type => Badge {
            letter: "T",
            bg: C::from_rgb(150, 150, 150),
        },
        ClassType::Package => Badge {
            letter: "P",
            bg: C::from_rgb(190, 110, 110),
        },
        ClassType::Function => Badge {
            letter: "F",
            bg: C::from_rgb(110, 170, 200),
        },
        ClassType::Operator => Badge {
            letter: "O",
            bg: C::from_rgb(160, 160, 110),
        },
    }
}

fn paint_badge(ui: &mut egui::Ui, badge: Badge) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(badge.letter)
                .monospace()
                .small()
                .background_color(badge.bg)
                .color(egui::Color32::WHITE),
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
model B "with description" end B;
"#;
        let cs = parse_classes(src);
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].short_name, "A");
        assert_eq!(cs[0].qualified_path, "A");
        assert!(matches!(cs[0].kind, ClassType::Model));
        assert_eq!(cs[1].description.as_deref(), Some("with description"));
    }

    #[test]
    fn parses_nested_classes_with_qualified_paths() {
        let src = r#"
package P
  model Inner end Inner;
  model Other "x" end Other;
end P;
"#;
        let cs = parse_classes(src);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].short_name, "P");
        assert!(matches!(cs[0].kind, ClassType::Package));
        assert_eq!(cs[0].children.len(), 2);
        assert_eq!(cs[0].children[0].qualified_path, "P.Inner");
        assert_eq!(cs[0].children[1].qualified_path, "P.Other");
    }

    #[test]
    fn empty_source_returns_empty() {
        assert!(parse_classes("").is_empty());
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
        let cs = parse_classes(src);
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
        let cs = parse_classes(src);
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
