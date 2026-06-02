//! Canvas diagram overlay renderers.
//!
//! Centred-card overlays painted on top of the canvas while the
//! background pipeline is busy: drill-in resolving the target class,
//! the off-thread projector building a Scene, or an empty/missing
//! diagram (no graphics in the AST yet).

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_theme::ColorAlpha;

use crate::ui::state::ModelicaDocumentRegistry;
use crate::ui::theme::ModelicaThemeExt;

use super::active_doc_from_world;
// `crate::ui::panels::model_view::drilled_class_for_doc`.

// `render_drill_in_loading_overlay` and `render_projecting_overlay`
// retired — replaced by
// `lunco_ui::busy::LoadingIndicator::for_scope(BusyScope::Document(d))
//     .overlay_on(ui, rect, &bus, &theme)`. Drill-in / duplicate /
// projection all push to `StatusBus`; the widget picks the
// longest-running entry and renders the per-source verb.

/// Painted when a drill-in / duplicate load failed (e.g. MSL bundle
/// not yet ready, class missing, parse error). Replaces the spinner
/// so the tab doesn't sit on "Loading resource…" forever.
pub(super) fn render_drill_in_error_overlay(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    class_name: &str,
    error: &str,
    theme: &lunco_theme::Theme,
) {
    let card_w = 420.0;
    let card_h = 110.0;
    let card_rect = egui::Rect::from_center_size(
        canvas_rect.center(),
        egui::vec2(card_w, card_h),
    );
    let painter = ui.painter().clone().with_clip_rect(ui.clip_rect().intersect(canvas_rect));
    let painter = &painter;
    let shadow = theme.colors.base.alpha(100);
    painter.rect_filled(
        card_rect.translate(egui::vec2(0.0, 3.0)),
        8.0,
        shadow,
    );
    painter.rect_filled(card_rect, 8.0, theme.tokens.surface_raised);
    painter.rect_stroke(
        card_rect,
        8.0,
        egui::Stroke::new(1.0, theme.tokens.error),
        egui::StrokeKind::Outside,
    );
    painter.text(
        egui::pos2(card_rect.min.x + 16.0, card_rect.min.y + 16.0),
        egui::Align2::LEFT_TOP,
        "Failed to load resource",
        egui::FontId::proportional(14.0),
        theme.tokens.error,
    );
    let display = if class_name.len() > 56 {
        format!("…{}", &class_name[class_name.len() - 55..])
    } else {
        class_name.to_string()
    };
    painter.text(
        egui::pos2(card_rect.min.x + 16.0, card_rect.min.y + 38.0),
        egui::Align2::LEFT_TOP,
        display,
        egui::FontId::monospace(11.0),
        theme.tokens.text_subdued,
    );
    let trimmed = if error.len() > 220 {
        format!("{}…", &error[..219])
    } else {
        error.to_string()
    };
    let galley = painter.layout(
        trimmed,
        egui::FontId::proportional(11.0),
        theme.tokens.text,
        card_w - 32.0,
    );
    painter.galley(
        egui::pos2(card_rect.min.x + 16.0, card_rect.min.y + 58.0),
        galley,
        theme.tokens.text,
    );
}


// ─── Empty-diagram summary ──────────────────────────────────────────

