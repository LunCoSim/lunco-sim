//! `UsdSceneSection` — Twin-browser entry surfacing every loaded USD
//! stage in the Models scope.
//!
//! Under the WP-8 reactive-egui contract the section is a pure reader:
//! it snapshots the change-gated [`UsdBrowserView`] (built by
//! [`produce_usd_browser_view`](crate::ui::loaded_stages::produce_usd_browser_view))
//! through `BrowserCtx::resource`, paints the row + prim tree, and emits
//! viewport intent through typed commands. No `&mut World`, no inline
//! parse, no resource take-and-restore.

use bevy_egui::egui;
use lunco_doc::DocumentId;
use lunco_workbench::twin_browser::{BrowserAction, BrowserQuery, BrowserScope};
use lunco_workbench::{BrowserCtx, BrowserSection};
use openusd::sdf;
// The layer browser walks the AUTHORED specs of a layer, deliberately without
// composition — so it reads through `UsdDataExt` (the authored-layer accessor),
// not `UsdRead` (which is now the composed-stage contract, one impl: `StageView`).
use lunco_usd_bevy::usd_data::UsdDataExt;
use lunco_usd_bevy::UsdData;

use crate::ui::loaded_stages::{UsdBrowserView, UsdStageRow};
use crate::ui::viewport::{OpenUsdPreview, EDITOR_PREVIEW_ID, USD_VIEWPORT_PANEL_ID};
use crate::ui::USD_CONNECTION_CANVAS_PANEL_ID;
use crate::{
    CommitUsdProposal, LayerId, ReviewUsdProposal, UsdProposalId, UsdProposalReviewAction,
    UsdProposalState, UsdProposalSummary,
};

/// Twin navigation entry for the composed USD connection graph.
///
/// The graph itself remains a simulator/editor panel because it needs the
/// live spawned-stage projection and USD write-back path. The entry belongs to
/// the USD domain and is surfaced by the Twin Browser used by Lunica's Design
/// workspace, rather than by a separate top-level perspective.
pub struct ConnectionsSection;

impl BrowserSection for ConnectionsSection {
    fn id(&self) -> &str {
        "lunco.usd.connections"
    }

    fn title(&self) -> &str {
        "Connections"
    }

