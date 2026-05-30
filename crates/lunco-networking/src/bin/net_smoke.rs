//! Headless networking smoke-test harness.
//!
//! Runs the *real* `LunCoNetworkingPlugin` (lightyear WebTransport + our
//! cert/handshake/ferry) with no window or scene, so the connect→handshake path
//! can be exercised without launching the full GUI sandbox.
//!
//!   net_smoke --host 5888
//!   net_smoke --connect 127.0.0.1:5888
//!
//! Success = the host logs `[net] client connected …` and the client logs
//! `[smoke] LocalSession now = <nonzero>` (handshake delivered end-to-end).

use bevy::app::AppExit;
use bevy::prelude::*;
use lunco_networking::{LunCoNetworkingPlugin, NetworkMode};

fn main() {
    let Some(mode) = NetworkMode::from_args() else {
        eprintln!("usage: net_smoke --host [port] | --connect <addr>");
        return;
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::log::LogPlugin::default());
    app.add_plugins(lunco_core::LunCoCorePlugin);
    app.add_plugins(lunco_api::LunCoApiPlugin::default());
    app.add_plugins(LunCoNetworkingPlugin { mode });
    app.add_systems(Update, (report_session, exit_after_timeout));
    app.run();
}

/// Log the session id whenever it changes (the client's becomes non-zero once
/// the handshake lands).
fn report_session(local: Res<lunco_core::LocalSession>, mut last: Local<u64>) {
    let cur = local.0 .0;
    if cur != *last {
        *last = cur;
        info!("[smoke] LocalSession now = {cur}");
    }
}

/// End the run so the harness doesn't hang.
fn exit_after_timeout(time: Res<Time>, mut exit: MessageWriter<AppExit>) {
    if time.elapsed_secs() > 12.0 {
        info!("[smoke] timeout reached, exiting");
        exit.write(AppExit::Success);
    }
}
