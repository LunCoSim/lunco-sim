//! Built-in **overzoom** layer — authorable procedural sub-DEM detail.
//!
//! Wraps [`lunco_terrain_core::Overzoom`] (deterministic craterlet population +
//! FBM micro-relief below the DEM's data resolution) as a composable
//! `lunco:layer = "overzoom"` prim. The synthesis is *invented-but-plausible*
//! ground — scientifically-honest scenes simply omit the layer; scenes that want
//! the close-up lunar look author it with a seed. Nyquist gating (per-consumer
//! attenuation) is applied downstream by [`SurfaceOracle::detail_limited`]
//! (crate::oracle::SurfaceOracle::detail_limited), not here.

use std::sync::Arc;

use lunco_terrain_core::Overzoom;

use super::{LayerAttrSource, TerrainLayer};
use crate::oracle::HeightContribution;

struct OverzoomLayer {
    spec: Overzoom,
}

impl TerrainLayer for OverzoomLayer {
    fn id(&self) -> &'static str {
        "overzoom"
    }

    fn height_modifier(&self, _half_extent: f32) -> Option<HeightContribution> {
        let s = &self.spec;
        let mut key = lunco_precompute::Fnv1a::new();
        // Synthesis-algorithm version: bump when the Overzoom math changes with
        // identical params (Poisson counts / domain warp / rim variety = v2;
        // degradation-tied craterlet bowl shape = v3), so content-addressed
        // tiles + derived maps re-bake instead of serving the old field.
        key.write_u64(3);
        key.write_u64(s.seed);
        key.write_u64(s.max_radius.to_bits());
        key.write_u64(s.min_radius.to_bits());
        key.write_u64(s.crater_mean.to_bits());
        key.write_u64(s.depth_ratio.0.to_bits());
        key.write_u64(s.depth_ratio.1.to_bits());
        key.write_u64(s.relief_amp.to_bits());
        key.write_u64(s.relief_scale.to_bits());
        Some(HeightContribution {
            modifier: Arc::new(s.clone()),
            content_key: key.finish(),
        })
    }
}

/// The default sub-DEM detail layer (all [`Overzoom::default`] parameters:
/// 0.4–2 m craterlets handing off to the crater layer's 2 m SFD floor + FBM
/// micro-relief). The USD bridge folds this in when a terrain authors no
/// `overzoom` prim of its own — without SOME sub-DEM signal the ground between
/// the finest shader grain (~12 cm) and the DEM resolution (~5 m) is empty in
/// every channel and reads as flat plastic one step from the camera. A scene
/// that wants scientifically-honest bare interpolation authors an `overzoom`
/// prim with `amplitude = 0` and `density = 0`.
pub fn default_overzoom_layer() -> Arc<dyn TerrainLayer> {
    Arc::new(OverzoomLayer {
        spec: Overzoom::default(),
    })
}

/// Parse a `lunco:layer = "overzoom"` prim:
/// - `amplitude` — micro-relief amplitude (m), default 0.08; `0` disables relief;
/// - `reliefScale` — coarsest relief wavelength (m), default 14;
/// - `maxFeature` / `minFeature` — synthetic craterlet radius range (m), default 6 / 0.4;
/// - `density` — mean craterlets per band cell, default 0.9; `0` disables craterlets;
/// - `seed` — determinism seed.
///
/// Returns `None` (layer disabled) when both channels are zeroed.
pub(super) fn parse_overzoom_layer(a: &dyn LayerAttrSource) -> Option<Arc<dyn TerrainLayer>> {
    let defaults = Overzoom::default();
    let relief_amp = a
        .get_f32("amplitude")
        .map(f64::from)
        .unwrap_or(defaults.relief_amp);
    let crater_mean = a
        .get_f32("density")
        .map(f64::from)
        .unwrap_or(defaults.crater_mean);
    if relief_amp <= 0.0 && crater_mean <= 0.0 {
        return None;
    }
    let spec = Overzoom {
        seed: a.get_i64("seed").map(|s| s as u64).unwrap_or(defaults.seed),
        max_radius: a
            .get_f32("maxFeature")
            .map(f64::from)
            .unwrap_or(defaults.max_radius),
        min_radius: a
            .get_f32("minFeature")
            .map(f64::from)
            .unwrap_or(defaults.min_radius),
        crater_mean,
        relief_amp,
        relief_scale: a
            .get_f32("reliefScale")
            .map(f64::from)
            .unwrap_or(defaults.relief_scale),
        ..defaults
    };
    Some(Arc::new(OverzoomLayer { spec }))
}