    fn scope(&self) -> BrowserScope {
        BrowserScope::Models
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        125
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        ui.label("Inspect and edit the wiring graph of the selected Assembly document.");
        if ui.button("Open Connections graph").clicked() {
            ctx.actions.push(BrowserAction::OpenPanel {
                id: USD_CONNECTION_CANVAS_PANEL_ID.0.to_string(),
            });
        }
        if ctx
            .resource::<UsdBrowserView>()
            .is_some_and(|view| view.stages.is_empty())
        {
            ui.label(
                egui::RichText::new("Select a USD document first to populate the graph.")
                    .weak()
                    .italics(),
            );
        }
    }
}

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
        // typed commands can be emitted after row painting. Rows
        // are cheap to clone (Arc readers + short strings).
        let query = ctx.resource::<BrowserQuery>().cloned().unwrap_or_default();
        let rows: Vec<UsdStageRow> = match ctx.resource::<UsdBrowserView>() {
            Some(view) => view
                .stages
                .iter()
                .filter(|row| stage_matches(row, &query))
                .cloned()
                .collect(),
            None => {
                ui.colored_label(error_color, "UsdBrowserView resource missing");
                return;
            }
        };

        if rows.is_empty() {
            let message = if query.is_active() {
                "No USD stages match the filter."
            } else {
                "No USD stages open. Open or create a `.usda` to add one."
            };
            ui.label(egui::RichText::new(message).weak().italics());
            return;
        }

        // Collect viewport-target requests during the render pass.
        // Deferring inside the egui callbacks would clash with the
        // immutable borrows the closures hold; we batch one click and
        // dispatch after the rows finish painting.
        let mut focus_doc: Option<DocumentId> = None;
        let mut proposal_actions = Vec::new();

        for row in &rows {
            let header_id = ui.make_persistent_id(("usd-stage", &row.salt));
            let writable_badge = if row.writable { "" } else { "  [read-only]" };
            let dirty_badge = if row.dirty { "  [modified]" } else { "" };
            let review_badge = review_badge(&row.edit_proposals);
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
                    let label = format!(
                        "{}{}{}{}",
                        row.name, writable_badge, dirty_badge, review_badge
                    );
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
                    body_clicked = render_stage_body(ui, row, error_color, &query);
                    render_proposal_review(
                        ui,
                        &row.edit_proposals,
                        error_color,
                        &mut proposal_actions,
                    );
                },
            );

            if (header_clicked || body_clicked) && viewport_doc.is_some() {
                focus_doc = viewport_doc;
            }
        }

        // Clicking any stage / prim row opens or replaces the desktop editor's
        // explicit preview lease and focuses the viewport tab. The command is
        // emitted after paint so the egui pass stays a pure read.
        if let Some(doc) = focus_doc {
            ctx.trigger(OpenUsdPreview {
                preview: EDITOR_PREVIEW_ID,
                doc,
                edit_target: LayerId::root(),
            });
            ctx.trigger(lunco_workbench::FocusPanel {
                id: USD_VIEWPORT_PANEL_ID.0.to_string(),
            });
        }
        for action in proposal_actions {
            match action {
                ProposalAction::Review { proposal, action } => {
                    ctx.trigger(ReviewUsdProposal { proposal, action });
                }
                ProposalAction::Commit { proposal } => {
                    ctx.trigger(CommitUsdProposal { proposal });
                }
            }
        }
    }
}

fn review_badge(proposals: &[UsdProposalSummary]) -> String {
    let conflicts = proposals
        .iter()
        .filter(|proposal| proposal.state == UsdProposalState::Conflict)
        .count();
    let pending = proposals
        .iter()
        .filter(|proposal| proposal.state == UsdProposalState::Pending)
        .count();
    let muted = proposals
        .iter()
        .filter(|proposal| proposal.state == UsdProposalState::Muted)
        .count();
    if conflicts != 0 {
        format!("  [conflict {conflicts}]")
    } else if pending != 0 {
        format!("  [review {pending}]")
    } else if muted != 0 {
        format!("  [muted {muted}]")
    } else {
        String::new()
    }
}

enum ProposalAction {
    Review {
        proposal: UsdProposalId,
        action: UsdProposalReviewAction,
    },
    Commit {
        proposal: UsdProposalId,
    },
}

fn render_proposal_review(
    ui: &mut egui::Ui,
    proposals: &[UsdProposalSummary],
    error_color: egui::Color32,
    actions: &mut Vec<ProposalAction>,
) {
    if proposals.is_empty() {
        return;
    }

    ui.separator();
    ui.label("Assembly edit review");
    for proposal in proposals {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "#{} · {} · {}",
                proposal.id.0,
                proposal.scope.as_str(),
                proposal.label
            ));
            ui.weak(format!("{} operation(s)", proposal.operation_count));
        });
        ui.small(format!("Paths: {}", proposal.affected_paths.join(", ")));
        if !proposal.diagnostics.is_empty() {
            ui.colored_label(error_color, proposal.diagnostics.join("; "));
        }
        ui.horizontal(|ui| match proposal.state {
            UsdProposalState::Pending => {
                if ui.button("Commit").clicked() {
                    actions.push(ProposalAction::Commit {
                        proposal: proposal.id,
                    });
                }
                if ui.button("Mute").clicked() {
                    actions.push(ProposalAction::Review {
                        proposal: proposal.id,
                        action: UsdProposalReviewAction::Mute,
                    });
                }
                if ui.button("Reject").clicked() {
                    actions.push(ProposalAction::Review {
                        proposal: proposal.id,
                        action: UsdProposalReviewAction::Reject,
                    });
                }
            }
            UsdProposalState::Muted => {
                if ui.button("Unmute").clicked() {
                    actions.push(ProposalAction::Review {
                        proposal: proposal.id,
                        action: UsdProposalReviewAction::Unmute,
                    });
                }
                if ui.button("Reject").clicked() {
                    actions.push(ProposalAction::Review {
                        proposal: proposal.id,
                        action: UsdProposalReviewAction::Reject,
                    });
                }
            }
            UsdProposalState::Conflict => {
                ui.colored_label(error_color, "Conflict — create a fresh proposal");
                if ui.button("Reject").clicked() {
                    actions.push(ProposalAction::Review {
                        proposal: proposal.id,
                        action: UsdProposalReviewAction::Reject,
                    });
                }
            }
        });
    }
}

