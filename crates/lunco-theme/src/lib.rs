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

/// Core design tokens — colors, spacing, and rounding.
#[derive(Resource, Clone, Debug)]
pub struct Theme {
    pub mode: ThemeMode,
    /// The raw color palette.
    pub colors: ColorPalette,
    /// The functional design tokens (Semantic).
    pub tokens: DesignTokens,
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
        Self {
            mode: ThemeMode::Dark,
            colors,
            tokens,
            spacing: SpacingScale::default(),
            rounding: RoundingScale::default(),
            overrides: HashMap::new(),
        }
    }

    pub fn light() -> Self {
        let colors = ColorPalette::from_catppuccin(catppuccin_egui::LATTE);
        let tokens = DesignTokens::from_palette(&colors);
        Self {
            mode: ThemeMode::Light,
            colors,
            tokens,
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

/// Plugin to register theme resources.
pub struct ThemePlugin;

impl Plugin for ThemePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Theme>();
    }
}
