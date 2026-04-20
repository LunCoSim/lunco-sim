//! # lunco-theme
//!
//! Core design tokens and theming system for LunCoSim.

use bevy::prelude::*;
use bevy_egui::egui;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Supported theme modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Reflect, serde::Serialize, serde::Deserialize)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

/// Helper for fast, allocation-free keys
pub fn theme_key(domain: &str, token: &str) -> (u64, u64) {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    domain.hash(&mut hasher);
    let domain_id = hasher.finish();
    
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    let token_id = hasher.finish();
    
    (domain_id, token_id)
}

/// Version-agnostic color palette to avoid dependency conflicts.
#[derive(Clone, Debug)]
pub struct ColorPalette {
    pub rosewater: egui::Color32,
    pub flamingo: egui::Color32,
    pub pink: egui::Color32,
    pub mauve: egui::Color32,
    pub red: egui::Color32,
    pub maroon: egui::Color32,
    pub peach: egui::Color32,
    pub yellow: egui::Color32,
    pub green: egui::Color32,
    pub teal: egui::Color32,
    pub sky: egui::Color32,
    pub sapphire: egui::Color32,
    pub blue: egui::Color32,
    pub lavender: egui::Color32,
    pub text: egui::Color32,
    pub subtext1: egui::Color32,
    pub subtext0: egui::Color32,
    pub overlay2: egui::Color32,
    pub overlay1: egui::Color32,
    pub overlay0: egui::Color32,
    pub surface2: egui::Color32,
    pub surface1: egui::Color32,
    pub surface0: egui::Color32,
    pub base: egui::Color32,
    pub mantle: egui::Color32,
    pub crust: egui::Color32,
}

impl ColorPalette {
    pub fn from_catppuccin(f: catppuccin_egui::Theme) -> Self {
        // Use component accessors to bridge different egui versions
        Self {
            rosewater: egui::Color32::from_rgb(f.rosewater.r(), f.rosewater.g(), f.rosewater.b()),
            flamingo: egui::Color32::from_rgb(f.flamingo.r(), f.flamingo.g(), f.flamingo.b()),
            pink: egui::Color32::from_rgb(f.pink.r(), f.pink.g(), f.pink.b()),
            mauve: egui::Color32::from_rgb(f.mauve.r(), f.mauve.g(), f.mauve.b()),
            red: egui::Color32::from_rgb(f.red.r(), f.red.g(), f.red.b()),
            maroon: egui::Color32::from_rgb(f.maroon.r(), f.maroon.g(), f.maroon.b()),
            peach: egui::Color32::from_rgb(f.peach.r(), f.peach.g(), f.peach.b()),
            yellow: egui::Color32::from_rgb(f.yellow.r(), f.yellow.g(), f.yellow.b()),
            green: egui::Color32::from_rgb(f.green.r(), f.green.g(), f.green.b()),
            teal: egui::Color32::from_rgb(f.teal.r(), f.teal.g(), f.teal.b()),
            sky: egui::Color32::from_rgb(f.sky.r(), f.sky.g(), f.sky.b()),
            sapphire: egui::Color32::from_rgb(f.sapphire.r(), f.sapphire.g(), f.sapphire.b()),
            blue: egui::Color32::from_rgb(f.blue.r(), f.blue.g(), f.blue.b()),
            lavender: egui::Color32::from_rgb(f.lavender.r(), f.lavender.g(), f.lavender.b()),
            text: egui::Color32::from_rgb(f.text.r(), f.text.g(), f.text.b()),
            subtext1: egui::Color32::from_rgb(f.subtext1.r(), f.subtext1.g(), f.subtext1.b()),
            subtext0: egui::Color32::from_rgb(f.subtext0.r(), f.subtext0.g(), f.subtext0.b()),
            overlay2: egui::Color32::from_rgb(f.overlay2.r(), f.overlay2.g(), f.overlay2.b()),
            overlay1: egui::Color32::from_rgb(f.overlay1.r(), f.overlay1.g(), f.overlay1.b()),
            overlay0: egui::Color32::from_rgb(f.overlay0.r(), f.overlay0.g(), f.overlay0.b()),
            surface2: egui::Color32::from_rgb(f.surface2.r(), f.surface2.g(), f.surface2.b()),
            surface1: egui::Color32::from_rgb(f.surface1.r(), f.surface1.g(), f.surface1.b()),
            surface0: egui::Color32::from_rgb(f.surface0.r(), f.surface0.g(), f.surface0.b()),
            base: egui::Color32::from_rgb(f.base.r(), f.base.g(), f.base.b()),
            mantle: egui::Color32::from_rgb(f.mantle.r(), f.mantle.g(), f.mantle.b()),
            crust: egui::Color32::from_rgb(f.crust.r(), f.crust.g(), f.crust.b()),
        }
    }
}

