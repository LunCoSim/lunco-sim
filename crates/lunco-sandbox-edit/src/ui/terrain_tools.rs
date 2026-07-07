//! Tools palette panel — WorkbenchPanel implementation.
//!
//! A dockable "🛠 Tools" view holding the in-scene editing tools. Today it hosts
//! the terrain-sculpt brushes; new tools slot in as further sections. Pure
//! presentation: it reads/writes [`TerrainToolState`] (UI-local tool state) and
//! never mutates domain data directly — the actual edits are emitted by the
//! scene-click observer.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_workbench::{Panel, PanelCtx, PanelId, PanelSlot};

use crate::terrain_tools::{TerrainTool, TerrainToolState};

/// Tools palette — arms terrain brushes and sizes them.
pub struct ToolsPanel;

impl Panel for ToolsPanel {
    fn id(&self) -> PanelId { PanelId("tools_palette") }
    fn title(&self) -> String { "🛠 Tools".into() }
    fn default_slot(&self) -> PanelSlot { PanelSlot::SideBrowser }
    fn transparent_background(&self) -> bool { true }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some((mantle, tokens)) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| (theme.colors.mantle, theme.tokens.clone()))
        else {
            return;
        };
        egui::Frame::new()
            .fill(mantle)
            .inner_margin(8.0)
            .corner_radius(4)
            .show(ui, |ui| terrain_section(ui, ctx, &tokens));
    }
}

fn terrain_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, tokens: &lunco_theme::DesignTokens) {
    ui.heading("Terrain");

    // Snapshot current state (the panel can't hold a mutable borrow across
    // paint — it defers writes into the world).
    let (tool, mut radius, mut strength) = ctx
        .resource::<TerrainToolState>()
        .map(|s| (s.tool, s.radius, s.strength))
        .unwrap_or((TerrainTool::None, 5.0, 0.5));

    ui.horizontal(|ui| {
        tool_button(ui, ctx, tokens, "Sculpt", TerrainTool::Sculpt, tool)
            .on_hover_text("Left-click raises · Alt+click digs · Ctrl+click flattens");
        tool_button(ui, ctx, tokens, "Flatten", TerrainTool::Flatten, tool)
            .on_hover_text("Left-click levels the surface to the clicked height");
        if tool != TerrainTool::None && ui.button("Off").clicked() {
            set_tool(ctx, TerrainTool::None);
        }
    });

    ui.add_space(4.0);

    // Brush parameters — mutated locally, then deferred back if changed.
    let r0 = radius;
    ui.add(egui::Slider::new(&mut radius, 0.5..=200.0).text("Radius (m)").logarithmic(true));
    if (radius - r0).abs() > f32::EPSILON {
        ctx.defer(move |world| {
            if let Some(mut s) = world.get_resource_mut::<TerrainToolState>() { s.radius = radius; }
        });
    }
    let s0 = strength;
    ui.add(egui::Slider::new(&mut strength, 0.05..=50.0).text("Strength (m)").logarithmic(true));
    if (strength - s0).abs() > f32::EPSILON {
        ctx.defer(move |world| {
            if let Some(mut s) = world.get_resource_mut::<TerrainToolState>() { s.strength = strength; }
        });
    }

    ui.separator();
    if tool == TerrainTool::None {
        ui.small("Pick a brush, then click the terrain to sculpt it.");
    } else {
        ui.small(egui::RichText::new("Brush armed — click the terrain.").color(tokens.success));
    }
    ui.small("Shift + ↑/↓ or Shift+scroll — brush radius");
    ui.small("Alt + ↑/↓ or Alt+scroll — brush strength");
    ui.small("Alt+click — dig · Ctrl+click — flatten · Esc — off");
}

/// A toggle-style tool button; highlights when it's the armed tool.
fn tool_button(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    tokens: &lunco_theme::DesignTokens,
    label: &str,
    which: TerrainTool,
    current: TerrainTool,
) -> egui::Response {
    let selected = current == which;
    let text = if selected { format!("✓ {label}") } else { label.to_string() };
    let btn = egui::Button::new(text);
    let btn = if selected { btn.fill(tokens.success_subdued) } else { btn };
    let resp = ui.add(btn);
    if resp.clicked() {
        // Toggle: clicking the armed tool disarms it.
        set_tool(ctx, if selected { TerrainTool::None } else { which });
    }
    resp
}

fn set_tool(ctx: &mut PanelCtx, tool: TerrainTool) {
    ctx.defer(move |world| {
        if let Some(mut s) = world.get_resource_mut::<TerrainToolState>() {
            s.tool = tool;
        }
    });
}
