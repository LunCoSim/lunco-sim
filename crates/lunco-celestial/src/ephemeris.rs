//! # Ephemeris abstraction
//!
//! Defines the [`EphemerisProvider`] trait and the [`EphemerisResource`] that
//! celestial body placement and lighting query.
//! No heavy planetary-theory dependencies live here — they're in the sibling
//! crate `lunco-celestial-ephemeris`, which provides
//! `CelestialEphemerisProvider` (VSOP2013 + ELP/MPP02)
//! and an `EphemerisPlugin` that drops it into `EphemerisResource`.
//!
//! Ephemeris is an explicit provider. Apps that need orbital placement add
//! `lunco-celestial-ephemeris`; scenes without that provider remain valid only
//! for authored, non-orbital celestial content. No synthetic provider is
//! installed to hide a missing dependency.

use crate::frames::EclipticAu;
use bevy::prelude::*;

use std::sync::Arc;

/// Abstract interface for any system providing spatial state over time.
pub trait EphemerisProvider: Send + Sync + 'static {
    /// Position of a body relative to its parent in ecliptic J2000 astronomical units.
    /// `None` means the provider has no position for that body at the requested epoch.
    fn position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu>;

    /// A certified upper bound for the angular rate of every position this
    /// provider can return, in radians per simulated day.
    ///
    /// The cadence policy uses this bound to turn its geometric angular-error
    /// tolerance into an epoch step. `f64::INFINITY` means that the provider
    /// cannot certify a bound; the caller then solves every frame. Returning a
    /// guessed constant here would be worse than the extra solve because it
    /// would make a visibly stale frame look valid.
    fn maximum_angular_rate_rad_per_day(&self) -> f64;

    /// Parent body id in the provider's hierarchy. `None` means the body is
    /// heliocentric or the provider has no parent fact for it.
    fn parent_id(&self, _body_id: i32) -> Option<i32> {
        None
    }

    /// Heliocentric position, by walking the parent tree.
    ///
    /// A missing position anywhere in the chain yields `None`; the provider does
    /// not invent an origin position for a body with incomplete state.
    fn global_position(&self, body_id: i32, epoch_jd: f64) -> Option<EclipticAu> {
        let mut pos = self.position(body_id, epoch_jd)?;
        let mut current_id = body_id;

        // Walk up to the Sun (NAIF 10), which IS the origin of this frame.
        for _ in 0..10 {
            let Some(parent_id) = self.parent_id(current_id) else {
                break;
            };
            if parent_id == crate::ephemeris_id::SUN {
                break;
            }
            pos += self.position(parent_id, epoch_jd)?;
            current_id = parent_id;
        }
        Some(pos)
    }
}

/// Thread-safe resource facilitating access to the active ephemeris engine.
#[derive(Resource)]
pub struct EphemerisResource {
    pub provider: Arc<dyn EphemerisProvider>,
}

#[cfg(test)]
mod ephemeris_contract_tests {
    use super::*;
    use crate::frames::EclipticAu;
    use bevy::math::DVec3;

    const TEST_BODY: i32 = 10_024;

    /// A partial provider with a missing non-analytic body.
    struct Partial;
    impl EphemerisProvider for Partial {
        fn position(&self, body_id: i32, _jd: f64) -> Option<EclipticAu> {
            match body_id {
                crate::ephemeris_id::SUN => Some(EclipticAu::ZERO),
                crate::ephemeris_id::EARTH_MOON_BARYCENTER => {
                    Some(EclipticAu::new(DVec3::new(1.0, 0.0, 0.0)))
                }
                crate::ephemeris_id::EARTH => Some(EclipticAu::new(DVec3::new(0.00001, 0.0, 0.0))),
                _ => None,
            }
        }
        fn parent_id(&self, body_id: i32) -> Option<i32> {
            match body_id {
                crate::ephemeris_id::EARTH => Some(crate::ephemeris_id::EARTH_MOON_BARYCENTER),
                crate::ephemeris_id::EARTH_MOON_BARYCENTER => Some(crate::ephemeris_id::SUN),
                TEST_BODY => Some(crate::ephemeris_id::EARTH),
                _ => None,
            }
        }

        fn maximum_angular_rate_rad_per_day(&self) -> f64 {
            0.0
        }
    }

    /// Missing positions stay absent through both local and global lookup.
    #[test]
    fn a_body_with_no_ephemeris_is_none_not_the_origin() {
        assert!(
            Partial.position(TEST_BODY, 2_451_545.0).is_none(),
            "no data must remain absent"
        );
        assert!(
            Partial.global_position(TEST_BODY, 2_451_545.0).is_none(),
            "a missing local position cannot produce a global position"
        );
    }

    /// …while a body it DOES have still resolves, through the tree.
    #[test]
    fn a_known_body_still_resolves_through_the_parent_chain() {
        // Earth = Earth-rel-EMB + EMB-rel-Sun.
        let earth = Partial
            .global_position(crate::ephemeris_id::EARTH, 2_451_545.0)
            .expect("Earth is known");
        assert!(
            (earth.raw().x - 1.00001).abs() < 1.0e-9,
            "the parent walk must still compose"
        );
    }

    /// A provider without parent facts supplies a heliocentric position directly.
    #[test]
    fn the_parent_tree_comes_from_the_provider_not_a_hardcoded_match() {
        struct Flat;
        impl EphemerisProvider for Flat {
            fn position(&self, _id: i32, _jd: f64) -> Option<EclipticAu> {
                Some(EclipticAu::new(DVec3::new(1.0, 0.0, 0.0)))
            }
            fn maximum_angular_rate_rad_per_day(&self) -> f64 {
                0.0
            }
            // no `parent_id` override ⇒ no tree
        }
        let p = Flat
            .global_position(crate::ephemeris_id::EARTH, 0.0)
            .unwrap();
        assert_eq!(
            p.raw().x,
            1.0,
            "a provider without parent facts returns its supplied position"
        );
    }
}
