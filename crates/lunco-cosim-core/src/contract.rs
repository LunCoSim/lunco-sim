//! Shared authored connector names used by environmental and celestial bridges.
//!
//! These are data-contract names only. Runtime behavior belongs to the owning
//! environment or celestial domain, while connections remain generic.

/// Stable source identifier carried by a framed direction output triplet.
///
/// A probe output is named `<id>_mount_x/y/z`; consumers choose their own input
/// names and connect them to the selected source. Keeping this parser beside
/// the cosim port contract gives USD authoring, wiring, and environment code
/// one identity grammar.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DirectionSourceId(String);

impl DirectionSourceId {
    /// Parse lower-case ASCII ids beginning with a letter and containing only
    /// letters, digits, and underscores. A trailing underscore is forbidden.
    pub fn parse(value: &str) -> Option<Self> {
        let mut chars = value.chars();
        let first = chars.next()?;
        (first.is_ascii_lowercase()
            && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
            && !value.ends_with('_'))
        .then(|| Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn mount_connector(&self, axis: char) -> Option<String> {
        matches!(axis, 'x' | 'y' | 'z').then(|| format!("{}_mount_{axis}", self.0))
    }

    /// Resolve a direction-source output connector to its stable id.
    pub fn from_mount_connector(connector: &str) -> Option<Self> {
        let (source, axis) = connector.rsplit_once("_mount_")?;
        if !matches!(axis, "x" | "y" | "z") {
            return None;
        }
        Self::parse(source)
    }
}

#[cfg(test)]
mod direction_source_tests {
    use super::DirectionSourceId;

    #[test]
    fn direction_source_ids_round_trip_through_axis_connectors() {
        for axis in ['x', 'y', 'z'] {
            let source = DirectionSourceId::parse("spacecraft_7").unwrap();
            let connector = source.mount_connector(axis).unwrap();
            assert_eq!(
                DirectionSourceId::from_mount_connector(&connector),
                Some(source.clone())
            );
        }
        assert!(DirectionSourceId::parse("7_spacecraft").is_none());
        assert!(DirectionSourceId::parse("spacecraft_").is_none());
        assert!(DirectionSourceId::from_mount_connector("spacecraft_mount_w").is_none());
    }
}

/// SimComponent **output** connector carrying the magnitude of local
/// gravitational acceleration (m/s²).
///
/// Cosim itself never produces this value — it would have to hardcode a
/// constant, and the master algorithm stays domain-agnostic. Instead a domain
/// system (lunco-environment's gravity bridge) writes the entity's real
/// [`LocalGravity`](https://docs.rs) magnitude into this output each tick, so
/// gravity flows through an ordinary output→input [`crate::SimConnection`] like
/// any other signal — correct on the Moon, Earth, or any body.
pub const GRAVITY_SOURCE_CONNECTOR: &str = "gravity_accel";
/// Local gravity vector components expressed in the probe frame (m/s²).
pub const GRAVITY_X_SOURCE_CONNECTOR: &str = "gravity_x";
pub const GRAVITY_Y_SOURCE_CONNECTOR: &str = "gravity_y";
pub const GRAVITY_Z_SOURCE_CONNECTOR: &str = "gravity_z";

/// Static output contract of a `LunCoEnvironmentProbeAPI` source prim.
///
/// These are schema-declared properties, so they are not necessarily present in
/// a live prim's authored `property_names()` list. The USD runtime projection
/// uses this contract to materialize fixed environmental scalars. Direction
/// outputs are source-identified and materialized from composed wire demand by
/// the generic environment publisher.
pub const ENVIRONMENT_PROBE_BASE_OUTPUTS: &[&str] = &[
    GRAVITY_SOURCE_CONNECTOR,
    GRAVITY_X_SOURCE_CONNECTOR,
    GRAVITY_Y_SOURCE_CONNECTOR,
    GRAVITY_Z_SOURCE_CONNECTOR,
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
/// Cosim stays domain-agnostic and a domain system writes the real value each
/// solve, so an RF model (`CommsLink.mo`) receives it through an ordinary
/// output→input [`crate::SimConnection`].
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
