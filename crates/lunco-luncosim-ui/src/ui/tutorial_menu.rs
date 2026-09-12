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

struct PendingTwinCatalog {
    twin: String,
    task: Task<Result<TutorialMenuFile, String>>,
}

#[derive(Resource, Default)]
struct TutorialMenuCatalog {
    entries: Vec<TutorialMenuEntry>,
    error: Option<String>,
    twin_entries: BTreeMap<String, Vec<TutorialMenuEntry>>,
    loaded_twins: BTreeSet<String>,
    pending_twins: HashMap<String, PendingTwinCatalog>,
}

pub(crate) struct TutorialMenuPlugin;

impl Plugin for TutorialMenuPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_catalog())
            .add_systems(Startup, register_tutorial_menu)
            .add_systems(Update, sync_twin_tutorial_catalogs);
    }
}

fn load_catalog() -> TutorialMenuCatalog {
    match serde_json::from_str::<TutorialMenuFile>(&lunco_assets::tutorials::tutorial_catalog_json())
    {
        Ok(file) => TutorialMenuCatalog {
            entries: file.tutorials,
            error: None,
            ..default()
        },
        Err(error) => TutorialMenuCatalog {
            entries: Vec::new(),
            error: Some(error.to_string()),
            ..default()
        },
    }
}

fn register_tutorial_menu(world: &mut World) {
    let Some(mut menus) = world.get_resource_mut::<WorkbenchMenuRegistry>() else {
        return;
    };

    menus.register_custom_menu("Tutorials", |ui, ctx| {
        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);

        let Some((bundled, twin_entries, catalog_error, loading_twins)) =
            ctx.resource::<TutorialMenuCatalog>().map(|catalog| {
                (
                    catalog.entries.clone(),
                    catalog.twin_entries.clone(),
                    catalog.error.clone(),
                    !catalog.pending_twins.is_empty(),
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

        egui::ScrollArea::vertical()
            .max_height(SCENARIO_MENU_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for ((kind, twin, track), entries) in groups {
                    let heading = if kind == 0 {
                        track
                    } else {
                        format!("{twin} · {track}")
                    };
                    egui::CollapsingHeader::new(format!("{heading} ({})", entries.len()))
                        .default_open(false)
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
                                        params: String::new(),
                                        scene_asset: entry.scene_asset.clone(),
                                        reload_policy: ScenarioReloadPolicy::Restart,
                                    });
                                    ui.close();
                                }
                            }
                        });
                }
            });
    });
}

fn twin_asset_uri(twin: &str, reference: &str) -> String {
    if lunco_assets::has_scheme(reference) {
        return reference.to_owned();
    }
    lunco_assets::twin_uri(twin, Path::new(reference))
}

/// Read Twin tutorial metadata through the shared asset/storage boundary. The
/// menu never walks a Twin itself and never reads its files on the UI thread.
fn sync_twin_tutorial_catalogs(
    mut catalog: ResMut<TutorialMenuCatalog>,
    roots: Res<lunco_assets::TwinRoots>,
    manifest: Res<lunco_assets::discovery::AssetManifest>,
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
    let mut requests = Vec::new();
    for name in names {
        if catalog.loaded_twins.contains(&name) || catalog.pending_twins.contains_key(&name) {
            continue;
        }
        let path = lunco_assets::twin_uri(&name, Path::new("sim/tutorials/catalog.json"));
        let Ok(Some(asset)) = lunco_assets::discovery::resolve_asset(&manifest, &roots, &path)
        else {
            // A Twin may legitimately have no menu catalog. It remains a valid
            // Twin; only its lessons are absent from this application menu.
            catalog.loaded_twins.insert(name);
            continue;
        };
        let settings = settings.clone();
        requests.push((name, asset, settings));
    }
    for (name, asset, settings) in requests {
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let text = lunco_assets::asset_read::read_asset_text(&asset, &settings).await?;
            serde_json::from_str::<TutorialMenuFile>(&text)
                .map_err(|error| format!("{}: invalid tutorial catalog: {error}", asset.rel))
        });
        catalog
            .pending_twins
            .insert(name.clone(), PendingTwinCatalog { twin: name, task });
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
            Ok(file) => {
                catalog.twin_entries.insert(name, file.tutorials);
            }
            Err(error) => {
                error!("[tutorials] could not load Twin catalog: {error}");
            }
        }
    }
}
