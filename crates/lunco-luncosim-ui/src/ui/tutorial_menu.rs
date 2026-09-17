//! Application-owned catalog for authored tutorial scenarios.
//!
//! This module is intentionally the only Rust code that knows the word
//! "tutorial". A tutorial is otherwise just a file-backed Rhai scenario plus
//! optional standard USD scene content. The menu projects the authored
//! catalog into the application UI and launches it through the generic
//! `RunScenarioAsset` command.

use bevy::prelude::*;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use bevy_egui::egui;
use lunco_scripting::commands::RunScenarioAsset;
use lunco_scripting::ScenarioReloadPolicy;
use lunco_workbench_core::WorkbenchMenuRegistry;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use super::{SCENARIO_MENU_HEIGHT, SCENARIO_MENU_MAX_WIDTH, SCENARIO_MENU_MIN_WIDTH};

const TUTORIAL_CATALOG_KIND: &str = "lunco.tutorial-catalog.v1";

#[derive(Debug, Clone, Deserialize)]
struct TutorialMenuEntry {
    track: String,
    title: String,
    blurb: String,
    difficulty: String,
    source_asset: String,
    #[serde(default)]
    scene_asset: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TutorialMenuFile {
    #[serde(default)]
    tutorials: Vec<TutorialMenuEntry>,
}

struct PendingBundledCatalog {
    asset_path: String,
    handle: Handle<lunco_assets_core::TextAsset>,
}

struct PendingTwinCatalog {
    twin: String,
    task: Task<Result<Option<TutorialMenuFile>, String>>,
}

#[derive(Resource, Default)]
struct TutorialMenuCatalog {
    entries: Vec<TutorialMenuEntry>,
    error: Option<String>,
    bundled_candidates: Vec<PendingBundledCatalog>,
    bundled_scan_started: bool,
    bundled_ready: bool,
    twin_entries: BTreeMap<String, Vec<TutorialMenuEntry>>,
    loaded_twins: BTreeSet<String>,
    pending_twins: HashMap<String, PendingTwinCatalog>,
}

pub(crate) struct TutorialMenuPlugin;

impl Plugin for TutorialMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TutorialMenuCatalog>()
            .add_systems(Startup, register_tutorial_menu)
            .add_systems(
                Update,
                (sync_bundled_tutorial_catalog, sync_twin_tutorial_catalogs).chain(),
            );
    }
}

/// Discover the application-owned tutorial catalog from the runtime asset
/// listing and load it through the generic text asset pipeline. The catalog is
/// identified by its authored kind, not by a Rust path convention; adding or
/// relocating the file therefore does not require a core rebuild.
fn sync_bundled_tutorial_catalog(
    mut catalog: ResMut<TutorialMenuCatalog>,
    text_assets: Option<Res<lunco_assets_core::TextAssetCatalog>>,
    asset_server: Option<Res<AssetServer>>,
    assets: Option<Res<Assets<lunco_assets_core::TextAsset>>>,
) {
    let Some(text_assets) = text_assets else {
        return;
    };
    if !text_assets.ready() {
        return;
    }

    if !catalog.bundled_scan_started {
        catalog.bundled_candidates = text_assets
            .entries()
            .iter()
            .filter(|entry| entry.asset_path.ends_with(".json"))
            .map(|entry| PendingBundledCatalog {
                asset_path: entry.asset_path.clone(),
                handle: entry.handle.clone(),
            })
            .collect();
        catalog.bundled_scan_started = true;
    }

    let Some(assets) = assets else {
        return;
    };

    let candidates = std::mem::take(&mut catalog.bundled_candidates);
    let mut pending = Vec::new();
    let mut match_found = None;
    let mut catalog_error = None;

    for candidate in candidates {
        let Some(asset) = assets.get(&candidate.handle) else {
            let failed = asset_server.as_ref().is_some_and(|server| {
                server
                    .get_load_state(candidate.handle.id())
                    .is_some_and(|state| state.is_failed())
            });
            if !failed {
                pending.push(candidate);
            }
            continue;
        };

        let Ok(value) = serde_json::from_str::<serde_json::Value>(&asset.text) else {
            continue;
        };
        if value.get("kind").and_then(serde_json::Value::as_str)
            != Some(TUTORIAL_CATALOG_KIND)
        {
            continue;
        }

        match serde_json::from_value::<TutorialMenuFile>(value) {
            Ok(file) if match_found.is_none() => match_found = Some(file),
            Ok(_) => {
                catalog_error = Some(format!(
                    "more than one runtime asset is marked {TUTORIAL_CATALOG_KIND}"
                ));
            }
            Err(error) => {
                catalog_error = Some(format!("{}: invalid tutorial catalog: {error}", candidate.asset_path));
            }
        }
    }

    catalog.bundled_candidates = pending;
    if catalog.bundled_candidates.is_empty() {
        catalog.bundled_ready = true;
        if let Some(error) = catalog_error {
            catalog.error = Some(error);
        } else if let Some(file) = match_found {
            catalog.entries = file.tutorials;
            catalog.error = None;
        } else {
            catalog.error = Some(format!(
                "runtime asset listing contains no asset marked {TUTORIAL_CATALOG_KIND}"
            ));
        }
    }
}