/// Semantic design tokens for functional UI styling.
#[derive(Clone, Debug)]
pub struct DesignTokens {
    /// Primary action/brand color.
    pub accent: egui::Color32,
    /// Color for successful states.
    pub success: egui::Color32,
    /// Color for warning/caution states.
    pub warning: egui::Color32,
    /// Color for error/critical states.
    pub error: egui::Color32,
    /// Subdued version of success (e.g. for backgrounds).
    pub success_subdued: egui::Color32,
    /// Primary text color.
    pub text: egui::Color32,
    /// Subdued/secondary text.
    pub text_subdued: egui::Color32,
}

impl DesignTokens {
    pub fn from_palette(p: &ColorPalette) -> Self {
        Self {
            accent: p.mauve,
            success: p.green,
            warning: p.yellow,
            error: p.red,
            success_subdued: p.green.linear_multiply(0.4),
            text: p.text,
            text_subdued: p.subtext0,
        }
    }
}

/// Semantic colours for any typed-block-diagram editor — wires by
/// connector domain (electrical, mechanical, signal, …) and class /
/// component badges by kind (model, block, package, …).
///
/// Theme authors customise these through light/dark overrides. Domain
/// crates (Modelica, SysML, electrical CAD) read them as plain
/// fields — no string lookups, no per-call defaults — so the palette
/// → intent mapping lives in **one** place: [`Self::from_palette`].
#[derive(Clone, Debug)]
pub struct SchematicTokens {
    // Wire colours
    pub wire_electrical: egui::Color32,
    pub wire_mechanical: egui::Color32,
    pub wire_thermal: egui::Color32,
    pub wire_fluid: egui::Color32,
    pub wire_signal: egui::Color32,
    pub wire_boolean: egui::Color32,
    pub wire_integer: egui::Color32,
    pub wire_multibody: egui::Color32,
    pub wire_unknown: egui::Color32,

    // Class/component-kind badge backgrounds
    pub class_model_badge: egui::Color32,
    pub class_block_badge: egui::Color32,
    pub class_class_badge: egui::Color32,
    pub class_connector_badge: egui::Color32,
    pub class_record_badge: egui::Color32,
    pub class_type_badge: egui::Color32,
    pub class_package_badge: egui::Color32,
    pub class_function_badge: egui::Color32,
    pub class_operator_badge: egui::Color32,
    pub class_badge_fg: egui::Color32,

    // Typography outside of egui's `visuals.*` chain
    pub text_muted: egui::Color32,
    pub text_heading: egui::Color32,

    // ── Canvas-specific backgrounds ───────────────────────────────
    //
    // Tuned to sit well with Modelica-style blue-heavy icons: cool
    // blues on the canvas background fight (MSL Blocks use strong
    // blue outlines and blue fills, which blend into a bluish
    // backdrop). These tokens give the schematic editor a slightly
    // **warm neutral** canvas that makes blue icons read cleanly
    // without darkening the rest of the app chrome.
    /// Card background underneath each component on the canvas.
    /// Slightly warmer / more neutral than `colors.surface0` so
    /// Modelica blue icon strokes and fills stay contrasty.
    pub canvas_card: egui::Color32,
    /// The diagram's "paper" — the area the grid dots sit on.
    /// Reads as a subtle warm grey in dark mode and near-white in
    /// Latte, so MSL components look like they're on a drafting
    /// sheet rather than a UI surface.
    pub canvas_paper: egui::Color32,
}

