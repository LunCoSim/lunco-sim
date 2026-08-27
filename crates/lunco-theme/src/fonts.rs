//! Font installation — registers **DejaVu Sans** as a fallback
//! font on the egui context so Modelica icons can render math /
//! Greek / arrow / logic-operator glyphs without tofu boxes.
//!
//! # Why DejaVu Sans
//!
//! egui's default proportional font (Ubuntu-Light) covers Latin and
//! basic punctuation but tofus on `∧ ∨ Δ ρ φ ← →` — all of which
//! Modelica icons use heavily (Blocks.MathBoolean's `∧`, Thermal's
//! Δ, Magnetic's φ, signal-flow arrows).
//!
//! An earlier iteration of this module tried **Noto Sans +
//! Noto Sans Symbols 2** (Google's "no tofu" fonts). Verification
//! via `fc-query` showed both have holes in the Mathematical
//! Operators block (U+2200-22FF): Noto Sans covers up to U+21FF
//! (ends before math), Noto Sans Symbols 2 has sparse math
//! coverage (only a handful in U+22xx). Neither covers the common
//! `∧ ∨` logical operators.
//!
//! **DejaVu Sans** covers U+2190-U+2311 contiguously — arrows +
//! the full Mathematical Operators block + Miscellaneous
//! Technical — in a single file. This is why Godot and Blender
//! ship DejaVu for the same purpose.
//!
//! The file is an engine asset named `fonts/DejaVuSans.ttf`. Native builds
//! resolve it through the shared asset roots, including the packed cache in a
//! release bundle; web builds fetch the copy staged by `build_web.sh`.
//!
//! # Fallback order
//!
//! egui walks a font family's Vec in order and uses the first entry
//! that has the glyph. We *append* DejaVu after the default, so
//! regular UI text renders in Ubuntu-Light as before — only the
//! rare characters (math, Greek, arrows) fall through to DejaVu.

use bevy_egui::egui;

/// Marker resource: set after the fonts are installed, so the
/// install system short-circuits on every subsequent frame.
#[derive(bevy::prelude::Resource, Default)]
pub struct FontsInstalled(pub bool);

/// Idempotent installer. Native reads the font from the cache dir;
/// wasm callers must hand in pre-fetched bytes via
/// [`install_fallback_fonts_with_bytes`] (see [`crate::spawn_wasm_font_fetch`]).
/// If a development tree is missing the font, the app logs a visible startup
/// error and keeps the engine running so the diagnostic surface can explain the
/// missing package input. Release packaging rejects that state before producing
/// a bundle.
// Native-only (the wasm path hands in pre-fetched bytes via
// `install_fallback_fonts_with_bytes`), one-shot font load at startup —
// a direct `std::fs::read` is correct here, so the `disallowed_methods`
// ban (wasm-safety) is locally allowed rather than routed through an
// async asset load.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_methods)]
pub fn install_fallback_fonts(ctx: &egui::Context) {
    let dejavu = match std::fs::read(lunco_assets::dejavu_sans_path()) {
        Ok(bytes) => bytes,
        Err(e) => {
            bevy::log::warn!(
                "[lunco-theme] DejaVu Sans not found at {}: {e} — math \
                 / Greek / arrow glyphs will tofu. Populate the asset cache \
                 before launching a packaged build.",
                lunco_assets::dejavu_sans_path().display()
            );
            return;
        }
    };
    install_fallback_fonts_with_bytes(ctx, dejavu);
}

/// Wasm stub: there's no filesystem at runtime, so the sync entry
/// point can't read the font. Callers go through
/// [`crate::spawn_wasm_font_fetch`] instead, which fetches over HTTP and then
/// calls [`install_fallback_fonts_with_bytes`] once the bytes land.
#[cfg(target_arch = "wasm32")]
pub fn install_fallback_fonts(_ctx: &egui::Context) {}

/// Same as [`install_fallback_fonts`] but takes the DejaVu bytes
/// directly — used on wasm where the font must be fetched over HTTP at
/// runtime instead of read from disk.
pub fn install_fallback_fonts_with_bytes(ctx: &egui::Context, dejavu: Vec<u8>) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "dejavu".into(),
        std::sync::Arc::new(egui::FontData::from_owned(dejavu)),
    );
    // Append — DejaVu runs after the default font, so common UI text
    // keeps the existing look and only the glyphs the primary font
    // lacks fall through to DejaVu.
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push("dejavu".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("dejavu".into());

    ctx.set_fonts(fonts);
    bevy::log::info!(
        "[lunco-theme] installed DejaVu Sans fallback (covers U+2190-2311: \
         arrows, math operators, misc technical)"
    );
}

/// Wasm-only: fetch `url` over HTTP, then install the result as a
/// fallback font on `ctx`. The page bundle stages the manifest-declared
/// `fonts/DejaVuSans.ttf` artifact beside the web assets. A fetch failure is
/// reported as a startup resource error; it is not treated as a successful
/// font load.
#[cfg(target_arch = "wasm32")]
pub fn spawn_wasm_font_fetch(
    ctx: egui::Context,
    url: String,
    settings: lunco_settings::DownloadSettings,
) {
    wasm_bindgen_futures::spawn_local(async move {
        match lunco_assets::web_fetch::network_fetch_uncached(&url, &settings).await {
            Ok(bytes) => {
                bevy::log::info!(
                    "[lunco-theme] font fetched {url}: {} bytes — installing",
                    bytes.len()
                );
                install_fallback_fonts_with_bytes(&ctx, bytes);
            }
            Err(error) => bevy::log::warn!("[lunco-theme] font fetch {url}: {error}"),
        }
    });
}
