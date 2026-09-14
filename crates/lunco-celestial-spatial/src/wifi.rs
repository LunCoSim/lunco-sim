//! Rover radio graph projected over the generic link geometry substrate.
//!
//! `LinkState` is the direct connectivity graph and remains subject to the
//! authored `link.connected` policy. Wi-Fi is a
//! separate domain graph: it consumes [`link::LinkGeometryState`] and applies
//! only radio endpoint/range eligibility, so adding rover-to-rover radio does
//! not reopen a direct Earth link or alter its policy.

use bevy::prelude::*;
use std::collections::HashMap;

use crate::link::LinkGeometryState;

/// A scene-authored rover radio endpoint.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct WifiNode {
    /// Maximum radio range in metres for this endpoint.
    pub max_range_m: f64,
}

/// One endpoint's resolved Wi-Fi peers.
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct WifiState {
    pub peers: Vec<WifiPeer>,
}

#[derive(Debug, Clone, Reflect)]
pub struct WifiPeer {
    pub peer: u64,
    pub connected: bool,
    pub range_m: f64,
    pub light_time_s: f64,
    pub class: Option<String>,
}

/// Wi-Fi is a projection of `WifiNode` settings and the generic link geometry.
/// Rebuild it only when one of those inputs changes; stable frames have no new
/// graph to project.
pub(crate) fn wifi_links_due(
    changed: Query<
        (),
        (
            With<WifiNode>,
            Or<(Changed<WifiNode>, Changed<LinkGeometryState>)>,
        ),
    >,
) -> bool {
    !changed.is_empty()
}

/// Project a separate Wi-Fi graph from the raw geometry observations.
///
/// This system deliberately does not read `LinkState`: a direct policy-down
/// pair is still a valid Wi-Fi candidate when both endpoints author `WifiNode`.
pub fn update_wifi_links(
    q_nodes: Query<(
        Entity,
        &lunco_core::GlobalEntityId,
        &WifiNode,
        &LinkGeometryState,
    )>,
    mut q_states: Query<&mut WifiState>,
    mut commands: Commands,
) {
    let endpoints: HashMap<u64, (Entity, f64)> = q_nodes
        .iter()
        .map(|(entity, gid, wifi, _)| (gid.get(), (entity, wifi.max_range_m)))
        .collect();

    for (entity, _gid, wifi, geometry) in q_nodes.iter() {
        let peers = geometry
            .peers
            .iter()
            .filter_map(|peer| {
                let (_, peer_range) = endpoints.get(&peer.peer)?;
                Some(WifiPeer {
                    peer: peer.peer,
                    connected: peer.builtin
                        && peer.range_m.is_finite()
                        && peer.range_m <= wifi.max_range_m.min(*peer_range),
                    range_m: peer.range_m,
                    light_time_s: peer.light_time_s,
                    class: peer.class.clone(),
                })
            })
            .collect();
        if let Ok(mut state) = q_states.get_mut(entity) {
            state.peers = peers;
        } else {
            commands.entity(entity).try_insert(WifiState { peers });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{LinkGeometryPeer, LinkState};

    #[test]
    fn rover_radio_can_connect_when_direct_link_state_is_absent() {
        let mut app = App::new();
        app.add_systems(Update, update_wifi_links.run_if(wifi_links_due));

        let a = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(1),
                WifiNode { max_range_m: 100.0 },
                LinkGeometryState {
                    peers: vec![LinkGeometryPeer {
                        peer: 2,
                        builtin: true,
                        range_m: 25.0,
                        light_time_s: 0.0,
                        elevation_deg: None,
                        class: Some("rover".to_string()),
                    }],
                },
            ))
            .id();
        let b = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(2),
                WifiNode { max_range_m: 100.0 },
                LinkGeometryState {
                    peers: vec![LinkGeometryPeer {
                        peer: 1,
                        builtin: true,
                        range_m: 25.0,
                        light_time_s: 0.0,
                        elevation_deg: None,
                        class: Some("rover".to_string()),
                    }],
                },
            ))
            .id();

        app.update();

        assert!(app.world().get::<LinkState>(a).is_none());
        assert!(app.world().get::<LinkState>(b).is_none());
        assert!(app
            .world()
            .get::<WifiState>(a)
            .unwrap()
            .peers
            .iter()
            .any(|peer| peer.peer == 2 && peer.connected));
        assert!(app
            .world()
            .get::<WifiState>(b)
            .unwrap()
            .peers
            .iter()
            .any(|peer| peer.peer == 1 && peer.connected));
    }

    #[test]
    fn wifi_applies_both_endpoint_ranges() {
        let mut app = App::new();
        app.add_systems(Update, update_wifi_links.run_if(wifi_links_due));
        let a = app
            .world_mut()
            .spawn((
                lunco_core::GlobalEntityId::from_raw(1),
                WifiNode { max_range_m: 100.0 },
                LinkGeometryState {
                    peers: vec![LinkGeometryPeer {
                        peer: 2,
                        builtin: true,
                        range_m: 25.0,
                        light_time_s: 0.0,
                        elevation_deg: None,
                        class: Some("rover".to_string()),
                    }],
                },
            ))
            .id();
        app.world_mut().spawn((
            lunco_core::GlobalEntityId::from_raw(2),
            WifiNode { max_range_m: 10.0 },
            LinkGeometryState::default(),
        ));
        app.update();
        assert!(!app.world().get::<WifiState>(a).unwrap().peers[0].connected);
    }
}