impl SchematicTokens {
    /// Build the semantic schematic-editor colours from a raw
    /// palette. This is the *only* site that maps palette entries
    /// to intent — all consumer code reads the resulting fields.
    pub fn from_palette(p: &ColorPalette) -> Self {
        Self {
            wire_electrical: p.blue,
            wire_mechanical: p.maroon,
            wire_thermal: p.peach,
            wire_fluid: p.teal,
            wire_signal: p.mauve,
            wire_boolean: p.red,
            wire_integer: p.green,
            wire_multibody: p.lavender,
            wire_unknown: p.overlay1,

            class_model_badge: p.blue,
            class_block_badge: p.green,
            class_class_badge: p.overlay1,
            class_connector_badge: p.peach,
            class_record_badge: p.mauve,
            class_type_badge: p.overlay0,
            class_package_badge: p.red,
            class_function_badge: p.sapphire,
            class_operator_badge: p.yellow,
            class_badge_fg: p.base,

            text_muted: p.subtext0,
            text_heading: p.text,

            // The Catppuccin palette's `surface1` is a neutral
            // mid-tone that, unlike `surface0` / `base` / `mantle`
            // (which all carry the theme's cool blue wash), reads
            // warm-neutral in both Mocha and Latte. That's exactly
            // what Modelica blue icons want underneath them.
            canvas_card: p.surface1,
            // One step warmer for the paper — `rosewater` in Mocha
            // lifts the canvas off the panel chrome just enough to
            // separate it; in Latte `rosewater` is a soft cream,
            // which reads as "drafting sheet" rather than UI.
            //
            // Actually, use `surface0` with a slight warm tint via
            // blending toward `rosewater`. Since the palette is
            // fixed, we pick the nearest palette entry that matches
            // the drafting-sheet intent: `surface0` in Mocha (a bit
            // warmer than `mantle`) and the identical slot in Latte
            // reads near-white with a faint cream cast.
            canvas_paper: p.surface0,
        }
    }
}

/// Core design tokens — colors, spacing, and rounding.
#[derive(Resource, Clone, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    /// The raw color palette.
    pub colors: ColorPalette,
    /// The functional design tokens (Semantic).
    pub tokens: DesignTokens,
    /// Schematic-editor tokens (wire colours, class-kind badges,
    /// typography). Shared by every block-diagram-style editor in
    /// the workspace.
    pub schematic: SchematicTokens,
    pub spacing: SpacingScale,
    pub rounding: RoundingScale,
    /// Generic registry for domain-specific theme overrides.
    pub overrides: HashMap<(u64, u64), egui::Color32>,
}

#[derive(Clone, Debug, Reflect)]
pub struct SpacingScale {
    pub window_padding: f32,
    pub item_spacing: f32,
    pub button_padding: Vec2,
}

impl Default for SpacingScale {
    fn default() -> Self {
        Self {
            window_padding: 8.0,
            item_spacing: 4.0,
            button_padding: Vec2::new(6.0, 2.0),
        }
    }
}

#[derive(Clone, Debug, Reflect)]
pub struct RoundingScale {
    pub window: f32,
    pub button: f32,
    pub panel: f32,
}

