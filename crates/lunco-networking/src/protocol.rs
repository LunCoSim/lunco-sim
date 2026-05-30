//! lightyear protocol: two messages-on-channels carrying our serialized
//! envelopes. Tiny + stable on purpose — all semantics live in `lunco-api`.

use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

/// One serialized [`lunco_api::WireEnvelope`] on the wire.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Frame(pub Vec<u8>);

/// Reliable, ordered channel — commands, handshake, spawn replication.
pub(crate) struct CmdChannel;

/// Best-effort channel — state snapshots (latest-ish wins).
pub(crate) struct SnapChannel;

/// Registers the message type + the two channels, both bidirectional.
pub(crate) struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        app.register_message::<Frame>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_channel::<CmdChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.add_channel::<SnapChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
    }
}
