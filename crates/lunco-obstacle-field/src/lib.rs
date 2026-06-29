//! Procedural **crater + rock field** generation for rover testing.
//!
//! Generate obstacle fields on the fly with tunable distribution parameters
//! (density, size distribution, spatial pattern, seed) so a rover can be tested
//! across varied surface conditions. The generation core is **pure and
//! deterministic** — the same `(spec, seed)` always yields the same field, so
//! networking replicates only the [`ObstacleFieldSpec`], and an experiment sweep
//! just varies its numbers.
//!
//! Layers:
//! - [`spec`] — the tunable knobs.
//! - [`sampler`] — deterministic placement (ChaCha8, pure, off-thread-safe).
//! - [`field`] — synthesised height surface: craters stamped as bowls, with an
//!   analytic `height_at` for raycast-free rock placement.
//! - [`assets`] — size-bucket quantization (shared meshes/colliders).
//! - [`plugin`] — the Bevy [`ObstacleFieldPlugin`] that wires it into the world.
//!
//! See `PLAN.md` for the phased roadmap (streaming, dynamics, tuning UI, bake
//! cache, experiment sweep).

pub mod assets;
pub mod field;
pub mod plugin;
pub mod rock;
pub mod sampler;
pub mod spec;

pub use field::{grid_indices, grid_normals};
pub use plugin::{grid_mesh, ObstacleFieldPlugin, ObstacleFieldRoot, RegenerateField};
pub use spec::{ObstacleFieldSpec, Pattern};
