//! Shared authored connector names used by environmental and celestial bridges.
//!
//! These are data-contract names only. Runtime behavior belongs to the owning
//! environment or celestial domain, while connections remain generic.

/// SimComponent **output** connector carrying an entity's local gravitational
/// acceleration magnitude (m/s²).
///
/// Cosim itself never produces this value — it would have to hardcode a
/// constant, and the master algorithm stays domain-agnostic. Instead a domain
/// system (lunco-environment's gravity bridge) writes the entity's real
/// [`LocalGravity`](https://docs.rs) magnitude into this output each tick, so
/// gravity flows through an ordinary output→input [`crate::SimConnection`] like
/// any other signal — correct on the Moon, Earth, or any body.
pub const GRAVITY_SOURCE_CONNECTOR: &str = "gravity_accel";
pub const GRAVITY_X_SOURCE_CONNECTOR: &str = "gravity_x";
pub const GRAVITY_Y_SOURCE_CONNECTOR: &str = "gravity_y";
pub const GRAVITY_Z_SOURCE_CONNECTOR: &str = "gravity_z";

/// SimComponent output connectors carrying the unit direction **toward the
/// Sun** in the consumer's authored mount frame.  The convention is shared with
/// Earth tracking: `+X` right, `+Y` up, `-Z` forward.  Environment systems
/// publish vectors; models alone convert them to their joint coordinates.
pub const SUN_MOUNT_X_CONNECTOR: &str = "sun_mount_x";
pub const SUN_MOUNT_Y_CONNECTOR: &str = "sun_mount_y";
pub const SUN_MOUNT_Z_CONNECTOR: &str = "sun_mount_z";

/// SimComponent output connectors carrying the unit direction **toward Earth**
/// in the consumer's authored mount frame.  `+X` is mount-right, `+Y` mount-up,
/// and `-Z` mount-forward.  The bridge performs one complete inverse mount
/// rotation; a pointing model then performs the one documented vector→joint
/// conversion.  Passing site azimuth/elevation to a mount-frame joint is
/// intentionally not supported because it is ambiguous for pitched or rolled
/// vehicles.
pub const EARTH_MOUNT_X_CONNECTOR: &str = "earth_mount_x";
pub const EARTH_MOUNT_Y_CONNECTOR: &str = "earth_mount_y";
pub const EARTH_MOUNT_Z_CONNECTOR: &str = "earth_mount_z";

/// The complete output contract of a `LunCoEnvironmentProbeAPI` source prim.
///
/// These are schema-declared properties, so they are not necessarily present in
/// a live prim's authored `property_names()` list. The USD runtime projection
/// uses this contract to materialize the source-side port surface that the
/// environment domain fills and ordinary USD connections consume.
pub const ENVIRONMENT_PROBE_OUTPUTS: &[&str] = &[
    GRAVITY_SOURCE_CONNECTOR,
    GRAVITY_X_SOURCE_CONNECTOR,
    GRAVITY_Y_SOURCE_CONNECTOR,
    GRAVITY_Z_SOURCE_CONNECTOR,
    SUN_MOUNT_X_CONNECTOR,
    SUN_MOUNT_Y_CONNECTOR,
    SUN_MOUNT_Z_CONNECTOR,
    EARTH_MOUNT_X_CONNECTOR,
    EARTH_MOUNT_Y_CONNECTOR,
    EARTH_MOUNT_Z_CONNECTOR,
];

/// Prefix of the SimComponent **output** connectors `lunco-celestial`'s link bridge
/// writes on every link node, one set per authored peer `class`:
///
/// ```text
/// link_<class>_range_m        metres to the best peer of that class
/// link_<class>_connected      1.0 = geometry closes, 0.0 = severed
/// link_<class>_elevation_deg  that peer's elevation above the local horizon
/// ```
///
/// Same contract as [`SUN_MOUNT_X_CONNECTOR`]: cosim stays domain-agnostic and a domain
/// system writes the real value each solve, so an RF model (`CommsLink.mo`) receives it
/// through an ordinary output→input [`crate::SimConnection`].
///
/// WHY PER CLASS: `LinkState` already hands every peer to anything that can hold a list
/// (rhai, the API, the UI). Cosim is the one consumer that cannot — a Modelica port is a
/// fixed scalar — so N peers must reduce. `class` is the authored routing group that
/// exists for exactly this (three DSN complexes all author `class = "earth"`), which
/// keeps the choice of link with the AUTHOR:
///
/// ```usda
/// float inputs:link_range_m.connect = </…/Comms.outputs:link_relay_range_m>
/// ```
///
/// The model keeps generic inputs; the connection picks the link. Same `CommsLink.mo`
/// serves a relay uplink here and direct-to-Earth there, and a two-radio vehicle
/// instantiates it twice — no policy in the kernel, none in the model.
///
/// The kernel publishes GEOMETRY and only geometry: metres and a verdict, never a data
/// rate. Turning metres into bits/s is the authored channel model's job. And `connected`
/// is the geometry verdict ALONE — a peer can be in plain sight and still too far to
/// close the link budget; that verdict belongs to the model.
pub const LINK_CONNECTOR_PREFIX: &str = "link_";
