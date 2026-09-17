//! Transport-side connection state shared by deep-link adapters and the UI.

use bevy::prelude::*;

/// A connection request from an untrusted deep link awaiting user confirmation.
///
/// The networking adapter seeds this resource from native arguments or the
/// browser URL. The UI turns an accepted request into the typed `JoinServer`
/// command and clears it on either decision.
#[derive(Resource, Clone, Debug, Default)]
pub struct PendingConnect {
    /// The pending link, or `None` when nothing awaits.
    pub request: Option<PendingConnectRequest>,
}

/// The address and optional certificate digest a [`PendingConnect`] will dial.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConnectRequest {
    /// `host:port` — hostname or `ip:port`.
    pub address: String,
    /// Self-signed certificate digest to pin, or empty for CA validation.
    pub digest: String,
}
