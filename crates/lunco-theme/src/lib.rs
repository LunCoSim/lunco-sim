//! # lunco-theme
//!
//! Core design tokens and theming system for LunCoSim.

use bevy::prelude::*;
use bevy_egui::egui;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Terse "same colour, different alpha" for opaque design tokens.
///
/// Every palette entry / design token in this crate is **opaque**
/// (`Color32::from_rgb`). Overlays (grid dots, shadows, marquees,
/// dashed guides) need the *same hue* at reduced opacity. Before this
/// trait that meant `Color32::from_rgba_unmultiplied(c.r(), c.g(),
/// c.b(), a)` spelled out at every site; now it's just `c.alpha(a)`.
pub trait ColorAlpha {
    /// Return this colour with its alpha set to `a` (0–255), keeping
    /// the RGB hue. Source is assumed opaque (true for all tokens).
    fn alpha(self, a: u8) -> egui::Color32;
}

impl ColorAlpha for egui::Color32 {
    #[inline]
    fn alpha(self, a: u8) -> egui::Color32 {
        egui::Color32::from_rgba_unmultiplied(self.r(), self.g(), self.b(), a)
    }
}

/// egui data-cache id under which the active [`Theme`] is published for
/// the current frame.
fn active_theme_id() -> egui::Id {
    egui::Id::new("lunco_theme.active")
}

/// Publish the active theme into the egui data cache for this frame.
///
/// egui paint callbacks (canvas layers, node/edge painters, overlays)
/// run *outside* the Bevy world, so they can't read `Res<Theme>`. The
/// app calls this **once per frame** (before any egui paint) and every
/// paint helper then reads it back via [`active`]. One theme, one
/// transport — no per-consumer projection structs.
pub fn store_active(ctx: &egui::Context, theme: &Theme) {
    ctx.data_mut(|d| d.insert_temp(active_theme_id(), Arc::new(theme.clone())));
}

/// Read the active theme published this frame, or a dark-mode default
/// (tests, standalone demos, or a frame before the app pushed one).
pub fn active(ctx: &egui::Context) -> Arc<Theme> {
    ctx.data(|d| d.get_temp::<Arc<Theme>>(active_theme_id()))
        .unwrap_or_else(|| Arc::new(Theme::dark()))
}

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
    /// Card / button background — always **lighter** than the panel
    /// fill in both modes so "raised" tiles read as raised. Catppuccin
    /// Latte inverts the surface scale (surface0 < mantle in lightness),
    /// so a naive `surface0` here gives dark cards on a light panel.
    pub surface_raised: egui::Color32,
    /// Inset background (text inputs, code editor gutter) — always
    /// **darker** than the panel fill in both modes for the opposite
    /// "sunken" affordance.
    pub surface_sunken: egui::Color32,
    /// Border for raised tiles. Visible against both the panel and
    /// the raised surface in both modes.
    pub surface_raised_border: egui::Color32,
}