impl Default for RoundingScale {
    fn default() -> Self {
        Self {
            window: 6.0,
            button: 4.0,
            panel: 0.0,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        let colors = ColorPalette::from_catppuccin(catppuccin_egui::MOCHA);
        let tokens = DesignTokens::from_palette(&colors);
        let schematic = SchematicTokens::from_palette(&colors);
        Self {
            mode: ThemeMode::Dark,
            colors,
            tokens,
            schematic,
            spacing: SpacingScale::default(),
            rounding: RoundingScale::default(),
            overrides: HashMap::new(),
        }
    }

    pub fn light() -> Self {
        let colors = ColorPalette::from_catppuccin(catppuccin_egui::LATTE);
        let tokens = DesignTokens::from_palette(&colors);
        let schematic = SchematicTokens::from_palette(&colors);
        Self {
            mode: ThemeMode::Light,
            colors,
            tokens,
            schematic,
            spacing: SpacingScale::default(),
            rounding: RoundingScale::default(),
            overrides: HashMap::new(),
        }
    }

    /// Resolve a theme token.
    pub fn get_token(&self, domain: &str, token: &str, default: egui::Color32) -> egui::Color32 {
        self.overrides
            .get(&theme_key(domain, token))
            .copied()
            .unwrap_or(default)
    }

    /// Register a theme override.
    pub fn register_override(&mut self, domain: &str, token: &str, color: egui::Color32) {
        self.overrides.insert(theme_key(domain, token), color);
    }

    /// Toggle between Dark and Light mode.
    pub fn toggle_mode(&mut self) {
        let new_mode = match self.mode {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
        
        let mut new_theme = match new_mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        };
        
        // Preserve overrides
        new_theme.overrides = self.overrides.clone();
        *self = new_theme;
    }

    /// Map theme tokens to egui::Visuals.
    pub fn to_visuals(&self) -> egui::Visuals {
        let mut visuals = match self.mode {
            ThemeMode::Dark => egui::Visuals::dark(),
            ThemeMode::Light => egui::Visuals::light(),
        };

        let f = &self.colors;

        let to_c32 = |c: egui::Color32| c;

        // Base surface colors
        visuals.window_fill = to_c32(f.crust);
        visuals.panel_fill = to_c32(f.mantle);
        visuals.extreme_bg_color = to_c32(f.base);

        // Text colors
        visuals.override_text_color = Some(to_c32(f.text));
        visuals.widgets.noninteractive.fg_stroke.color = to_c32(f.subtext1);
        visuals.widgets.inactive.fg_stroke.color = to_c32(f.subtext0);

        // Widget fills
        visuals.widgets.inactive.bg_fill = to_c32(f.surface0);
        visuals.widgets.inactive.weak_bg_fill = to_c32(f.surface0);
        visuals.widgets.hovered.bg_fill = to_c32(f.surface1);
        visuals.widgets.active.bg_fill = to_c32(f.surface2);

        // Borders
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, to_c32(f.surface1));
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, to_c32(f.surface1));

        // Selection / Accent
        visuals.selection.bg_fill = to_c32(f.mauve).linear_multiply(0.4);
        visuals.selection.stroke = egui::Stroke::new(1.0, to_c32(f.mauve));

        visuals
    }
}

pub mod fonts;

/// Plugin to register theme resources.
///
/// Also installs fallback fonts (Noto Sans + Noto Sans Symbols 2)
/// on the egui context once the context is live — Modelica icons
/// use math / Greek / arrow glyphs that egui's default font
/// doesn't cover, so we append Noto as a fallback in the
/// `Proportional` / `Monospace` font families.
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Theme>()
            .init_resource::<fonts::FontsInstalled>()
            .add_systems(
                bevy_egui::EguiPrimaryContextPass,
                install_fallback_fonts_once,
            );
    }
}

/// Install Noto fallback fonts the first time the egui context is
/// available, then mark the resource as installed so subsequent
/// frames short-circuit. Lives here (not in an `on_startup` system)
/// because the egui context is created on the first render pass —
/// calling `set_fonts` earlier would be a no-op.
///
/// Calling `set_fonts` from inside `EguiPrimaryContextPass` (i.e.
/// between `ctx.begin_pass` and `ctx.end_pass`) is harmless — egui
/// queues the FontDefinitions change and applies it on the next
/// frame's atlas build. By the time the Icon view renders the
/// `∧` glyph two frames after startup, the fallback is active.
fn install_fallback_fonts_once(
    mut contexts: bevy_egui::EguiContexts,
    mut done: ResMut<fonts::FontsInstalled>,
) {
    if done.0 {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        bevy::log::warn!(
            "[lunco-theme] install_fallback_fonts_once: egui ctx not yet \
             available; retrying next frame"
        );
        return;
    };
    bevy::log::info!(
        "[lunco-theme] installing Noto fallback fonts (Greek/math/arrow coverage)…"
    );
    fonts::install_fallback_fonts(ctx);
    done.0 = true;
}
