//! Tools palette panel — `lunco-workbench::Panel` implementation.
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

/// Typed UI intent for the editor-owned terrain/script tool resources.
#[derive(Event)]
pub(crate) enum TerrainUiAction {
    /// Update the brush selection and numeric parameters.
    SetTerrain {
        tool: TerrainTool,
        radius: f32,
        strength: f32,
    },
    /// Arm or disarm a named script tool.
    SetScriptTool(Option<String>),
}

pub(crate) fn on_terrain_ui_action(
    trigger: On<TerrainUiAction>,
    mut terrain: ResMut<TerrainToolState>,
    mut script: ResMut<lunco_core::ArmedScriptTool>,
) {
    match &trigger.event() {
        TerrainUiAction::SetTerrain {
            tool,
            radius,
            strength,
        } => {
            terrain.tool = *tool;
            terrain.radius = *radius;
            terrain.strength = *strength;
        }
        TerrainUiAction::SetScriptTool(name) => script.0 = name.clone(),
    }
}

/// Tools palette — arms terrain brushes and sizes them.
pub struct ToolsPanel;

impl Panel for ToolsPanel {
    fn id(&self) -> PanelId {
        PanelId("tools_palette")
    }
    fn title(&self) -> String {
        "Tools".into()
    }
    fn default_slot(&self) -> PanelSlot {
        PanelSlot::SideBrowser
    }
    fn menu_group(&self) -> lunco_workbench::PanelMenuGroup {
        lunco_workbench::PanelMenuGroup::Scene
    }
    fn transparent_background(&self) -> bool {
        true
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut PanelCtx) {
        let Some(tokens) = ctx
            .resource::<lunco_theme::Theme>()
            .map(|theme| theme.tokens.clone())
        else {
            return;
        };
        ctx.panel_content_frame().show(ui, |ui| {
            terrain_section(ui, ctx, &tokens);
            ui.add_space(10.0);
            script_tools_section(ui, ctx, &tokens);
        });
    }
}

fn terrain_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, tokens: &lunco_theme::DesignTokens) {
    ui.heading("Terrain");

    // Snapshot current state; writes are emitted as typed UI actions.
    let (tool, mut radius, mut strength) = ctx
        .resource::<TerrainToolState>()
        .map(|s| (s.tool, s.radius, s.strength))
        .unwrap_or((TerrainTool::None, 5.0, 0.5));

    ui.horizontal(|ui| {
        tool_button(ui, ctx, tokens, "Sculpt", TerrainTool::Sculpt, tool)
            .on_hover_text("Left-click raises · Alt+click digs · Ctrl+click flattens");
        tool_button(ui, ctx, tokens, "Flatten", TerrainTool::Flatten, tool)
            .on_hover_text("Left-click levels the surface to the clicked height");
        tool_button(ui, ctx, tokens, "Crater", TerrainTool::Crater, tool)
            .on_hover_text("Left-click stamps one impact crater (rim radius = brush radius)");
        tool_button(ui, ctx, tokens, "Rock", TerrainTool::Rock, tool)
            .on_hover_text("Left-click places one boulder (radius = brush radius)");
        if tool != TerrainTool::None && ui.button("Off").clicked() {
            set_tool(ctx, TerrainTool::None);
        }
    });

    ui.add_space(4.0);

    // Brush parameters — mutated locally, then deferred back if changed.
    let r0 = radius;
    ui.add(
        egui::Slider::new(&mut radius, 0.5..=200.0)
            .text("Radius (m)")
            .logarithmic(true),
    );
    if (radius - r0).abs() > f32::EPSILON {
        ctx.trigger(TerrainUiAction::SetTerrain {
            tool,
            radius,
            strength,
        });
    }
    let s0 = strength;
    ui.add(
        egui::Slider::new(&mut strength, 0.05..=50.0)
            .text("Strength (m)")
            .logarithmic(true),
    );
    if (strength - s0).abs() > f32::EPSILON {
        ctx.trigger(TerrainUiAction::SetTerrain {
            tool,
            radius,
            strength,
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

/// SCRIPT-AUTHORED tools — one button per registered tool exposing
/// `on_click/1`, straight from the [`lunco_tools`] registry.
///
/// Nothing here knows what any of them do. A `.rhai` dropped into
/// `assets/scripting/tools/` with an `on_click(id)` appears as a button; delete
/// the file and the button goes. That is the whole contract, and it is why this
/// section has no per-tool code the way the terrain brushes above do.
///
/// Labels come from the tool itself when it says so. `ui_label/0` and
/// `ui_hint/0` are rhai functions, and calling them needs the script runtime
/// (which the panel does not have — it paints), so the button falls back to the
/// tool's own name, Title-cased. Reading those two is worth doing once the
/// palette can hold a script result; the arming contract does not change.
fn script_tools_section(ui: &mut egui::Ui, ctx: &mut PanelCtx, tokens: &lunco_theme::DesignTokens) {
    let tools = lunco_tools::ui_click_tools();
    if tools.is_empty() {
        return;
    }
    ui.heading("Tools");

    let armed = ctx
        .resource::<lunco_core::ArmedScriptTool>()
        .and_then(|a| a.0.clone());

    ui.horizontal_wrapped(|ui| {
        for tool in &tools {
            let is_armed = armed.as_deref() == Some(tool.name.as_str());
            let label = title_case(&tool.name);
            let text = label;
            let btn = egui::Button::new(text);
            let btn = if is_armed {
                btn.fill(tokens.success_subdued)
            } else {
                btn
            };
            let name = tool.name.clone();
            if ui
                .add(btn)
                .on_hover_text(format!("{}::on_click — click a scene object", tool.name))
                .clicked()
            {
                ctx.trigger(TerrainUiAction::SetScriptTool(if is_armed {
                    None
                } else {
                    Some(name)
                }));
            }
        }
    });

    ui.add_space(4.0);
    match armed {
        Some(name) => {
            ui.small(
                egui::RichText::new(format!("{name} armed — click an object."))
                    .color(tokens.success),
            );
        }
        None => {
            ui.small("Script-authored — each is a .rhai in assets/scripting/tools/.");
        }
    }
    ui.small("Esc — off");
}

/// `"recover"` → `"Recover"`. Tool names are file stems, so this is the whole
/// of the display transform until a tool declares its own `ui_label`.
fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
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
    let text = label.to_string();
    let btn = egui::Button::new(text);
    let btn = if selected {
        btn.fill(tokens.success_subdued)
    } else {
        btn
    };
    let resp = ui.add(btn);
    if resp.clicked() {
        // Toggle: clicking the armed tool disarms it.
        set_tool(ctx, if selected { TerrainTool::None } else { which });
    }
    resp
}

fn set_tool(ctx: &mut PanelCtx, tool: TerrainTool) {
    let (radius, strength) = ctx
        .resource::<TerrainToolState>()
        .map(|s| (s.radius, s.strength))
        .unwrap_or((5.0, 0.5));
    ctx.trigger(TerrainUiAction::SetTerrain {
        tool,
        radius,
        strength,
    });
}
