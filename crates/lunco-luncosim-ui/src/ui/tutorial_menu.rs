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

/// Group catalog indices by their first-seen track while preserving the
/// authored track and lesson order. The indices keep the catalog entries as
/// the single source of lesson metadata and avoid cloning them for menus.
fn tutorial_track_groups(entries: &[TutorialMenuEntry]) -> Vec<(String, Vec<usize>)> {
    let mut groups = Vec::<(String, Vec<usize>)>::new();
    for (index, entry) in entries.iter().enumerate() {
        if let Some((_, indices)) = groups.iter_mut().find(|(track, _)| track == &entry.track) {
            indices.push(index);
        } else {
            groups.push((entry.track.clone(), vec![index]));
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

        egui::ScrollArea::vertical()
            .max_height(SCENARIO_MENU_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (track, indices) in tutorial_track_groups(&catalog.entries) {
                    ui.menu_button(format!("{track}  ({})", indices.len()), |ui| {
                        ui.set_min_width(SCENARIO_MENU_MIN_WIDTH);
                        ui.set_max_width(SCENARIO_MENU_MAX_WIDTH);
                        egui::ScrollArea::vertical()
                            .max_height(SCENARIO_MENU_HEIGHT)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                for index in indices {
                                    let entry = &catalog.entries[index];
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
                    });
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::{tutorial_track_groups, TutorialMenuEntry};

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
    fn tutorial_tracks_preserve_first_seen_groups_and_lesson_order() {
        let entries = vec![
            entry("Sandbox", "First Drive"),
            entry("Modelica", "Overview"),
            entry("Sandbox", "Build a Scene"),
            entry("Navigation", "View and Build"),
        ];

        assert_eq!(
            tutorial_track_groups(&entries),
            vec![
                ("Sandbox".to_owned(), vec![0, 2]),
                ("Modelica".to_owned(), vec![1]),
                ("Navigation".to_owned(), vec![3]),
            ]
        );
    }
}