/// When the canvas scene has no nodes — common for equation-only
/// leaf models (Battery, RocketEngine, BouncyBall, SpringMass) and
/// MSL building blocks (Integrator, Resistor, Inertia) — paint a
/// "data sheet" card in the centre of the canvas. Treats the class
/// as a first-class display object instead of leaving the user
/// staring at the blank grid.
///
/// Card layout:
/// 1. **Hero strip** — the class's authored `Icon(graphics={...})`
///    annotation rendered via [`crate::icon_paint::paint_graphics`].
///    For classes without one, a stylised type-badge (M / B / C / …).
/// 2. **Heading** — class name + type label.
/// 3. **Symbol bands** — named parameters / inputs / outputs (top 6
///    each). Names beat counts: "tau, J, c" tells the user what the
///    model is for; "3 parameters" doesn't.
/// 4. **Footer counts** — equations + connect equations as a one-
///    line summary, plus a hint that points at the Text tab.
pub(super) fn render_empty_diagram_overlay(
    ui: &mut egui::Ui,
    canvas_rect: egui::Rect,
    world: &mut World,
) {
    let active = world
        .get_resource::<lunco_workbench::WorkspaceResource>()
        .and_then(|ws| ws.active_document);
    let Some(doc) = active else { return };
    let registry = world.resource::<crate::ui::state::ModelicaDocumentRegistry>();
    let Some(host) = registry.host(doc) else { return };
    let document = host.document();
    let theme = world
        .get_resource::<lunco_theme::Theme>()
        .cloned()
        .unwrap_or_else(lunco_theme::Theme::dark);
    let class_name = document
        .strict_ast()
        .and_then(|ast| crate::ast_extract::extract_model_name_from_ast(&ast))
        .unwrap_or_else(|| "(unnamed)".into());

    // Read counts from the per-doc Index. Falls back to all-zeros
    // when the document hasn't installed yet (e.g. drill-in tab still
    // loading) or the active class isn't in the index.
    let counts = {
        let active_doc = active_doc_from_world(world);
        let drilled = active_doc.and_then(|doc| {
            crate::ui::panels::model_view::drilled_class_for_doc(world, doc)
        });
        let registry = world.resource::<ModelicaDocumentRegistry>();
        active_doc
            .and_then(|doc| registry.host(doc))
            .and_then(|host| {
                let document = host.document();
                let index = document.index();
                let qualified = drilled.clone().or_else(|| {
                    index
                        .classes
                        .values()
                        .find(|c| !matches!(c.kind, crate::index::ClassKind::Package))
                        .map(|c| c.name.clone())
                })?;
                let _ = document;
                Some(empty_overlay_counts_from_index(index, &qualified))
            })
            .unwrap_or_default()
    };

    // Pull the live class info out of the document registry so we
    // can show real symbol names + (when authored) the class's own
    // `Icon` graphics. This is the same AST the canvas projector
    // already holds, so we don't pay a re-parse.
    let active_doc = active_doc_from_world(world);
    let (icon, class_type, description, param_names, input_names, output_names) =
        empty_overlay_class_info(world, active_doc, &class_name);

    crate::ui::panels::placeholder::render_centered_card(
        ui,
        canvas_rect,
        egui::vec2(440.0, 360.0),
        &theme,
        |child| {
            // ── Hero strip ────────────────────────────────────────
            // Either the authored icon or a stylised type badge.
            let hero_size = egui::vec2(120.0, 80.0);
            let (_, hero_rect) = child.allocate_space(hero_size);
            if let Some(icon) = &icon {
                crate::icon_paint::paint_graphics(
                    child.painter(),
                    hero_rect,
                    icon.coordinate_system,
                    &icon.graphics,
                );
            } else {
                paint_class_type_badge(
                    child.painter(),
                    hero_rect,
                    class_type.unwrap_or("model"),
                    &theme,
                );
            }
            child.add_space(8.0);

            // ── Class name + type label ───────────────────────────
            child.label(
                egui::RichText::new(&class_name)
                    .strong()
                    .size(15.0)
                    .color(theme.text_heading()),
            );
            if let Some(t) = class_type {
                child.label(
                    egui::RichText::new(t)
                        .size(10.5)
                        .italics()
                        .color(theme.text_muted()),
                );
            }
            if let Some(desc) = &description {
                child.add_space(4.0);
                child.label(
                    egui::RichText::new(desc)
                        .size(11.0)
                        .color(theme.tokens.text),
                );
            }
            child.add_space(8.0);
            child.separator();
            child.add_space(6.0);

            // ── Named symbol bands ───────────────────────────────
            paint_symbol_band(child, "Parameters", &param_names, counts.params, &theme);
            paint_symbol_band(child, "Inputs", &input_names, counts.inputs, &theme);
            paint_symbol_band(child, "Outputs", &output_names, counts.outputs, &theme);

            child.add_space(6.0);
            child.label(
                egui::RichText::new(format!(
                    "{} equations · {} connect equations",
                    counts.equations, counts.connects,
                ))
                .small()
                .color(theme.text_muted()),
            );
            child.add_space(4.0);
            child.label(
                egui::RichText::new("→ Switch to the Text tab to read / edit the source.")
                    .italics()
                    .size(10.0)
                    .color(theme.text_muted()),
            );
        },
    );
}

