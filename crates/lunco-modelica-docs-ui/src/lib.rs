//! Reusable egui renderer for Modelica HTML documentation.
//!
//! Modelica documentation annotations are authored as HTML. This package owns
//! the HTML-to-Markdown conversion cache, CommonMark rendering cache, and URI
//! interception that routes `modelica://` links back through the workbench
//! registry. Document selection and panel layout remain with the Modelica UI.

use bevy_egui::egui;
use lunco_workbench_core::PanelCtx;

/// Render one HTML documentation fragment as cached CommonMark.
///
/// Links handled by the workbench URI registry are converted into typed
/// `UriClicked` events; unrelated links remain ordinary egui output commands.
pub fn render_html_as_markdown(
    ui: &mut egui::Ui,
    ctx: &mut PanelCtx,
    target_width: f32,
    html: &str,
) {
    use std::sync::Mutex;

    static CACHE: std::sync::OnceLock<Mutex<egui_commonmark::CommonMarkCache>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(egui_commonmark::CommonMarkCache::default()));

    static MD_CACHE: std::sync::OnceLock<Mutex<Option<(u64, String)>>> = std::sync::OnceLock::new();
    let md_cache = MD_CACHE.get_or_init(|| Mutex::new(None));

    let html_hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        html.hash(&mut h);
        h.finish()
    };

    let md = {
        let mut guard = md_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((key, value)) = guard.as_ref() {
            if *key == html_hash {
                value.clone()
            } else {
                let value = htmd::convert(html).unwrap_or_else(|_| html.to_string());
                *guard = Some((html_hash, value.clone()));
                value
            }
        } else {
            let value = htmd::convert(html).unwrap_or_else(|_| html.to_string());
            *guard = Some((html_hash, value.clone()));
            value
        }
    };

    if let Ok(mut guard) = cache.lock() {
        egui_commonmark::CommonMarkViewer::new()
            .max_image_width(Some(target_width as usize))
            .show(ui, &mut guard, &md);
    }

    let intercepts: Vec<(usize, String, lunco_workbench_core::uri::UriResolution)> = {
        let registry = ctx.resource::<lunco_workbench_core::uri::UriRegistry>();
        ui.ctx().output_mut(|output| {
            output
                .commands
                .iter()
                .enumerate()
                .filter_map(|(index, command)| {
                    if let egui::OutputCommand::OpenUrl(open) = command {
                        let resolution = registry
                            .map(|registry| registry.dispatch(&open.url))
                            .unwrap_or(lunco_workbench_core::uri::UriResolution::NotHandled);
                        if !matches!(
                            resolution,
                            lunco_workbench_core::uri::UriResolution::NotHandled
                        ) {
                            return Some((index, open.url.clone(), resolution));
                        }
                    }
                    None
                })
                .collect()
        })
    };

    ui.ctx().output_mut(|output| {
        for (index, _, _) in intercepts.iter().rev() {
            if *index < output.commands.len() {
                output.commands.remove(*index);
            }
        }
    });
    for (_, url, resolution) in intercepts {
        ctx.trigger(lunco_workbench_core::uri::UriClicked {
            uri: url,
            resolution,
        });
    }
}
