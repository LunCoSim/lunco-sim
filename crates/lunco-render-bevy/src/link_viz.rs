//! Draws connectivity links — the render half of `lunco_celestial::link`.
//!
//! `lunco-celestial` is a render-free simulation crate: it solves the pairwise
//! geometry (range, elevation, body/terrain/occluder blocking), applies the
//! `link.connected` verdict and stores the result on each node's `LinkState`. It
//! never names `Gizmos` — doing so would pull `bevy_gizmos → bevy_render → wgpu`
//! into the `--no-ui` server and the wasm worker, and its Cargo.toml carries an
//! explicit guard against exactly that.
//!
//! So the lines are drawn here, from state the kernel already published. **It
//! solves nothing** — no ranges, no raycasts, no verdicts; it reads `LinkState`
//! and draws it. Same split as `sensor_beams.rs`, for the same reason.
//!
//! ## Why this exists
//!
//! The kernel computed correct link geometry — including real terrain
//! radio-shadow — that **nothing rendered**. `LinkState` is `Reflect`-registered,
//! so the only way to see connectivity was to select a node and read a struct in
//! the inspector. For a lesson whose whole point is "line of sight is geometry,
//! so move to fix it", a number in a panel is not an answer; the line between the
//! two antennas is.
//!
//! See `docs/architecture/render-decoupling.md` and doc 49.

use bevy::gizmos::config::GizmoConfigStore;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use big_space::prelude::BigSpaceSystems;
use lunco_celestial::link::LinkState;
use lunco_core::{on_command, register_commands, Command, GlobalEntityId};

/// A live link: bright, opaque.
const UP_COLOR: Color = Color::srgb(0.25, 0.95, 0.45);
/// A severed link: the same geometry, faded and red. Drawn rather than hidden —
/// "there is a link here and it is broken" is the interesting state, and a line
/// that vanishes teaches nothing about WHY.
const DOWN_COLOR: Color = Color::srgba(0.95, 0.25, 0.2, 0.35);

/// Connectivity overlay toggle. Global (not per-entity): a link is a property of a
/// PAIR, so there is no single node that owns the line.
#[derive(Resource, Debug, Clone, Reflect)]
#[reflect(Resource)]
pub struct LinkVizSettings {
    /// Draw a line per node pair, coloured by link state.
    pub show_links: bool,
    /// Also draw the pairs that are DOWN (faded red). Off in a dense scene, where
    /// N nodes mean N²/2 lines and the down set is most of them.
    pub show_down: bool,
}

impl Default for LinkVizSettings {
    fn default() -> Self {
        // On by default: the overlay is the only way to see connectivity at all,
        // and a scene with no link nodes draws nothing.
        Self { show_links: true, show_down: true }
    }
}

/// Toggle the connectivity overlay at runtime, from any client or language:
/// `cmd("ToggleLinkViz", #{ show_links: true, show_down: false })`.
#[Command(default)]
pub struct ToggleLinkViz {
    pub show_links: bool,
    pub show_down: bool,
}

#[on_command(ToggleLinkViz)]
fn on_toggle_link_viz(trigger: On<ToggleLinkViz>, mut settings: ResMut<LinkVizSettings>) {
    settings.show_links = cmd.show_links;
    settings.show_down = cmd.show_down;
    info!(
        "[link-viz] links {} / down {}",
        settings.show_links, settings.show_down
    );
}

register_commands!(on_toggle_link_viz);

pub(crate) fn build(app: &mut App) {
    app.init_resource::<LinkVizSettings>();
    app.register_type::<LinkVizSettings>();
    register_all_commands(app);
    // GATED ON THE GIZMO STORE, and it must stay that way: a `Gizmos` system param
    // PANICS when `GizmoPlugin` is absent, which is every `MinimalPlugins` test and
    // every headless app that links this crate. See `sensor_beams::build`.
    //
    // PostUpdate AFTER `BigSpaceSystems::PropagateHighPrecision`, not `Update` —
    // the endpoints anchor to each node's `GlobalTransform`, and in `Update` that
    // is a frame stale while the antenna MESH renders from this frame's propagated
    // value. At rest invisible; on a moving rover, a line whose end lags the dish.
    app.add_systems(
        PostUpdate,
        draw_links
            .after(BigSpaceSystems::PropagateHighPrecision)
            .run_if(resource_exists::<GizmoConfigStore>),
    );
}

fn draw_links(
    settings: Res<LinkVizSettings>,
    q: Query<(&GlobalEntityId, &GlobalTransform, &LinkState)>,
    mut gizmos: Gizmos,
) {
    if !settings.show_links {
        return;
    }
    // GID → render-frame position. `LinkState` names its peers by GID (identity),
    // not by entity, so resolving the far end means a lookup — and a node whose
    // peer is missing from this map (despawned mid-frame, identity not yet minted)
    // simply is not drawn.
    let pos: HashMap<u64, Vec3> = q
        .iter()
        .map(|(gid, gt, _)| (gid.get(), gt.translation()))
        .collect();

    for (gid, gt, state) in &q {
        for peer in &state.peers {
            // Each pair is listed from both ends; draw the edge once.
            if gid.get() > peer.peer {
                continue;
            }
            if !peer.connected && !settings.show_down {
                continue;
            }
            let Some(&b) = pos.get(&peer.peer) else {
                continue;
            };
            let color = if peer.connected { UP_COLOR } else { DOWN_COLOR };
            gizmos.line(gt.translation(), b, color);
        }
    }
}
