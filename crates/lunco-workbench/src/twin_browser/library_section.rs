//! Read-only browser for the engine's bundled source library.

use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use bevy_egui::egui;

use super::{
    path_tree::{build_path_tree, PathTree},
    BrowserCtx, BrowserScope, BrowserSection,
};
use crate::OpenSourceView;

/// Display-ready source entry retained between UI frames.
struct LibraryEntry {
    asset_path: String,
    rel: PathBuf,
    stem: String,
    file_name: String,
}

/// The LunCo Library browser section.
#[derive(Default)]
pub struct LuncoLibrarySection {
    manifest_fingerprint: u64,
    tree: PathTree<LibraryEntry>,
    populated: bool,
}

impl LuncoLibrarySection {
    /// Rebuild only when the immutable bundle manifest changes (not every frame).
    fn refresh(&mut self, manifest: &lunco_assets::discovery::AssetManifest) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        manifest.rels().hash(&mut hasher);
        let fingerprint = hasher.finish();
        if self.populated && self.manifest_fingerprint == fingerprint {
            return;
        }

        let mut assets = lunco_assets::discovery::list_library_assets(manifest);
        assets.sort_by(|a, b| a.rel.cmp(&b.rel));
        self.tree = build_path_tree(assets.into_iter().map(|asset| {
            let rel = PathBuf::from(&asset.rel);
            let file_name = rel
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| asset.stem.clone());
            let path = rel.clone();
            (
                path,
                LibraryEntry {
                    asset_path: asset.asset_path,
                    rel,
                    stem: asset.stem,
                    file_name,
                },
            )
        }));
        self.manifest_fingerprint = fingerprint;
        self.populated = true;
    }
}

impl BrowserSection for LuncoLibrarySection {
    fn id(&self) -> &str {
        "lunco.workbench.library"
    }

    fn title(&self) -> &str {
        "LunCo Library"
    }

    fn scope(&self) -> BrowserScope {
        BrowserScope::Files
    }

    fn default_open(&self) -> bool {
        true
    }

    fn order(&self) -> u32 {
        150
    }

    fn render(&mut self, ui: &mut egui::Ui, ctx: &mut BrowserCtx<'_, '_>) {
        let Some(manifest) = ctx.resource::<lunco_assets::discovery::AssetManifest>() else {
            ui.label(
                egui::RichText::new("(library unavailable)")
                    .weak()
                    .italics(),
            );
            return;
        };
        if !manifest.ready() {
            ui.label(egui::RichText::new("Loading library…").weak().italics());
            return;
        }
        self.refresh(manifest);
        if self.tree.files.is_empty() && self.tree.subdirs.is_empty() {
            ui.label(egui::RichText::new("(empty)").weak().italics());
            return;
        }

        let loaded_name = ctx
            .resource::<crate::CurrentSceneName>()
            .map(|scene| scene.0.clone())
            .unwrap_or_default();
        let mut clicked = None;
        render_dir(&self.tree, Path::new(""), &loaded_name, &mut clicked, ui);
        if let Some(asset_path) = clicked {
            ctx.trigger(OpenSourceView { asset_path });
        }
    }
}

fn render_dir(
    node: &PathTree<LibraryEntry>,
    prefix: &Path,
    loaded_name: &str,
    clicked: &mut Option<String>,
    ui: &mut egui::Ui,
) {
    for (directory, child) in &node.subdirs {
        let rel = prefix.join(directory);
        egui::CollapsingHeader::new(format!("📁 {directory}"))
            .id_salt(("library_dir", &rel))
            .default_open(false)
            .show(ui, |ui| render_dir(child, &rel, loaded_name, clicked, ui));
    }
    for asset in &node.files {
        let is_loaded = asset.rel.extension().is_some_and(|ext| ext == "usda")
            && !loaded_name.is_empty()
            && (asset.stem == loaded_name
                || loaded_name
                    .split_whitespace()
                    .next()
                    .is_some_and(|first| first == asset.stem));
        let response = if is_loaded {
            ui.selectable_label(false, format!("● {}", asset.file_name))
        } else {
            ui.selectable_label(false, &asset.file_name)
        };
        if response.clicked() {
            *clicked = Some(asset.asset_path.clone());
        }
    }
}