/// Pull human-friendly info about the active class: authored Icon,
/// type keyword (`model`/`block`/…), description string, and the top
/// few parameter / input / output names. Falls back to `None`/empty
/// vectors silently when the registry doesn't have the doc.
pub(super) fn empty_overlay_class_info(
    world: &mut World,
    doc_id: Option<lunco_doc::DocumentId>,
    class_name: &str,
) -> (
    Option<crate::annotations::Icon>,
    Option<&'static str>,
    Option<String>,
    Vec<String>,
    Vec<String>,
    Vec<String>,
) {
    let Some(doc) = doc_id else {
        return (None, None, None, vec![], vec![], vec![]);
    };
    let registry = world.resource::<ModelicaDocumentRegistry>();
    let Some(host) = registry.host(doc) else {
        return (None, None, None, vec![], vec![], vec![]);
    };
    let document = host.document();
    let Some(ast_arc) = document.strict_ast() else {
        return (None, None, None, vec![], vec![], vec![]);
    };

    // Locate the class. Prefer an exact name match; fall back to the
    // first non-package class (matches `extract_model_name`).
    let class_def = locate_class(&ast_arc, class_name);
    let Some(class) = class_def else {
        return (None, None, None, vec![], vec![], vec![]);
    };

    use rumoca_compile::parsing::ast::Causality;
    use rumoca_compile::parsing::ClassType;

    // Engine-driven Icon merge: hand the qualified class path to
    // [`crate::annotations::extract_icon_via_engine`] which walks
    // the inheritance chain through rumoca's session — scope-chain
    // resolution, cross-file walks, and `visible=...` filtering all
    // happen inside the engine. Replaces the prior resolver-lambda
    // pattern that mirrored `register_local_class`'s lookup logic
    // by hand.
    let class_context = match ast_arc.within.as_ref() {
        Some(within) => {
            let pkg = within
                .name
                .iter()
                .map(|t| t.text.as_ref())
                .collect::<Vec<_>>()
                .join(".");
            if pkg.is_empty() {
                class_name.to_string()
            } else {
                format!("{pkg}.{class_name}")
            }
        }
        None => class_name.to_string(),
    };
    // Engine owns icon resolution (cached, AST-aware).
    let icon = world
        .get_resource::<crate::engine_resource::ModelicaEngineHandle>()
        .and_then(|handle| handle.lock().icon_for(&class_context));
    let class_type = match class.class_type {
        ClassType::Model => Some("model"),
        ClassType::Block => Some("block"),
        ClassType::Class => Some("class"),
        ClassType::Connector => Some("connector"),
        ClassType::Record => Some("record"),
        ClassType::Type => Some("type"),
        ClassType::Package => Some("package"),
        ClassType::Function => Some("function"),
        ClassType::Operator => Some("operator"),
    };
    let description: Option<String> = class
        .description
        .iter()
        .next()
        .map(|t| t.text.as_ref().trim_matches('"').to_string())
        .filter(|s| !s.is_empty());

    let mut params = Vec::new();
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    for (name, comp) in class.components.iter() {
        use rumoca_compile::parsing::ast::Variability;
        if matches!(comp.variability, Variability::Parameter(_)) {
            params.push(name.clone());
        }
        match comp.causality {
            Causality::Input(_) => inputs.push(name.clone()),
            Causality::Output(_) => outputs.push(name.clone()),
            _ => {}
        }
    }

    (icon, class_type, description, params, inputs, outputs)
}

