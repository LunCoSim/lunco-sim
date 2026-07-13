//! Shared render-look configuration for LunCoSim.
//!
//! This crate is the single, render-capable home that sits below every 3D crate
//! (`lunco-celestial`, `lunco-usd-bevy`, `lunco-environment`, the binaries) so
//! they can agree on "what the scene's look is" by construction instead of by
//! copy-paste. It depends only on `lunco-core` + the lightweight `bevy_light`
//! component types, so it never forms a cycle and never drags the `bevy_pbr`
//! render pipeline into the slim web/Modelica binaries.
//!
//! Today it owns [`sun::LunarSunShadow`] (the canonical sun-shadow spec). It is
//! the intended home for the rest of the render-look roadmap — exposure /
//! earthshine, anti-aliasing, sky/Earth, and the `RenderSettings` window
//! backing.

pub mod appearance;
pub mod camera;
pub mod sun;

pub use appearance::{PbrLook, PbrLookKey, PbrTextures, SurfaceAlpha};
pub use camera::{BloomLook, MsaaLevel, SceneCamera, ToneMap, WorldLabel};
pub use sun::LunarSunShadow;
