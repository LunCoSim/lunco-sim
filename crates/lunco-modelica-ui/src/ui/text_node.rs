//! Editable diagram-text scene node.
//!
//! Modelica `Diagram(graphics={Text(...)})` annotations (callout
//! labels, section headings, etc.) are rendered as first-class
//! canvas Nodes so the user can drag, resize, delete, and rename
//! them with the same affordances component icons use. Source
//! round-trips through the `SetDiagramTextExtent` /
//! `SetDiagramTextString` / `RemoveDiagramText` ops that
//! `document.rs` defines.
//!
//! The visual side is intentionally tiny: paint the string inside
//! the node's rect with a chosen font size (per-Text override) or
//! auto-fit when the source omits `fontSize=`. No DynamicSelect
//! support yet — this kind is for static labels only; dynamic
//! captions stay in the icon-internal text path that already
//! handles `DynamicSelect`.

use bevy_egui::egui;
use lunco_canvas::visual::{DrawCtx, NodeVisual};
use lunco_canvas::Node;

/// Stable kind id used in `Node::kind` and the `VisualRegistry`.
pub const TEXT_NODE_KIND: &str = "lunco.modelica.text";

/// Per-node payload for an editable diagram Text. `data: Arc<dyn
/// Any>` on a canvas Node is `Arc<TextNodeData>`.
#[derive(Debug, Clone, Default)]
pub struct TextNodeData {
    /// The literal string drawn inside the rect. `%name` and other
    /// Modelica substitutions are *not* resolved here — they're
    /// only meaningful in icon contexts, not on the diagram layer.
    pub text: String,
    /// Modelica `fontSize` arg in points. `0` means auto-fit (the
    /// MLS default — text scales to fill the extent height).
    pub font_size: f64,
    /// Text colour as RGB 0..255. `None` falls back to the theme's
    /// default text colour.
    pub color: Option<[u8; 3]>,
}

/// `NodeVisual` reconstructed from `TextNodeData` by the registry.
pub struct TextNodeVisual {
    pub data: TextNodeData,
}

impl TextNodeVisual {
    pub fn from_data(data: TextNodeData) -> Self {
        Self { data }
    }
}

impl NodeVisual for TextNodeVisual {
    fn draw(&self, ctx: &mut DrawCtx, node: &Node, selected: bool) {
        let screen_rect = ctx
            .viewport
            .world_rect_to_screen(node.rect, ctx.screen_rect);
        let egui_rect = egui::Rect::from_min_max(
            egui::pos2(screen_rect.min.x, screen_rect.min.y),
            egui::pos2(screen_rect.max.x, screen_rect.max.y),
        );
        // Selection halo first so the text reads on top.
        if selected {
            ctx.ui.painter().rect_stroke(
                egui_rect,
                3.0,
                egui::Stroke::new(1.5, egui::Color32::from_rgb(70, 160, 240)),
                egui::StrokeKind::Outside,
            );
        }
        // Font size: explicit value (in points) when the source
        // sets `fontSize=`, else auto-fit to the rect's pixel
        // height. The MLS auto-fit factor is "draw at the extent
        // height," which is what we approximate.
        let color = self
            .data
            .color
            .map(|[r, g, b]| egui::Color32::from_rgb(r, g, b))
            .unwrap_or_else(|| ctx.ui.visuals().text_color());
        // MLS §18: `fontSize=0` means "auto-fit to extent". OMEdit /
        // Dymola interpret this as fit BOTH dimensions: start at the
        // extent height (the natural cap height for one-line text),
        // then shrink uniformly if the rendered text width exceeds
        // the extent width. Without the width shrink, wide labels
        // like "reference speed generation" in a 67×8 extent would
        // be sized to the 8-unit height (a font far too large for
        // the 67-unit width) and wrap onto two lines past the
        // authored frame.
        let mut font_px = if self.data.font_size > 0.0 {
            let zoom = (egui_rect.height() / node.rect.height().max(1e-3)).max(0.05);
            (self.data.font_size as f32 * zoom).max(1.0)
        } else {
            (egui_rect.height() * 0.9).max(1.0)
        };
        if egui_rect.width() > 0.5 && font_px > 1.0 {
            let measure = ctx.ui.painter().layout_no_wrap(
                self.data.text.clone(),
                egui::FontId::proportional(font_px),
                color,
            );
            let measured_w = measure.size().x;
            if measured_w > egui_rect.width() {
                font_px = (font_px * egui_rect.width() / measured_w).max(1.0);
            }
        }
        // No-wrap layout: render the text on a single line (already
        // shrunk to fit the extent width above). Wrapping would
        // betray the authored extent box.
        let layout = ctx.ui.painter().layout_no_wrap(
            self.data.text.clone(),
            egui::FontId::proportional(font_px),
            color,
        );
        // Centre the layout inside the rect.
        let pos = egui::pos2(
            egui_rect.center().x - layout.size().x * 0.5,
            egui_rect.center().y - layout.size().y * 0.5,
        );
        ctx.ui.painter().galley(pos, layout, color);
    }
}

/// Convenience: register the kind with a `VisualRegistry`. Call
/// once at plugin-build time alongside other kinds.
pub fn register(reg: &mut lunco_canvas::VisualRegistry) {
    reg.register_node_kind(TEXT_NODE_KIND, |data: &lunco_canvas::NodeData| {
        let payload = data
            .downcast_ref::<TextNodeData>()
            .cloned()
            .unwrap_or_default();
        TextNodeVisual::from_data(payload)
    });
}