fn tutorial_groups(
    bundled: Vec<TutorialMenuEntry>,
    twin_entries: BTreeMap<String, Vec<TutorialMenuEntry>>,
) -> BTreeMap<(u8, String, String), Vec<TutorialMenuEntry>> {
    let mut groups = BTreeMap::<(u8, String, String), Vec<TutorialMenuEntry>>::new();
    for entry in bundled {
        groups
            .entry((0, String::new(), entry.track.clone()))
            .or_default()
            .push(entry);
    }
    for (twin, entries) in twin_entries {
        for mut entry in entries {
            entry.source_asset = twin_asset_uri(&twin, &entry.source_asset);
            if !entry.scene_asset.is_empty() {
                entry.scene_asset = twin_asset_uri(&twin, &entry.scene_asset);
            }
            groups
                .entry((1, twin.clone(), entry.track.clone()))
                .or_default()
                .push(entry);
        }
    }
    groups
}

fn register_tutorial_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };

    menus.register_custom_menu("Tutorials", |ui, ctx| {
        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);

        let Some((bundled, twin_entries, catalog_error, loading_twins, bundled_ready)) =
            ctx.resource::<TutorialMenuCatalog>().map(|catalog| {
                (
                    catalog.entries.clone(),
                    catalog.twin_entries.clone(),
                    catalog.error.clone(),
                    !catalog.pending_twins.is_empty(),
                    catalog.bundled_ready,
                )
            })
        else {
            ui.label(
                egui::RichText::new("(tutorial catalog unavailable)")
                    .weak()
                    .italics(),
            );
            return;
        };
        if let Some(error) = catalog_error {
            ui.label(
                egui::RichText::new("(tutorial catalog is invalid)")
                    .weak()
                    .italics(),
            );
            ui.label(egui::RichText::new(error).weak().small());
            return;
        }
        if !bundled_ready && bundled.is_empty() && twin_entries.values().all(Vec::is_empty) {
            ui.label(
                egui::RichText::new("Loading tutorial catalog…")
                    .weak()
                    .italics(),
            );
            return;
        }
        if bundled.is_empty() && twin_entries.values().all(Vec::is_empty) {
            if loading_twins {
                ui.label(
                    egui::RichText::new("Loading Twin tutorials…")
                        .weak()
                        .italics(),
                );
                return;
            }
            ui.label(
                egui::RichText::new("(no tutorials available)")
                    .weak()
                    .italics(),
            );
            return;
        }

        ui.label(
            egui::RichText::new("Authored lessons run as ordinary Rhai scenarios.")
                .weak()
                .small(),
        );
        ui.separator();

        let groups = tutorial_groups(bundled, twin_entries);

        egui::ScrollArea::vertical()
            .max_height(SCENARIO_MENU_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for ((kind, twin, track), entries) in groups {
                    let heading = if kind == 0 {
                        track
                    } else {
                        format!("{twin} · {track}")
                    };
                    ui.menu_button(format!("{heading} ({})", entries.len()), |ui| {
                        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
                        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
                        egui::ScrollArea::vertical()
                            .max_height(SCENARIO_MENU_HEIGHT)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for entry in entries {
                                    let label = format!("{}  ·  {}", entry.title, entry.difficulty);
                                    let response = ui.add_sized(
                                        [ui.available_width(), 0.0],
                                        egui::Button::new(label).wrap(),
                                    );
                                    let response = response.on_hover_text(entry.blurb.as_str());
                                    if response.clicked() {
                                        ctx.trigger(RunScenarioAsset {
                                            target: Entity::PLACEHOLDER,
                                            source_asset: entry.source_asset.clone(),
                                            params: Default::default(),
                                            scene_asset: entry.scene_asset.clone(),
                                            reload_policy: ScenarioReloadPolicy::Restart,
                                        });
                                        ui.close();
                                    }
                                }
                            });
                    });
                }
            });
    });
}

fn twin_asset_uri(twin: &str, reference: &str) -> String {
    if lunco_assets_core::has_scheme(reference) {
        return reference.to_owned();
    }
    lunco_assets_core::twin_uri(twin, Path::new(reference))
}