/// Resolve `name` (short or fully-qualified) to a class in `ast`,
/// falling back to the first non-package class when nothing matches.
///
/// Lookup is delegated to `crate::diagram::find_class_by_qualified_name`,
/// which handles both short names (`"PID"`) and qualified names with
/// within-clause tolerance (`"Modelica.Blocks.PID"`). The previous
/// hand-rolled walk only matched literal IndexMap keys, so qualified
/// input always fell through to the picker silently — the lookup
/// half of the function was effectively dead. The fallback (first
/// non-package class) is kept explicit because callers do pass it
/// `extract_model_name_from_ast`'s output and rely on the picker
/// when that name isn't structurally findable.
pub(super) fn locate_class<'a>(
    ast: &'a rumoca_compile::parsing::ast::StoredDefinition,
    name: &str,
) -> Option<&'a rumoca_compile::parsing::ast::ClassDef> {
    if let Some(c) = crate::diagram::find_class_by_qualified_name(ast, name) {
        return Some(c);
    }
    use rumoca_compile::parsing::ClassType;
    ast.classes
        .iter()
        .find(|(_, c)| !matches!(c.class_type, ClassType::Package))
        .map(|(_, c)| c)
}

/// Render a row showing a symbol band (e.g. "Parameters: tau, J, c
/// + 3 more"). When the names list is empty, falls through to "—".
pub(super) fn paint_symbol_band(
    ui: &mut egui::Ui,
    label: &str,
    names: &[String],
    total: usize,
    theme: &lunco_theme::Theme,
) {
    if total == 0 && names.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{label}:"))
                .small()
                .color(theme.text_muted()),
        );
        let shown = names.iter().take(6).cloned().collect::<Vec<_>>().join(", ");
        let suffix = if total > shown.len() && total > names.len().min(6) && names.len() > 6 {
            format!(" + {} more", total - 6)
        } else {
            String::new()
        };
        let display = if shown.is_empty() {
            format!("({total})")
        } else {
            format!("{shown}{suffix}")
        };
        ui.monospace(
            egui::RichText::new(display)
                .small()
                .color(theme.tokens.accent),
        );
    });
}

/// Stylised type badge used as the hero when a class has no authored
/// `Icon` annotation. A centred coloured pill with a single uppercase
/// letter — matches the [`crate::ui::browser_section`] type-badge
/// palette so the canvas hero and the browser row read as the same
/// "this is a model" affordance.
pub(super) fn paint_class_type_badge(
    painter: &egui::Painter,
    rect: egui::Rect,
    type_name: &str,
    theme: &lunco_theme::Theme,
) {
    let letter = match type_name {
        "model" => "M",
        "block" => "B",
        "class" => "C",
        "connector" => "X",
        "record" => "R",
        "type" => "T",
        "package" => "P",
        "function" => "F",
        _ => "?",
    };
    let bg = theme.class_badge_bg_by_keyword(type_name);
    let pill_w = rect.width().min(rect.height() * 1.4);
    let pill_h = rect.height().min(120.0);
    let pill = egui::Rect::from_center_size(rect.center(), egui::vec2(pill_w, pill_h));
    painter.rect_filled(pill, 16.0, bg);
    painter.text(
        pill.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        egui::FontId::proportional(pill_h * 0.55),
        theme.class_badge_fg(),
    );
}

/// Counts for the empty-diagram overlay. All four numbers come from
/// the per-doc [`crate::index::ModelicaIndex`]: components by
/// variability / causality, connections by class, and the class's
/// `equation_count` populated during rebuild.
///
/// O(components_in_class) per call. The Index is rebuilt on every
/// successful parse install, so callers see fresh counts as soon
/// as the AST refreshes — same staleness contract as every other
/// Index reader.
#[derive(Clone, Copy, Default)]
pub(super) struct EmptyOverlayCounts {
    params: usize,
    inputs: usize,
    outputs: usize,
    equations: usize,
    connects: usize,
}

pub(super) fn empty_overlay_counts_from_index(
    index: &crate::index::ModelicaIndex,
    qualified: &str,
) -> EmptyOverlayCounts {
    use crate::index::{Causality, Variability};
    let mut counts = EmptyOverlayCounts::default();
    for comp in index.components_in_class(qualified) {
        if matches!(comp.variability, Variability::Parameter) {
            counts.params += 1;
        }
        match comp.causality {
            Causality::Input => counts.inputs += 1,
            Causality::Output => counts.outputs += 1,
            Causality::None => {}
        }
    }
    counts.connects = index.connections_in_class(qualified).count();
    counts.equations = index
        .classes
        .get(qualified)
        .map(|e| e.equation_count)
        .unwrap_or(0);
    counts
}