/// Paint one stage's prim-tree body from its pre-derived row. Returns
/// `true` when the user clicked a prim row (→ retarget the viewport).
/// Pure read over cached authored-layer data; no world access.
fn render_stage_body(
    ui: &mut egui::Ui,
    row: &UsdStageRow,
    error_color: egui::Color32,
    query: &BrowserQuery,
) -> bool {
    if let Some(err) = &row.parse_error {
        ui.colored_label(error_color, err);
        return false;
    }
    let Some(data) = &row.data else {
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
    let mut top_paths: Vec<sdf::Path> = data.prim_children(&root);
    if top_paths.len() == 1 {
        let grand = data.prim_children(&top_paths[0]);
        if !grand.is_empty() {
            top_paths = grand;
        }
    }

    let mut clicked_prim = false;
    if top_paths.is_empty() {
        ui.label(egui::RichText::new("(no prims)").weak().italics());
    } else {
        let stage_matches_name = query.matches(&row.name);
        for path in top_paths {
            render_prim(
                ui,
                data,
                &path,
                &row.salt,
                query,
                stage_matches_name,
                &mut clicked_prim,
            );
        }
    }
    clicked_prim
}

/// Recursive prim-tree walker. One `CollapsingHeader` per prim;
/// children fetched from the authored-layer data.
///
/// Composition is intentionally not part of this authoring browser: it shows
/// the layer being edited, while runtime projection reads the live canonical
/// composed stage.
fn render_prim(
    ui: &mut egui::Ui,
    data: &UsdData,
    path: &sdf::Path,
    salt: &str,
    query: &BrowserQuery,
    ancestor_matches: bool,
    clicked: &mut bool,
) -> bool {
    let name = path.name().unwrap_or("(root)").to_string();
    let path_text = path.to_string();
    let row_matches = ancestor_matches || query.matches(&name) || query.matches(&path_text);
    let type_name = data.prim_type_name(path);
    let label = match &type_name {
        Some(ty) => format!("{} ({})", name, ty),
        None => name,
    };
    let children = data.prim_children(path);
    if query.is_active()
        && !row_matches
        && !children
            .iter()
            .any(|child| prim_matches(data, child, query))
    {
        return false;
    }
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
                    render_prim(ui, data, &child, salt, query, row_matches, clicked);
                }
            },
        );
        if row_clicked {
            *clicked = true;
        }
    }
    true
}

fn stage_matches(row: &UsdStageRow, query: &BrowserQuery) -> bool {
    if !query.is_active() || query.matches(&row.name) {
        return true;
    }
    row.data.as_deref().is_some_and(|data| {
        sdf::path("/").ok().is_some_and(|root| {
            data.prim_children(&root)
                .iter()
                .any(|path| prim_matches(data, path, query))
        })
    })
}

fn prim_matches(data: &UsdData, path: &sdf::Path, query: &BrowserQuery) -> bool {
    query.matches(path.name().unwrap_or("(root)"))
        || query.matches(&path.to_string())
        || data
            .prim_children(path)
            .iter()
            .any(|child| prim_matches(data, child, query))
}