/// Read Twin tutorial metadata through the shared discovery and storage
/// boundaries. The menu does not implement its own traversal and never reads
/// Twin files on the UI thread.
fn sync_twin_tutorial_catalogs(
    mut catalog: ResMut<TutorialMenuCatalog>,
    roots: Res<lunco_assets_core::TwinRoots>,
    manifest: Res<lunco_assets_core::discovery::AssetManifest>,
    settings: Option<Res<lunco_settings::DownloadSettings>>,
) {
    let Ok(names) = roots.names() else {
        catalog.twin_entries.clear();
        catalog.loaded_twins.clear();
        catalog.pending_twins.clear();
        return;
    };
    let active = names.iter().cloned().collect::<BTreeSet<_>>();
    catalog.twin_entries.retain(|name, _| active.contains(name));
    catalog.loaded_twins.retain(|name| active.contains(name));
    catalog
        .pending_twins
        .retain(|name, pending| active.contains(name) && pending.twin == *name);

    let Some(settings) = settings else {
        return;
    };
    let names_to_load = names
        .iter()
        .filter(|name| {
            !catalog.loaded_twins.contains(*name) && !catalog.pending_twins.contains_key(*name)
        })
        .cloned()
        .collect::<Vec<_>>();

    #[cfg(target_arch = "wasm32")]
    {
        let _ = (&manifest, &settings);
        // Twin roots are not enumerable in the browser. A Twin may still be
        // loaded through an explicit asset URI, but it cannot contribute an
        // undiscoverable menu catalog here.
        for name in names_to_load {
            catalog.loaded_twins.insert(name);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let twin_json_assets = if names_to_load.is_empty() {
            Vec::new()
        } else {
            match lunco_assets_core::discovery::list_assets(&manifest, &roots, "json") {
                Ok(assets) => assets
                    .into_iter()
                    .filter(|asset| asset.twin.is_some())
                    .collect::<Vec<_>>(),
                Err(error) => {
                    let message = format!("could not enumerate Twin JSON assets: {error}");
                    if catalog.error.as_deref() != Some(&message) {
                        error!("[tutorials] {message}");
                    }
                    catalog.error = Some(message);
                    return;
                }
            }
        };

        let mut requests = Vec::new();
        for name in names_to_load {
            let candidates = twin_json_assets
                .iter()
                .filter(|asset| asset.twin.as_deref() == Some(name.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                // A Twin may legitimately have no menu catalog. It remains a
                // valid Twin; only a JSON asset carrying the tutorial-catalog
                // kind is meaningful to this application-owned menu.
                catalog.loaded_twins.insert(name);
                continue;
            }
            requests.push((name, candidates, settings.clone()));
        }

        for (name, candidates, settings) in requests {
            let twin_name = name.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move {
                let mut found = None;
                for asset in candidates {
                    let text = lunco_assets_core::asset_read::read_asset_text(&asset, &settings).await?;
                    let value = serde_json::from_str::<serde_json::Value>(&text)
                        .map_err(|error| format!("{}: invalid JSON: {error}", asset.rel))?;
                    if value.get("kind").and_then(serde_json::Value::as_str)
                        != Some(TUTORIAL_CATALOG_KIND)
                    {
                        continue;
                    }
                    let file = serde_json::from_value::<TutorialMenuFile>(value)
                        .map_err(|error| format!("{}: invalid tutorial catalog: {error}", asset.rel))?;
                    if found.is_some() {
                        return Err(format!(
                            "Twin `{twin_name}` has more than one asset marked {TUTORIAL_CATALOG_KIND}"
                        ));
                    }
                    found = Some(file);
                }
                Ok(found)
            });
            catalog
                .pending_twins
                .insert(name.clone(), PendingTwinCatalog { twin: name, task });
        }
    }

    let mut finished = Vec::new();
    for (name, pending) in &mut catalog.pending_twins {
        if let Some(result) = block_on(future::poll_once(&mut pending.task)) {
            finished.push((name.clone(), result));
        }
    }
    for (name, result) in finished {
        catalog.pending_twins.remove(&name);
        catalog.loaded_twins.insert(name.clone());
        match result {
            Ok(Some(file)) => {
                catalog.twin_entries.insert(name, file.tutorials);
            }
            Ok(None) => {}
            Err(error) => {
                error!("[tutorials] could not load Twin catalog: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{tutorial_groups, TutorialMenuEntry};

    fn entry(track: &str, title: &str) -> TutorialMenuEntry {
        TutorialMenuEntry {
            track: track.to_owned(),
            title: title.to_owned(),
            blurb: String::new(),
            difficulty: "beginner".to_owned(),
            source_asset: format!("lunco://tutorials/{track}/{title}.rhai"),
            scene_asset: String::new(),
        }
    }

    #[test]
    fn tutorial_groups_preserve_track_and_lesson_order() {
        let mut twin_entries = std::collections::BTreeMap::new();
        twin_entries.insert(
            "Demo Twin".to_owned(),
            vec![
                entry("Rover", "Twin Lesson 1"),
                entry("Rover", "Twin Lesson 2"),
            ],
        );
        let groups = tutorial_groups(
            vec![
                entry("Sandbox", "First Drive"),
                entry("Modelica", "Overview"),
                entry("Sandbox", "Build a Scene"),
                entry("Navigation", "View and Build"),
            ],
            twin_entries,
        );

        let bundled = groups
            .get(&(0, String::new(), "Sandbox".to_owned()))
            .unwrap();
        assert_eq!(
            bundled
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["First Drive", "Build a Scene"]
        );
        let twin = groups
            .get(&(1, "Demo Twin".to_owned(), "Rover".to_owned()))
            .unwrap();
        assert_eq!(
            twin.iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Twin Lesson 1", "Twin Lesson 2"]
        );
    }
}
