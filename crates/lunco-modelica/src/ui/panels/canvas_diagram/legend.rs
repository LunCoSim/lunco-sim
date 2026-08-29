//! Accessible connection legend for the Modelica canvas.
//!
//! The legend is derived from the rendered scene's typed edge payloads. It
//! therefore documents only connector types that are actually present and
//! uses the same colour mapping as [`super::edge::OrthogonalEdgeVisual`].
//! It is a non-interactive foreground overlay, so it cannot steal canvas
//! navigation or selection input.

use std::collections::BTreeMap;

use bevy_egui::egui;
use lunco_canvas::Scene;

use super::edge::ConnectionEdgeData;
use super::paint::wire_style_for;
use super::theme::modelica_icon_palette_from_ctx;

#[derive(Clone, Copy)]
struct LegendEntry {
    color: egui::Color32,
    kind: crate::visual_diagram::PortKind,
    domain: &'static str,
}

/// Paint a legend for the connection styles used by `scene`.
pub(super) fn render(ui: &egui::Ui, rect: egui::Rect, scene: &Scene, show_edges: bool) {
    if !show_edges {
        return;
    }
    let mut entries = BTreeMap::<String, LegendEntry>::new();
    for (_, edge) in scene.edges() {
        if edge.kind != "modelica.connection" {
            continue;
        }
        let Some(data) = edge.data.downcast_ref::<ConnectionEdgeData>() else {
            continue;
        };
        let connector = data.connector_type.clone();
        let style = wire_style_for(&connector);
        entries.entry(connector).or_insert(LegendEntry {
            color: data.icon_color.unwrap_or(style.color),
            kind: data.kind,
            domain: style.domain,
        });
    }

    if entries.is_empty() {
        return;
    }

    let ctx = ui.ctx().clone();
    let theme = lunco_theme::active(&ctx);
    let palette = modelica_icon_palette_from_ctx(&ctx);
    let width = 300.0;
    let left = (rect.right() - width - 12.0).max(rect.left() + 12.0);
    egui::Area::new(egui::Id::new("lunco_modelica_connection_legend"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .fixed_pos(egui::pos2(left, rect.top() + 12.0))
        .show(&ctx, |ui| {
            egui::Frame::new()
                .fill(theme.tokens.surface_raised)
                .stroke(egui::Stroke::new(1.0, theme.tokens.surface_raised_border))
                .corner_radius(8.0)
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_min_width(width - 20.0);
                    ui.label(egui::RichText::new("Connections").strong());
                    ui.label(
                        egui::RichText::new("Colors follow authored connector Icons")
                            .color(theme.tokens.text_subdued)
                            .small(),
                    );
                    ui.label(
                        egui::RichText::new("Hover a wire for endpoint values and flow")
                            .color(theme.tokens.text_subdued)
                            .small(),
                    );
                    ui.add_space(4.0);

                    for (connector, entry) in entries {
                        let leaf = connector
                            .rsplit('.')
                            .next()
                            .filter(|leaf| !leaf.is_empty())
                            .unwrap_or("Unresolved connector");
                        let label =
                            format!("{leaf} · {} · {}", entry.domain, kind_label(entry.kind));
                        ui.horizontal(|ui| {
                            let (swatch, _) = ui
                                .allocate_exact_size(egui::vec2(28.0, 14.0), egui::Sense::hover());
                            let color = palette.remap(entry.color);
                            ui.painter().line_segment(
                                [
                                    egui::pos2(swatch.left(), swatch.center().y),
                                    egui::pos2(swatch.right(), swatch.center().y),
                                ],
                                egui::Stroke::new(2.0, color),
                            );
                            ui.label(label);
                        });
                    }
                });
        });
}

fn kind_label(kind: crate::visual_diagram::PortKind) -> &'static str {
    match kind {
        crate::visual_diagram::PortKind::Input => "input",
        crate::visual_diagram::PortKind::Output => "output",
        crate::visual_diagram::PortKind::Acausal => "acausal",
    }
}

#[cfg(test)]
mod tests {
    use super::kind_label;

    #[test]
    fn legend_explains_typed_port_kinds() {
        use crate::visual_diagram::PortKind;
        assert_eq!(kind_label(PortKind::Input), "input");
        assert_eq!(kind_label(PortKind::Output), "output");
        assert_eq!(kind_label(PortKind::Acausal), "acausal");
    }
}
