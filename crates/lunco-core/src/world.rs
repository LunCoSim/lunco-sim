//! The **world shell** — the single, persistent big_space coordinate root that
//! every scene mounts into.
//!
//! ## Why this exists
//!
//! The live 3D world is a `BigSpace` root + a canonical `Grid` (the `WorldGrid`)
//! + exactly one `FloatingOrigin`. Per `docs/architecture/21-domain-usd.md` the
//! Grid is the *rendered projection of the active stage*: switching scenes
//! **re-points** the Grid at new content, it does not rebuild the root. So the
//! shell is a **persistent singleton** — created once, reused across every
//! `LoadScene` / reload / scene-switch.
//!
//! [`ensure_world_root`] is the idempotent **create-or-get** every consumer calls
//! (scene mount, celestial nesting, …) instead of spawning its own root or
//! guessing "the first `Grid`". That removes the two failure modes the old code
//! had: a second stray `BigSpace` root, and a startup race where the root existed
//! before any `FloatingOrigin`.
//!
//! ## Coordinate concern, not a render concern
//!
//! The shell is **render-free and headless-complete**. The single `FloatingOrigin`
//! lives on a neutral [`OriginAnchor`], *not* a camera — so a **server** (no
//! camera at all) still gets correct big_space propagation. A **client** then has
//! a camera (avatar, a rover-built-in camera, free-flight, …) *claim* the origin
//! from the anchor; the camera is an optional consumer of the coordinate frame,
//! owned outside core (e.g. `lunco-avatar`). There is always exactly one
//! `FloatingOrigin`; only its holder changes.

use bevy::prelude::*;
use big_space::prelude::{BigSpace, CellCoord, FloatingOrigin, Grid};

/// Marks the one canonical `Grid` scenes mount under. Consumers query for this
/// marker rather than picking "the first `Grid`" — there may be other grids
/// (celestial scales, preview viewports); this is *the* world grid.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct WorldGrid;

/// Marks the single `BigSpace` root. Other subsystems (e.g. celestial, which
/// nests its solar grids) query this to attach under the *one* root instead of
/// spawning their own.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct WorldRoot;

/// The set [`setup_world`] runs in. Subsystems that need the shell to exist
/// (e.g. celestial's hierarchy) order `.after(WorldShellSet)`. Ordering is a
/// convenience — `ensure_world_root` is create-or-get, so it is never required
/// for correctness, only to avoid a redundant standalone-fallback spawn.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldShellSet;

/// The neutral default holder of the single `FloatingOrigin`. Present even
/// headless (a server has no camera), so big_space always has exactly one
/// origin. A camera, when one exists, claims the `FloatingOrigin` from here.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct OriginAnchor;

/// Tunables for the canonical [`WorldGrid`]. A *resource* so the binary / scene
/// sets the frame (cell size, switching threshold) — core carries no hardcoded
/// opinion. Defaults match the historical sandbox grid (`2000 m` cells).
#[derive(Resource, Debug, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct WorldGridConfig {
    /// Edge length of one grid cell, metres. big_space recentres around the
    /// `FloatingOrigin` in units of this.
    pub cell_edge_length: f32,
    /// Distance from cell centre at which big_space switches the origin's cell.
    pub switching_threshold: f32,
}

impl Default for WorldGridConfig {
    fn default() -> Self {
        Self { cell_edge_length: 2000.0, switching_threshold: 1.0e10 }
    }
}

/// Idempotent **create-or-get** for the world shell. Returns the [`WorldGrid`]
/// entity scenes mount under.
///
/// First call spawns `BigSpace` root → `WorldGrid` → [`OriginAnchor`] (carrying
/// the single `FloatingOrigin`). Subsequent calls return the existing
/// `WorldGrid`. Safe to call from `Startup`, from `LoadScene`, from celestial
/// setup — order-independent, which is the whole point.
pub fn ensure_world_root(world: &mut World) -> Entity {
    let existing = {
        let mut q = world.query_filtered::<Entity, With<WorldGrid>>();
        q.iter(world).next()
    };
    if let Some(grid) = existing {
        return grid;
    }

    let cfg = world
        .get_resource::<WorldGridConfig>()
        .copied()
        .unwrap_or_default();

    // BigSpace root + the `WorldRoot` marker (so subsystems attach under it). It
    // carries the full spatial/visibility bundle so its `WorldGrid` child doesn't
    // trip Bevy's B0004 "parent without GlobalTransform/InheritedVisibility" warning
    // — the root is the identity base of big_space propagation.
    let root = world
        .spawn((
            BigSpace::default(),
            WorldRoot,
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            ViewVisibility::default(),
            Name::new("WorldRoot"),
        ))
        .id();

    // The canonical grid scenes mount under.
    let grid = world
        .spawn((
            Grid::new(cfg.cell_edge_length, cfg.switching_threshold),
            WorldGrid,
            CellCoord::default(),
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
            InheritedVisibility::default(),
            Name::new("WorldGrid"),
            ChildOf(root),
        ))
        .id();

    // The neutral, always-present origin holder (Grid-direct child → big_space
    // propagates it). A camera takes the `FloatingOrigin` over from here when one
    // exists; on a server it stays here forever.
    world.spawn((
        OriginAnchor,
        FloatingOrigin,
        CellCoord::default(),
        Transform::default(),
        GlobalTransform::default(),
        Name::new("OriginAnchor"),
        ChildOf(grid),
    ));

    grid
}

/// Installs the world shell: registers the markers/config and guarantees the
/// shell (and therefore exactly one `FloatingOrigin`) exists from frame 0, so
/// there is never a window where the root has no origin.
///
/// Binaries add this **instead of** spawning their own `BigSpace` root.
pub struct WorldShellPlugin;

impl Plugin for WorldShellPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<WorldGrid>()
            .register_type::<WorldRoot>()
            .register_type::<OriginAnchor>()
            .register_type::<WorldGridConfig>()
            .init_resource::<WorldGridConfig>()
            .add_systems(Startup, setup_world.in_set(WorldShellSet));
    }
}

/// Startup guarantee — create the shell up front (race-free). Correctness does
/// not depend on this running before other consumers: they all call
/// [`ensure_world_root`], which is create-or-get.
fn setup_world(world: &mut World) {
    ensure_world_root(world);
}
