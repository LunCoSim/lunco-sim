//! lightyear protocol: two messages-on-channels carrying our serialized
//! envelopes. Tiny + stable on purpose — all semantics live in `lunco-api`.

use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

/// One serialized [`crate::sync::SyncEnvelope`] on the wire.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct Frame(pub Vec<u8>);

/// Reliable, ordered channel — commands, handshake, spawn replication.
pub(crate) struct CmdChannel;

/// Best-effort channel — state snapshots (latest-ish wins).
pub(crate) struct SnapChannel;

/// Reliable, ordered channel for **bulk** payloads — the scenario manifest
/// (and, Phase 3, the asset chunk stream). Separate from [`CmdChannel`] so a
/// large manifest / a multi-MB asset transfer can't head-of-line-block the
/// join-critical, latency-sensitive traffic on `CmdChannel` (Handshake,
/// Ownership, Profiles, PossessVessel, spawn). Both are `OrderedReliable`;
/// they're independent lightyear channels, so backpressure on one doesn't
/// stall the other.
pub(crate) struct BulkChannel;

/// Registers the message type + the three channels, all bidirectional.
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

        app.add_channel::<BulkChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        })
        .add_direction(NetworkDirection::Bidirectional);
    }
}
