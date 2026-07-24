//! `UsdSceneSection` — Twin-browser entry surfacing every loaded USD
//! stage in the Models scope.
//!
//! Under the WP-8 reactive-egui contract the section is a pure reader:
//! it snapshots the change-gated [`UsdBrowserView`] (built by
//! [`produce_usd_browser_view`](crate::ui::loaded_stages::produce_usd_browser_view))
//! through `BrowserCtx::resource`, paints the row + prim tree, and emits
//! viewport intent via `BrowserCtx::defer`. No `&mut World`, no inline
//! parse, no resource take-and-restore.

use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::twin_browser::BrowserScope;
use lunco_workbench::{BrowserCtx, BrowserSection};
use openusd::sdf;
// The layer browser walks the AUTHORED specs of a layer, deliberately without
// composition — so it reads through `UsdDataExt` (the authored-layer accessor),
// not `UsdRead` (which is now the composed-stage contract, one impl: `StageView`).
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::UsdData;

use crate::ui::loaded_stages::{UsdBrowserView, UsdStageRow};
use crate::ui::viewport::{SetActiveUsdViewport, USD_VIEWPORT_PANEL_ID};

/// Browser section that lists every loaded USD stage as a sibling row
/// in the Twin browser's Models scope. Populated by the lifecycle
/// observers in [`UsdUiPlugin`](crate::ui::UsdUiPlugin) (via
/// [`LoadedUsdStages`](crate::ui::loaded_stages::LoadedUsdStages)) and
/// flattened into [`UsdBrowserView`] by the producer system.
pub struct UsdSceneSection;

impl BrowserSection for UsdSceneSection {
    fn id(&self) -> &str {
        "usd-scenes"
    }

    fn title(&self) -> &str {
        "USD"
    }

    fn scope(&self) -> BrowserScope {
        // USD belongs in the same Models tab as Modelica — both are
        // typed-domain content of the open Twin. Files-scope rendering
        // of `.usda` files (raw on-disk view) is handled by the
        // built-in FilesSection independently.
        BrowserScope::Models
    }

    fn default_open(&self) -> bool {
        // Collapse by default so the USD section renders as a folder
        // entry until the user opens it. Avoids drowning the browser
        // with stage rows in folders containing many `.usda` files.
        false
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let error_color = ctx
            .resource::<lunco_theme::Theme>()
            .map(|t| t.tokens.error)
            .unwrap_or(egui::Color32::LIGHT_RED);

        // Snapshot the view-model out of the (immutable) ctx borrow so
        // the later `ctx.defer` dispatch is free to take `&mut`. Rows
        // are cheap to clone (Arc readers + short strings).
        let rows: Vec<UsdStageRow> = match ctx.resource::<UsdBrowserView>() {
            Some(view) => view.stages.clone(),
            None => {
                ui.colored_label(error_color, "UsdBrowserView resource missing");
                return;
            }
        };

        if rows.is_empty() {
            ui.label(
                egui::RichText::new("No USD stages open. Open or create a `.usda` to add one.")
                    .weak()
                    .italics(),
            );
            return;
        }

        // Collect viewport-target requests during the render pass.
        // Deferring inside the egui callbacks would clash with the
        // immutable borrows the closures hold; we batch one click and
        // dispatch after the rows finish painting.
        let mut focus_doc: Option<DocumentId> = None;

        for row in &rows {
            let header_id = ui.make_persistent_id(("usd-stage", &row.salt));
            let writable_badge = if row.writable { "" } else { "  🔒" };
            let viewport_doc = row.doc_id;
            let default_open = row.default_open;

            // `add_header` and `add_body` are both `FnOnce` handed to
            // `collapsing_row` at once, so they can't share a `&mut`.
            // Each records its own click into a distinct local; combine
            // afterwards.
            let mut header_clicked = false;
            let mut body_clicked = false;
            // Clicking the label both shows the stage in the viewport
            // *and* folds/unfolds the row — same as the triangle.
            lunco_ui::helpers::collapsing_row(
                ui,
                header_id,
                default_open,
                |ui| {
                    let label = format!("{}{}", row.name, writable_badge);
                    if viewport_doc.is_none() {
                        ui.label(label);
                        return false;
                    }
                    let resp = ui
                        .add(egui::Label::new(label).sense(egui::Sense::click()))
                        .on_hover_text("Click to show in 3D viewport");
                    header_clicked = resp.clicked();
                    resp.clicked()
                },
                |ui| {
                    body_clicked = render_stage_body(ui, row, error_color);
                },
            );

            if (header_clicked || body_clicked) && viewport_doc.is_some() {
                focus_doc = viewport_doc;
            }
        }

        // Clicking any stage / prim row retargets the shared USD
        // viewport at the owning doc and focuses the viewport tab.
        // `SetActiveUsdViewport` / `FocusPanel` are typed commands —
        // emitted after paint via `defer` so the egui pass stays a
        // pure read.
        if let Some(doc) = focus_doc {
            ctx.defer(move |world| {
                world.trigger(SetActiveUsdViewport { doc });
                world.trigger(lunco_workbench::FocusPanel {
                    id: USD_VIEWPORT_PANEL_ID.0.to_string(),
                });
            });
        }
    }
}

