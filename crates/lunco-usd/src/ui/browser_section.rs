//! `UsdSceneSection` — Twin-browser entry surfacing every loaded USD
//! stage in the Models scope.
//!
//! Direct mirror of `lunco_modelica::ui::browser_section::ModelicaSection`:
//! one `BrowserSection` impl that iterates
//! [`LoadedUsdStages`] and
//! draws a `CollapsingHeader` row per [`crate::ui::loaded_stages::LoadedStage`].
//!
//! Phase 3 paints the row + a placeholder body. Phase 4 swaps the
//! placeholder for the recursive prim-tree walk over composed stages.

use bevy_egui::egui;
use lunco_workbench::twin_browser::BrowserScope;
use lunco_workbench::{BrowserCtx, BrowserSection, FocusPanel};

use crate::ui::loaded_stages::LoadedUsdStages;
use crate::ui::viewport::{SetActiveUsdViewport, USD_VIEWPORT_PANEL_ID};

/// Browser section that lists every loaded USD stage as a sibling row
/// in the Twin browser's Models scope. Populated by the lifecycle
/// observers in [`UsdUiPlugin`](crate::ui::UsdUiPlugin).
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

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_>) {
        // Take the registry out of the world for the duration of the
        // render so each entry can borrow `ctx.world` mutably without
        // clashing with the resource borrow. Same pattern Modelica
        // uses for `LoadedModelicaClasses`.
        let Some(mut loaded) = ctx.world.remove_resource::<LoadedUsdStages>() else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "LoadedUsdStages resource missing",
            );
            return;
        };

        if loaded.entries.is_empty() {
            ui.label(
                egui::RichText::new("No USD stages open. Open or create a `.usda` to add one.")
                    .weak()
                    .italics(),
            );
            ctx.world.insert_resource(loaded);
            return;
        }

        // Collect viewport-target requests during the render pass.
        // Triggering inside the egui callbacks would clash with the
        // resource borrow we still hold (`loaded`); we batch and
        // dispatch after `loaded` is reinserted into the world.
        let mut focus_doc: Option<lunco_doc::DocumentId> = None;

        for entry in &mut loaded.entries {
            let header_id = ui.make_persistent_id(("usd-stage", entry.id().to_string()));
            let title = entry.name(ctx);
            let writable_badge = if entry.writable() { "" } else { "  🔒" };
            let viewport_doc = entry.doc_id_for_viewport();
            let default_open = entry.default_open();
            // Clicking the label both shows the stage in the viewport
            // *and* folds/unfolds the row — same as the triangle.
            lunco_ui::helpers::collapsing_row(
                ui,
                header_id,
                default_open,
                |ui| {
                    let label = format!("{}{}", title, writable_badge);
                    let Some(doc) = viewport_doc else {
                        ui.label(label);
                        return false;
                    };
                    let resp = ui
                        .add(egui::Label::new(label).sense(egui::Sense::click()))
                        .on_hover_text("Click to show in 3D viewport");
                    if resp.clicked() {
                        focus_doc = Some(doc);
                    }
                    resp.clicked()
                },
                |ui| entry.render_children(ui, ctx),
            );
        }

        ctx.world.insert_resource(loaded);

        if let Some(doc) = focus_doc {
            ctx.world.commands().trigger(SetActiveUsdViewport { doc });
            ctx.world.commands().trigger(FocusPanel {
                id: USD_VIEWPORT_PANEL_ID.0.to_string(),
            });
        }
    }
}
