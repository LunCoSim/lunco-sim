//! Thin re-export of the time-warp gate. The mission-time spine itself —
//! `MissionClock`/`TimeTransport`/`WorldTime`, the derivation step, and the
//! wall-clock seed — lives in `lunco-time` (doc 19); celestial just reads it.
pub use lunco_core::TimeWarpState;
