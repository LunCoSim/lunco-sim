//! Integration coverage for the public Twin-to-scene command boundary.
//!
//! These tests use the same asset source and `UsdSourceText` loader as the
//! production composition root. Keeping them outside `commands.rs` means the
//! command library has no test-only scene-loading branch or test-only imports.

use bevy::asset::{AssetApp, AssetPlugin};
use bevy::prelude::*;
use lunco_usd::commands::UsdCommandsPlugin;
use lunco_usd_bevy_core::{
    source::{UsdSourceText, UsdSourceTextLoader},
    UsdLoader, UsdStageAsset,
};
use lunco_usd_core::commands::EmptyViewportReason;
use lunco_usd_sim_cosim::{ClearScene, LoadScene};
use lunco_workspace::WorkspaceResource;

#[derive(Debug, Resource, Default)]
struct SceneCommands {
    loads: Vec<String>,
    clears: usize,
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    lunco_assets::register_lunco_asset_sources(&mut app);
    app.add_plugins(AssetPlugin::default());
    app.init_asset::<UsdSourceText>()
        .register_asset_loader(UsdSourceTextLoader)
        .init_asset::<UsdStageAsset>()
        .register_asset_loader(UsdLoader);
    app.init_resource::<WorkspaceResource>();
    app.add_plugins(UsdCommandsPlugin);
    app.init_resource::<SceneCommands>();
    app.add_observer(|trigger: On<LoadScene>, mut state: ResMut<SceneCommands>| {
        state.loads.push(trigger.event().path.clone());
    });
    app.add_observer(
        |_trigger: On<ClearScene>, mut state: ResMut<SceneCommands>| {
            state.clears += 1;
        },
    );
    app.update();
    app
}

fn wait_for_scene_decision(app: &mut App) {
    for _ in 0..1_000 {
        app.update();
        let state = app.world().resource::<SceneCommands>();
        if !state.loads.is_empty() || state.clears != 0 {
            return;
        }
        std::thread::yield_now();
    }
    panic!(
        "TwinAdded did not produce LoadScene or ClearScene: {:?}",
        app.world().resource::<SceneCommands>()
    );
}

fn scene_commands_for_twin(toml_body: &str) -> (tempfile::TempDir, SceneCommands) {
    let temp = tempfile::tempdir().expect("temporary Twin root");
    std::fs::write(temp.path().join("twin.toml"), toml_body).expect("Twin manifest");
    std::fs::write(
        temp.path().join("scene_a.usda"),
        "#usda 1.0\ndef Xform \"A\" {}\n",
    )
    .expect("starting scene");
    std::fs::write(
        temp.path().join("scene_b.usda"),
        "#usda 1.0\ndef Xform \"B\" {}\n",
    )
    .expect("library scene");
    std::fs::write(
        temp.path().join("controller.mo"),
        "model Controller end Controller;\n",
    )
    .expect("Twin Modelica source");

    let mut app = app();
    let twin = match lunco_twin::TwinMode::open(temp.path()).expect("Twin opens") {
        lunco_twin::TwinMode::Twin(twin) | lunco_twin::TwinMode::Folder(twin) => twin,
        other => panic!("expected Twin/Folder variant, got {other:?}"),
    };
    let twin_id = app
        .world_mut()
        .resource_mut::<WorkspaceResource>()
        .add_twin(twin);
    app.world_mut()
        .trigger(lunco_workspace::TwinAdded { twin: twin_id });
    wait_for_scene_decision(&mut app);

    let state = std::mem::take(app.world_mut().resource_mut::<SceneCommands>().as_mut());
    (temp, state)
}

#[test]
fn twin_added_loads_only_declared_starting_scene() {
    let (_temp, commands) = scene_commands_for_twin(
        "name = \"t\"\nversion = \"0.1.0\"\n\n[usd]\ndefault_scene = \"scene_a.usda\"\n",
    );
    assert_eq!(commands.loads.len(), 1, "exactly one scene loaded");
    assert!(
        commands.loads[0].ends_with("scene_a.usda"),
        "the declared starting scene, got {:?}",
        commands.loads
    );
    assert_eq!(
        commands.clears, 0,
        "the load path does not clear separately"
    );
}

#[test]
fn twin_added_without_default_scene_clears_viewport() {
    let (_temp, commands) = scene_commands_for_twin("name = \"t\"\nversion = \"0.1.0\"\n");
    assert!(
        commands.loads.is_empty(),
        "no scene loaded, got {:?}",
        commands.loads
    );
    assert_eq!(commands.clears, 1, "viewport cleared to empty");
}

#[test]
fn open_folder_with_no_usda_shows_nothing() {
    let temp = tempfile::tempdir().expect("temporary folder");
    std::fs::write(temp.path().join("notes.txt"), "no scenes here\n").expect("folder note");

    let mut app = app();
    let twin = match lunco_twin::TwinMode::open(temp.path()).expect("folder opens") {
        lunco_twin::TwinMode::Folder(twin) => twin,
        other => panic!("expected Folder variant, got {other:?}"),
    };
    let twin_id = app
        .world_mut()
        .resource_mut::<WorkspaceResource>()
        .add_twin(twin);
    app.world_mut()
        .trigger(lunco_workspace::TwinAdded { twin: twin_id });
    wait_for_scene_decision(&mut app);

    let commands = app.world().resource::<SceneCommands>();
    assert!(
        commands.loads.is_empty(),
        "nothing to load, got {:?}",
        commands.loads
    );
    assert_eq!(commands.clears, 1, "empty folder clears the viewport");
}

#[test]
fn folder_with_no_manifest_records_wrong_folder_reason() {
    let temp = tempfile::tempdir().expect("temporary folder");
    std::fs::write(temp.path().join("readme.txt"), "not a Twin\n").expect("folder note");

    let mut app = app();
    let twin = match lunco_twin::TwinMode::open(temp.path()).expect("folder opens") {
        lunco_twin::TwinMode::Folder(twin) => twin,
        other => panic!("a folder with no twin.toml is Folder, got {other:?}"),
    };
    let twin_id = app
        .world_mut()
        .resource_mut::<WorkspaceResource>()
        .add_twin(twin);
    app.world_mut()
        .trigger(lunco_workspace::TwinAdded { twin: twin_id });
    wait_for_scene_decision(&mut app);

    let reason = app
        .world()
        .resource::<EmptyViewportReason>()
        .0
        .as_ref()
        .expect("a folder without twin.toml must record a reason");
    assert!(
        reason.contains("no twin.toml"),
        "reason should name the missing manifest, got: {reason}"
    );
    assert!(
        reason.contains("wrong folder"),
        "reason should hint the likely cause, got: {reason}"
    );
}
