//! Dependency-light Bevy substrate for LunCoSim hosts.
//!
//! This package owns only the host-neutral ECS substrate: asset source/type
//! registration, the headless Bevy plugin group, task-pool policy, and log
//! deduplication. Domain composition (physics, USD, terrain, Modelica,
//! celestial, avatar, and scene commands) belongs to
//! `lunco-luncosim-simulation` and is installed by the host that needs it.

use bevy::asset::{AssetMetaCheck, AssetPlugin};
use bevy::prelude::*;

/// Exit status returned by production runners.
pub use bevy::app::AppExit;

/// Collapse repeated WARN/ERROR lines into one line plus a count.
pub mod log_dedup;

/// SemVer2 product version stamped into this build.
pub const PRODUCT_VERSION: &str = env!("LUNCO_RELEASE_VERSION");
/// Short source revision stamped into this build for diagnostics.
pub const GIT_SHA: &str = env!("LUNCO_GIT_SHA");
/// Public GitHub repository containing the stamped source revision.
pub const REPOSITORY_URL: &str = env!("LUNCO_REPOSITORY_URL");

/// Print the build identity shared by every production LunCoSim host.
pub fn log_build_identity(mode: &str) {
    println!(
        "[lunco] luncosim {ver} ({sha}) {profile} {mode} {os}/{arch}",
        ver = PRODUCT_VERSION,
        sha = GIT_SHA,
        profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    );
}

struct HeadlessAssetTypePlugin;

impl Plugin for HeadlessAssetTypePlugin {
    fn build(&self, app: &mut App) {
        // Avian's collider cache consumes AssetEvent<Mesh> even in a
        // render-free world. Register data-only stores here; GPU/render
        // plugins remain an application-edge concern.
        app.init_asset::<bevy::mesh::Mesh>();
        app.init_asset::<bevy::shader::Shader>();
        app.init_asset::<bevy::image::Image>();
    }
}

/// Build the headless Bevy substrate shared by servers and scene tests.
///
/// The host owns scheduling. `MinimalPlugins` therefore has its default
/// `ScheduleRunnerPlugin` disabled so deterministic runners can install one
/// explicit cadence policy.
pub fn default_plugins() -> bevy::app::PluginGroupBuilder {
    MinimalPlugins
        .build()
        .disable::<bevy::app::ScheduleRunnerPlugin>()
        .add(bevy::app::PanicHandlerPlugin)
        .add(bevy::log::LogPlugin {
            filter: "wgpu=error,naga=warn,cranelift=warn,cranelift_jit=warn,cranelift_codegen=warn,diffsol=warn,info".into(),
            fmt_layer: |_app| {
                use bevy::log::tracing_subscriber::Layer;
                use std::io::IsTerminal;
                let ansi =
                    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
                Some(Box::new(
                    bevy::log::tracing_subscriber::fmt::Layer::default()
                        .with_ansi(ansi)
                        .with_writer(std::io::stderr)
                        .with_filter(log_dedup::DedupFilter),
                ))
            },
            ..default()
        })
        .add(bevy::diagnostic::DiagnosticsPlugin)
        .add(bevy::input::InputPlugin)
        .add(bevy::state::app::StatesPlugin)
        .add(AssetPlugin {
            file_path: lunco_assets_core::assets_dir_abs().to_string_lossy().to_string(),
            watch_for_changes_override: Some(false),
            meta_check: AssetMetaCheck::Never,
            ..default()
        })
        .add_after::<AssetPlugin>(HeadlessAssetTypePlugin)
        .build()
}

/// Build the host-neutral Bevy substrate with an optional pinned Compute pool.
///
/// `None` keeps Bevy's default task-pool allocation. `Some(n)` pins only the
/// Compute pool to `n` threads; IO and AsyncCompute retain their Bevy defaults.
///
/// Domain plugins and application services are layered by the production host;
/// this function deliberately cannot install them because it has no dependency
/// on those domains.
pub fn build_core_app(compute_threads: Option<usize>) -> App {
    let mut app = App::new();
    lunco_assets_runtime::register_lunco_asset_sources(&mut app);

    let task_pool_options = if let Some(threads) = compute_threads {
        assert!(threads > 0, "compute_threads must be positive");
        bevy::app::TaskPoolOptions {
            compute: bevy::app::TaskPoolThreadAssignmentPolicy {
                min_threads: threads,
                max_threads: threads,
                percent: 1.0,
                on_thread_spawn: None,
                on_thread_destroy: None,
            },
            ..default()
        }
    } else {
        bevy::app::TaskPoolOptions::default()
    };
    let plugins = default_plugins().set(bevy::app::TaskPoolPlugin { task_pool_options });
    app.add_plugins(plugins);
    lunco_assets_runtime::register_lunco_asset_types(&mut app);
    app.add_plugins(log_dedup::LogDedupPlugin);
    app
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_substrate_installs_only_data_asset_stores() {
        let mut app = App::new();
        app.add_plugins(default_plugins());

        assert!(app.is_plugin_added::<AssetPlugin>());
        assert!(app.world().get_resource::<AssetServer>().is_some());
        assert!(
            app.world()
                .get_resource::<Assets<bevy::shader::Shader>>()
                .is_some()
        );
        assert!(
            app.world()
                .get_resource::<Assets<bevy::image::Image>>()
                .is_some()
        );
    }
}
