//! Application-owned catalog for authored tutorial scenarios.
//!
//! This module is intentionally the only Rust code that knows the word
//! "tutorial". A tutorial is otherwise just a file-backed Rhai scenario plus
//! optional standard USD scene content. The menu projects the authored
//! catalog into the application UI and launches it through the generic
//! `RunScenarioAsset` command.

use bevy::prelude::*;
use bevy_egui::egui;
use lunco_scripting::commands::RunScenarioAsset;
use lunco_scripting::ScenarioReloadPolicy;
use lunco_workbench_core::WorkbenchMenuRegistry;
use serde::Deserialize;

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

#[derive(Resource, Debug, Clone, Default)]
struct TutorialMenuCatalog {
    entries: Vec<TutorialMenuEntry>,
    error: Option<String>,
}

pub(crate) struct TutorialMenuPlugin;

impl Plugin for TutorialMenuPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(load_catalog())
            .add_systems(Startup, register_tutorial_menu);
    }
}

fn load_catalog() -> TutorialMenuCatalog {
    match serde_json::from_str::<TutorialMenuFile>(&lunco_assets::tutorials::tutorial_catalog_json())
    {
        Ok(file) => TutorialMenuCatalog {
            entries: file.tutorials,
            error: None,
        },
        Err(error) => TutorialMenuCatalog {
            entries: Vec::new(),
            error: Some(error.to_string()),
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

        let Some(catalog) = ctx.resource::<TutorialMenuCatalog>().cloned() else {
            ui.label(
                egui::RichText::new("(tutorial catalog unavailable)")
                    .weak()
                    .italics(),
            );
            return;
        };
        if let Some(error) = catalog.error {
            ui.label(
                egui::RichText::new("(tutorial catalog is invalid)")
                    .weak()
                    .italics(),
            );
            ui.label(egui::RichText::new(error).weak().small());
            return;
        }
        if catalog.entries.is_empty() {
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

        let mut tracks = Vec::<String>::new();
        for entry in &catalog.entries {
            if !tracks.iter().any(|track| track == &entry.track) {
                tracks.push(entry.track.clone());
            }
        }

        egui::ScrollArea::vertical()
            .max_height(SCENARIO_MENU_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for track in tracks {
                    ui.strong(&track);
                    for entry in catalog.entries.iter().filter(|entry| entry.track == track) {
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
                    ui.add_space(6.0);
                }
            });
    });
}