/// Paint one stage's prim-tree body from its pre-derived row. Returns
/// `true` when the user clicked a prim row (→ retarget the viewport).
/// Pure read over the cached [`TextReader`]; no world access.
fn render_stage_body(ui: &mut egui::Ui, row: &UsdStageRow, error_color: egui::Color32) -> bool {
    if let Some(err) = &row.parse_error {
        ui.colored_label(error_color, err);
        return false;
    }
    let Some(reader) = &row.reader else {
        ui.label(egui::RichText::new("(no parse)").weak().italics());
        return false;
    };

    let root = match sdf::path("/") {
        Ok(p) => p,
        Err(e) => {
            ui.colored_label(error_color, format!("root path: {e}"));
            return false;
        }
    };

    // Collapse a redundant single-root-prim wrapper whose name matches
    // the doc filename. e.g. a stage `artemis_2.usda` with a single
    // `def Xform "Artemis2"` is surfaced as `artemis_2 → Orion` instead
    // of `artemis_2 → Artemis2 (Xform) → Orion`. Single-root prims with
    // no children are kept (they ARE the content).
    let mut top_paths: Vec<sdf::Path> = reader.prim_children(&root);
    if top_paths.len() == 1 {
        let grand = reader.prim_children(&top_paths[0]);
        if !grand.is_empty() {
            top_paths = grand;
        }
    }

    let mut clicked_prim = false;
    if top_paths.is_empty() {
        ui.label(egui::RichText::new("(no prims)").weak().italics());
    } else {
        for path in top_paths {
            render_prim(ui, reader, &path, &row.salt, &mut clicked_prim);
        }
    }
    clicked_prim
}

/// Recursive prim-tree walker. One `CollapsingHeader` per prim;
/// children fetched via [`UsdRead::children`].
///
/// Composition arcs (sublayers, references, payloads) are **not**
/// flattened — referenced prims show up only after a future
/// `UsdComposer` integration. Today the walk reflects the raw root
/// layer, which is the source-of-truth most edits target.
fn render_prim(
    ui: &mut egui::Ui,
    reader: &UsdData,
    path: &sdf::Path,
    salt: &str,
    clicked: &mut bool,
) {
    let name = path.name().unwrap_or("(root)").to_string();
    let type_name = reader.prim_type_name(path);
    let label = match &type_name {
        Some(ty) => format!("{} ({})", name, ty),
        None => name,
    };
    let children = reader.prim_children(path);
    let header_id = ui.make_persistent_id((salt, path.to_string()));

    if children.is_empty() {
        ui.indent(header_id, |ui| {
            let resp = ui
                .add(egui::Label::new(&label).sense(egui::Sense::click()))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                *clicked = true;
            }
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
                    render_prim(ui, reader, &child, salt, clicked);
                }
            },
        );
        if row_clicked {
            *clicked = true;
        }
    }
}
