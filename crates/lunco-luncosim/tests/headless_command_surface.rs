//! The commands a WINDOWLESS host must expose, and the rule that keeps them.
//!
//! Which commands exist is decided by which PLUGINS a host adds, not by cargo
//! features — `--no-ui` is a runtime flag, so the code is compiled in either
//! way and only the registration differs. That makes the failure silent and
//! asymmetric: everything works in the GUI, and a server answers
//! `Command 'X' not found or not API-accessible`, which is also what a typo
//! looks like. A command registered from a plugin that draws egui does not
//! exist on a windowless host, and nothing but this test says so.
//!
//! The list below is DELIBERATELY a list, not an enumeration: it is the
//! session-control contract — probe the host, mount a twin to work on, shut the
//! host down. A server that cannot do those three is not operable, however many
//! domain commands it exposes. Anything else being unregistered is a feature
//! decision; these are not.

use bevy::prelude::*;

/// Exactly the check `lunco_api::executor` performs when a request names a
/// command: short type path in the registry, carrying `ReflectEvent`.
fn resolves(app: &App, command: &str) -> bool {
    let registry = app.world().resource::<AppTypeRegistry>().read();
    registry
        .get_with_short_type_path(command)
        .map(|r| r.data::<bevy::ecs::reflect::ReflectEvent>().is_some())
        .unwrap_or(false)
}

/// The plugins a windowless host adds — no window, no egui, no renderer.
fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(lunco_api::LunCoApiPlugin::default())
        .add_plugins(lunco_workspace::WorkspacePlugin);
    app
}

#[test]
fn a_windowless_host_can_probe_mount_and_shut_down() {
    let app = headless_app();
    for command in [
        // Probe: readiness. Answering "not found" to this is the one answer a
        // probe must never give — it is indistinguishable from "wrong name".
        "Ping",
        // Mount: a server can run a twin's scenarios, so it must be able to
        // open the twin they belong to rather than inheriting one from a GUI
        // session that happened to run earlier.
        "OpenTwin",
        "OpenFolder",
        "AddTwin",
        "AddFolderToWorkspace",
        // Shut down: the only way to stop a server that is not `kill`.
        "Exit",
    ] {
        assert!(
            resolves(&app, command),
            "'{command}' does not resolve on a windowless host — some plugin that \
             registers it needs a window. Register it from a headless plugin \
             instead; see crates/lunco-workspace/src/open.rs."
        );
    }
}

/// A name that was never registered must fail the SAME way — otherwise the
/// assertions above would pass against a registry that accepts anything.
#[test]
fn an_unregistered_name_does_not_resolve() {
    assert!(!resolves(&headless_app(), "NoSuchCommandExists"));
}
