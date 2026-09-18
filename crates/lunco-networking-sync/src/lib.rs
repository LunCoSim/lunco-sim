//! Transport-neutral synchronization runtime.
//!
//! This package owns the Bevy systems and data contracts for replicated state,
//! journal edits, and scenario asset distribution. It deliberately has no
//! lightyear or WebTransport dependency. The `lunco-networking` package is
//! the transport adapter: it registers its channels and ferries the envelopes
//! produced and consumed here.

pub mod http_fetch;
pub mod journal_plane;
pub mod scenario_sync;
pub mod sync;

/// Whether the network synchronization schedule should be active for this
/// world. Standalone worlds keep all sync resources available for composition,
/// but do not run network producers/consumers until a real peer role exists.
pub fn wire_is_live(role: Option<bevy::ecs::system::Res<lunco_core_session::NetworkRole>>) -> bool {
    role.is_some_and(|role| role.is_networked())
}

pub mod codec {
    //! Bounded bincode envelope codec shared by every transport adapter.

    use crate::sync::SyncEnvelope;

    /// Maximum decoded envelope size. This bounds both the received frame and
    /// bincode's length-prefixed allocations before a payload is inspected.
    pub const MAX_ENVELOPE_BYTES: usize = 16 * 1024 * 1024;

    /// Serialize one synchronization envelope using the canonical bincode
    /// configuration. Encoding errors are reported and returned to the adapter.
    pub fn serialize_env(env: &SyncEnvelope) -> Option<Vec<u8>> {
        match bincode::serde::encode_to_vec(env, bincode::config::standard()) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                bevy::log::warn!("[sync] envelope encode failed: {error}");
                None
            }
        }
    }

    /// Deserialize one synchronization envelope after applying the shared
    /// hard allocation limit. Malformed or oversized frames are rejected.
    pub fn deserialize_env(bytes: &[u8]) -> Option<SyncEnvelope> {
        if bytes.len() > MAX_ENVELOPE_BYTES {
            bevy::log::warn!(
                "[sync] envelope decode rejected: {} bytes exceeds cap",
                bytes.len()
            );
            return None;
        }
        let config = bincode::config::standard().with_limit::<MAX_ENVELOPE_BYTES>();
        match bincode::serde::decode_from_slice::<SyncEnvelope, _>(bytes, config) {
            Ok((envelope, _)) => Some(envelope),
            Err(error) => {
                bevy::log::warn!(
                    "[sync] envelope decode failed ({} bytes): {error}",
                    bytes.len()
                );
                None
            }
        }
    }
}
