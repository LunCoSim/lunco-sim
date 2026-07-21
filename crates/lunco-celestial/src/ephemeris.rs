//! # Ephemeris abstraction
//!
//! Defines the [`EphemerisProvider`] trait and the [`EphemerisResource`] that
//! systems in this crate query (missions, trajectories, body positioning).
//! No heavy planetary-theory dependencies live here — they're in the sibling
//! crate `lunco-celestial-ephemeris`, which provides
//! `CelestialEphemerisProvider` (VSOP2013 + ELP/MPP02 + JPL Horizons CSV)
//! and an `EphemerisPlugin` that drops it into `EphemerisResource`.
//!
//! Apps that don't add `lunco-celestial-ephemeris` get the [`NoOpEphemerisProvider`]
//! installed by [`crate::CelestialPlugin`]: bodies stay put (every position
//! returns zero). That's fine for the Modelica workbench and any sandbox
//! scene that places bodies explicitly; orbital sims add the heavy crate.

use bevy::prelude::*;
use bevy::math::DVec3;
use crate::frames::EclipticAu;

use std::sync::Arc;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct CsvDataPoint {
    pub jd: f64,
    /// **Ecliptic** J2000, AU — per the JPL `REF_PLANE=ECLIPTIC` request in the mission JSON.
    ///
    /// NOTE: that request is still trusted, not validated. A mission JSON asking for `FRAME`
    /// would re-introduce the Shackleton bug for that one body, silently. The newtype makes
    /// the *downstream* plumbing safe; it cannot check what JPL was asked for.
    pub pos_au: EclipticAu,
}

/// Abstract interface for any system providing spatial state over time.
pub trait EphemerisProvider: Send + Sync + 'static {
    /// The position of a body relative to its parent.
    ///
    /// The frame is now in the TYPE. It used to be in this sentence — and the provider
    /// returned equatorial vectors anyway, which is how the sun ended up 45° below the
    /// horizon at Shackleton. A doc comment cannot be type-checked.
    /// `None` ⇒ **this provider has no data for that body.**
    ///
    /// P8(d). It used to return `DVec3::ZERO`, which is a *position* — so a body whose CSV
    /// failed to fetch rendered at its parent's centre, **indistinguishable from a valid
    /// result**. A failed Mars fetch put Mars inside the Sun and nothing anywhere said so.
    /// Zero is a plausible answer, and that is exactly what made it dangerous: an error that
    /// looks like data is worse than a crash.
    ///
    /// Callers must now decide what "no ephemeris" means. Almost always the answer is *skip
    /// this body* — which is what they now do.
    fn position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu>;

    /// The body's parent in the gravitational hierarchy. `None` ⇒ it is already heliocentric
    /// (or unknown).
    ///
    /// P8(c). This used to be a `match` hardcoded inside `global_position`
    /// (`399→3, 301→3, 3→10, -1024→399`) while `BodyDescriptor::parent_id` **already carried
    /// the same tree** — two sources of truth for the shape of the solar system. And they had
    /// already diverged: the `match` knew about mission id `-1024`, the registry did not. The
    /// tree now lives in exactly one place, the registry, and providers read it from there.
    fn parent_id(&self, _body_id: i32) -> Option<i32> {
        None
    }

    /// Heliocentric position, by walking the parent tree.
    ///
    /// A missing link anywhere in the chain yields `None`: a body whose parent has no ephemeris
    /// has no meaningful heliocentric position either, and inventing one is how it ended up at
    /// the Sun's centre in the first place.
    fn global_position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        let mut pos = self.position(body_id, epoch_jd)?;
        let mut current_id = body_id;

        // Walk up to the Sun (NAIF 10), which IS the origin of this frame.
        for _ in 0..10 {
            let Some(parent_id) = self.parent_id(current_id) else { break };
            if parent_id == SUN_NAIF_ID {
                break;
            }
            pos += self.position(parent_id, epoch_jd)?;
            current_id = parent_id;
        }
        Some(pos)
    }
}

/// The Sun. The origin of the heliocentric frame — walking the parent tree stops here.
pub const SUN_NAIF_ID: i32 = 10;