impl DesignTokens {
    pub fn from_palette(p: &ColorPalette, mode: ThemeMode) -> Self {
        let (raised, sunken, raised_border) = match mode {
            // Mocha: surface scale climbs *lighter* than mantle, so
            // surface0 reads as raised and crust as sunken.
            ThemeMode::Dark => (p.surface0, p.crust, p.surface2),
            // Latte: surface scale climbs *darker* than mantle, so we
            // pick `base` (lighter than mantle) for raised and stick
            // with surface0 for sunken (darker than mantle).
            ThemeMode::Light => (p.base, p.surface0, p.overlay0),
        };
        Self {
            accent: p.mauve,
            success: p.green,
            warning: p.yellow,
            error: p.red,
            success_subdued: p.green.linear_multiply(0.4),
            text: p.text,
            text_subdued: p.subtext0,
            surface_raised: raised,
            surface_sunken: sunken,
            surface_raised_border: raised_border,
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
    /// Anchor + invert rules for re-coloring authored Modelica icon
    /// primitives so MSL (designed for paper-white backgrounds) reads
    /// well under the active theme. Identity in light mode.
    pub modelica_icons: ModelicaIconPalette,
    pub spacing: SpacingScale,
    pub rounding: RoundingScale,
    /// Generic registry for domain-specific theme overrides.
    pub overrides: HashMap<(u64, u64), egui::Color32>,
}

/// Color-mapping rules for Modelica icon primitives.
///
/// MSL was authored against a paper-white canvas with black outlines
/// and saturated accents. Rendered untouched on a dark theme, the
/// black outlines vanish into the bg and the bright primary blue/red
/// scream against the muted UI palette.
///
/// This palette runs every authored MSL `Color` through:
///   1. **Anchor table** — canonical MSL colors (pure black, pure
///      white, primary RGB, the gray ramp) get an explicit theme-
///      tuned remap. Match is RGB distance ≤ `anchor_eps`.
///   2. **Luminance invert (HSL)** — anything else flips its lightness
///      while keeping hue + saturation, then clamps to
///      `[invert_l_min, invert_l_max]` so user-authored colors stay
///      readable without losing identity.
///
/// Light theme defaults to identity (no remap) — MSL already looks
/// right on a light background.
#[derive(Clone, Debug)]
pub struct ModelicaIconPalette {
    pub enabled: bool,
    /// `(msl_rgb, target)` pairs. First match wins.
    pub anchors: Vec<(egui::Color32, egui::Color32)>,
    /// L1 RGB distance under which an authored color counts as the
    /// anchor. 8 catches `{0,0,255}` ↔ `{0,0,254}` style near-misses
    /// without colliding distinct hues.
    pub anchor_eps: u8,
    pub invert_l_min: f32,
    pub invert_l_max: f32,
}

impl ModelicaIconPalette {
    /// Identity palette — every color passes through unchanged.
    pub fn identity() -> Self {
        Self {
            enabled: false,
            anchors: Vec::new(),
            anchor_eps: 0,
            invert_l_min: 0.0,
            invert_l_max: 1.0,
        }
    }

    /// Dark-theme defaults: re-color the canonical MSL palette so it
    /// sits naturally on a dark Catppuccin-mocha-ish bg, invert the
    /// rest. Tuned by eye; iterate via theme JSON as needed.
    pub fn dark_default(c: &ColorPalette) -> Self {
        // Anchor RGBs: the values MSL uses everywhere. We match these
        // ±eps and substitute the theme-mapped equivalent.
        let anchors: Vec<(egui::Color32, egui::Color32)> = vec![
            // Pure black — outlines, default text. Map to high-contrast
            // theme text so outlines read on dark bg.
            (egui::Color32::from_rgb(0, 0, 0), c.text),
            // Pure white — default fillColor (the "page" feel). Map to
            // a slightly lighter surface than the canvas bg so MSL
            // bodies (Inertia rectangle, white-fill rects) feel like
            // they sit on a card, not blend into the background.
            (egui::Color32::from_rgb(255, 255, 255), c.surface2),
            // Pure blue — `%name` titles, signal block accents. Use
            // the theme's blue/sapphire so it harmonises with wires.
            (egui::Color32::from_rgb(0, 0, 255), c.sapphire),
            // Pure red — error markers. Slightly desaturated.
            (egui::Color32::from_rgb(255, 0, 0), c.red),
            // MSL "warm red" 191,0,0 — heat ports, thermal indicators.
            (egui::Color32::from_rgb(191, 0, 0), c.maroon),
            // Pure green — sensors, "ok" markers.
            (egui::Color32::from_rgb(0, 255, 0), c.green),
            // Modest green 0,128,0 — same family.
            (egui::Color32::from_rgb(0, 128, 0), c.green),
            // Light gray 192,192,192 — flange axles, cylinder rims.
            // Theme overlay2 is a near-equivalent on dark.
            (egui::Color32::from_rgb(192, 192, 192), c.overlay2),
            // Mid gray 128,128,128 — secondary outlines, hatching.
            (egui::Color32::from_rgb(128, 128, 128), c.overlay1),
            // Dark gray 64,64,64 — secondary text (e.g. parameter
            // labels like `tau`). Map to subtext so it reads.
            (egui::Color32::from_rgb(64, 64, 64), c.subtext0),
            // 95-ish gray (often used for transmission shading).
            (egui::Color32::from_rgb(95, 95, 95), c.overlay0),
            // Lighter "gray ramp" 224,224,224 — light shading.
            (egui::Color32::from_rgb(224, 224, 224), c.surface2),
        ];
        Self {
            enabled: true,
            anchors,
            anchor_eps: 8,
            invert_l_min: 0.18,
            invert_l_max: 0.92,
        }
    }

    /// Apply remap rules to `(r, g, b, a)`. Alpha preserved.
    pub fn remap_rgba(&self, r: u8, g: u8, b: u8, a: u8) -> (u8, u8, u8, u8) {
        if !self.enabled {
            return (r, g, b, a);
        }
        // Anchor pass — first within-eps match wins.
        let eps = self.anchor_eps as i32;
        for (src, dst) in &self.anchors {
            let dr = (src.r() as i32 - r as i32).abs();
            let dg = (src.g() as i32 - g as i32).abs();
            let db = (src.b() as i32 - b as i32).abs();
            if dr.max(dg).max(db) <= eps {
                return (dst.r(), dst.g(), dst.b(), a);
            }
        }
        // Luminance-invert in HSL, clamp to [min,max].
        let (h, s, l) = rgb_to_hsl(r, g, b);
        let mut l_inv = 1.0 - l;
        l_inv = l_inv.clamp(self.invert_l_min, self.invert_l_max);
        let (rr, gg, bb) = hsl_to_rgb(h, s, l_inv);
        (rr, gg, bb, a)
    }

    /// Convenience for `egui::Color32` callers.
    pub fn remap(&self, c: egui::Color32) -> egui::Color32 {
        let (r, g, b, a) = self.remap_rgba(c.r(), c.g(), c.b(), c.a());
        egui::Color32::from_rgba_unmultiplied(r, g, b, a)
    }
}

fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let rf = r as f32 / 255.0;
    let gf = g as f32 / 255.0;
    let bf = b as f32 / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let l = (max + min) * 0.5;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == rf {
        ((gf - bf) / d) + if gf < bf { 6.0 } else { 0.0 }
    } else if max == gf {
        ((bf - rf) / d) + 2.0
    } else {
        ((rf - gf) / d) + 4.0
    } / 6.0;
    (h, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s.abs() < f32::EPSILON {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);
    (
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 { t += 1.0; }
    if t > 1.0 { t -= 1.0; }
    if t < 1.0 / 6.0 { return p + (q - p) * 6.0 * t; }
    if t < 1.0 / 2.0 { return q; }
    if t < 2.0 / 3.0 { return p + (q - p) * (2.0 / 3.0 - t) * 6.0; }
    p
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
        let tokens = DesignTokens::from_palette(&colors, ThemeMode::Dark);
        let schematic = SchematicTokens::from_palette(&colors);
        let modelica_icons = ModelicaIconPalette::dark_default(&colors);
        Self {
            mode: ThemeMode::Dark,
            colors,
            tokens,
            schematic,
            modelica_icons,
            spacing: SpacingScale::default(),
            rounding: RoundingScale::default(),
            overrides: HashMap::new(),
        }
    }

    pub fn light() -> Self {
        let colors = ColorPalette::from_catppuccin(catppuccin_egui::LATTE);
        let tokens = DesignTokens::from_palette(&colors, ThemeMode::Light);
        let schematic = SchematicTokens::from_palette(&colors);
        Self {
            mode: ThemeMode::Light,
            colors,
            tokens,
            schematic,
            modelica_icons: ModelicaIconPalette::identity(),
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

        // Text colors. In light mode the Catppuccin Latte `subtext0`
        // (~#6c6f85) sits at borderline contrast on the lighter
        // surfaces and most secondary labels render gray-on-white. Use
        // `text` for noninteractive and `subtext1` for inactive in
        // light mode to keep button/tab labels legible; dark mode
        // already has plenty of contrast so the original mapping
        // stands.
        visuals.override_text_color = Some(to_c32(f.text));
        let (noninteractive_fg, inactive_fg, hovered_fg, active_fg) = match self.mode {
            ThemeMode::Dark => (f.subtext1, f.subtext0, f.text, f.text),
            ThemeMode::Light => (f.text, f.subtext1, f.text, f.text),
        };
        visuals.widgets.noninteractive.fg_stroke.color = to_c32(noninteractive_fg);
        visuals.widgets.inactive.fg_stroke.color = to_c32(inactive_fg);
        visuals.widgets.hovered.fg_stroke.color = to_c32(hovered_fg);
        visuals.widgets.active.fg_stroke.color = to_c32(active_fg);

        // Widget fills. Use the semantic `surface_raised` token so
        // buttons read as "raised" in both modes — in Latte the raw
        // `surface0` is darker than `mantle` and produces dark
        // buttons on a light panel.
        let raised = self.tokens.surface_raised;
        let raised_hover = match self.mode {
            ThemeMode::Dark => f.surface1,
            // Light: convention says hover gets *darker*; surface0
            // is the closest darker step that still has contrast
            // against the surrounding panel (mantle).
            ThemeMode::Light => f.surface0,
        };
        let raised_active = match self.mode {
            ThemeMode::Dark => f.surface2,
            ThemeMode::Light => f.surface1,
        };
        visuals.widgets.inactive.bg_fill = to_c32(raised);
        visuals.widgets.inactive.weak_bg_fill = to_c32(raised);
        visuals.widgets.hovered.bg_fill = to_c32(raised_hover);
        visuals.widgets.active.bg_fill = to_c32(raised_active);

        // Borders. In light mode `surface1` is barely distinct from
        // `surface0`/`mantle` so widget outlines vanish — bump to
        // `overlay0` for visible separation between adjacent buttons.
        let border = match self.mode {
            ThemeMode::Dark => f.surface1,
            ThemeMode::Light => f.overlay0,
        };
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, to_c32(border));
        visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, to_c32(border));

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
        "[lunco-theme] installing fallback fonts (Greek/math/arrow coverage)…"
    );
    #[cfg(target_arch = "wasm32")]
    {
        // Wasm has no filesystem — fetch the font over HTTP from the
        // page bundle. Fire-and-forget; the install happens on the
        // next frame after the bytes land. Until then egui uses the
        // default font and math/arrow glyphs tofu briefly.
        fonts::spawn_wasm_font_fetch(ctx.clone(), "./fonts/DejaVuSans.ttf".to_string());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        fonts::install_fallback_fonts(ctx);
    }
    done.0 = true;
}
