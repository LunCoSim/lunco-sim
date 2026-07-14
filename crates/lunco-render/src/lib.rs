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

/// The systems that BIND a look intent (`PbrLook`, …) onto an entity as a
/// concrete render component — everything in `lunco-render-bevy` that queues
/// `insert(MeshMaterial3d(..))` for a changed look.
///
/// This set exists to make the binders *nameable*, not to carry an ordering
/// contract. The ordering problem it was briefly used for is solved structurally
/// instead: the USD projector — which despawns and rebuilds the subtree of any
/// edited prim — runs in **`PreUpdate`**, a frame earlier than every binder here.
/// So a binder cannot queue an insert against an entity the projector is about to
/// despawn, and no binder has to remember to opt into an ordering rule (7 crates
/// bind looks; a rule each of them must remember is a rule that gets forgotten).
#[derive(bevy::ecs::schedule::SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LookRebind;