/// Thread-safe resource facilitating access to the active ephemeris engine.
#[derive(Resource)]
pub struct EphemerisResource {
    pub provider: Arc<dyn EphemerisProvider>,
}

/// Returns zero for every body at every epoch. Installed by default so
/// downstream systems that depend on `Res<EphemerisResource>` don't panic.
/// Apps that want real planetary positions add `lunco-celestial-ephemeris`
/// and its `EphemerisPlugin`, which overwrites the resource.
pub struct NoOpEphemerisProvider;

impl EphemerisProvider for NoOpEphemerisProvider {
    /// `None`, not zero. "I have no data" is not the same statement as "it is at the origin",
    /// and conflating them is the whole of P8(d). Systems now SKIP a body they cannot place,
    /// instead of drawing it inside the Sun.
    fn position(&self, _body_id: i32, _epoch_jd: f64) -> Option<EclipticAu> {
        None
    }
}


#[cfg(test)]
mod p8_tests {
    use super::*;
    use crate::frames::EclipticAu;
    use bevy::math::DVec3;

    /// A provider that knows where Earth is, knows the tree, and has NOTHING for a mission
    /// whose CSV never arrived.
    struct Partial;
    impl EphemerisProvider for Partial {
        fn position(&self, body_id: i32, _jd: f64) -> Option<EclipticAu> {
            match body_id {
                10 => Some(EclipticAu::ZERO),
                3 => Some(EclipticAu::new(DVec3::new(1.0, 0.0, 0.0))),
                399 => Some(EclipticAu::new(DVec3::new(0.00001, 0.0, 0.0))),
                _ => None, // the mission's fetch failed
            }
        }
        fn parent_id(&self, body_id: i32) -> Option<i32> {
            match body_id {
                399 => Some(3),
                3 => Some(SUN_NAIF_ID),
                -1024 => Some(399),
                _ => None,
            }
        }
    }

    /// P8(d). A body with no ephemeris must be `None` — NOT the origin.
    ///
    /// It used to return `DVec3::ZERO`, a perfectly plausible *position*, so a mission whose
    /// CSV failed to download rendered at its parent's centre and looked exactly like a real
    /// result. An error that resembles data is worse than a crash.
    #[test]
    fn a_body_with_no_ephemeris_is_none_not_the_origin() {
        assert!(
            Partial.position(-1024, 2_451_545.0).is_none(),
            "no data must be None — ZERO is a position, and it is INSIDE Earth"
        );
        assert!(
            Partial.global_position(-1024, 2_451_545.0).is_none(),
            "and it must not be laundered into a real heliocentric position by the parent walk"
        );
    }

    /// …while a body it DOES have still resolves, through the tree.
    #[test]
    fn a_known_body_still_resolves_through_the_parent_chain() {
        // Earth = Earth-rel-EMB + EMB-rel-Sun.
        let earth = Partial.global_position(399, 2_451_545.0).expect("Earth is known");
        assert!((earth.raw().x - 1.00001).abs() < 1.0e-9, "the parent walk must still compose");
    }

    /// P8(c). The parent tree is asked for, not hardcoded. A provider that declines to describe
    /// the hierarchy gets a flat one — it does NOT get a secret `match` that knows about Earth.
    #[test]
    fn the_parent_tree_comes_from_the_provider_not_a_hardcoded_match() {
        struct Flat;
        impl EphemerisProvider for Flat {
            fn position(&self, _id: i32, _jd: f64) -> Option<EclipticAu> {
                Some(EclipticAu::new(DVec3::new(1.0, 0.0, 0.0)))
            }
            // no `parent_id` override ⇒ no tree
        }
        // With the old hardcoded match, 399 would have walked 399→3→10 and summed THREE
        // positions (3.0). With the tree supplied by the provider — and this one supplies
        // none — it is just the body's own position.
        let p = Flat.global_position(399, 0.0).unwrap();
        assert_eq!(p.raw().x, 1.0, "no tree ⇒ no parent walk; the match no longer exists");
    }
}
