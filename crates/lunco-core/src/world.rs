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
use big_space::prelude::{BigSpace, BigSpaceSystems, CellCoord, FloatingOrigin, Grid};

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

    // BigSpace root + the `WorldRoot` marker (so subsystems attach under it).
    // It carries the full spatial bundle INCLUDING `Transform`: Avian's physics
    // transform handling follows the standard bevy convention (parentless root
    // with a `Transform`), and removing it silently breaks every collider under
    // the tree (rovers free-fall through the ground). The `Transform` also
    // makes big_space's bevy-compat pass treat this tree as a plain
    // low-precision root and re-propagate it with f32 math (dropping
    // `CellCoord`s) — racing `propagate_high_precision` for every
    // `GlobalTransform`, which strobed site-anchored scenes (cells ~5e7 on the
    // Solar Grid → losing frames rendered the Moon 1e11 m away). That race is
    // resolved by ORDERING, not by removing the `Transform`: `WorldShellPlugin`
    // constrains `BigSpaceSystems::PropagateHighPrecision` to run AFTER the
    // compat pass, so the high-precision writer deterministically wins.
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
            .add_systems(Startup, setup_world.in_set(WorldShellSet))
            // Enforce the `OriginAnchor`'s documented role — the *default* holder
            // of the origin — every frame, not just at startup. Runs in
            // `PostUpdate` immediately before big_space's own origin finder
            // (`RecenterLargeTransforms`) so the anchor reclaims the origin in
            // the same frame a claiming camera is despawned: big_space never sees
            // zero origins, so there is no error and no propagation gap.
            .add_systems(
                PostUpdate,
                anchor_owns_origin_by_default
                    .before(BigSpaceSystems::RecenterLargeTransforms),
            );

        // Kill the dual-propagation race deterministically. big_space registers
        // BOTH its high-precision propagation AND a plain bevy-compat
        // `propagate_parent_transforms` in `TransformSystems::Propagate` with
        // no mutual ordering. Our `WorldRoot` is a parentless entity WITH a
        // `Transform` (Avian's physics transform handling requires the standard
        // convention — see `ensure_world_root`), so the compat pass re-walks
        // the WHOLE big_space tree with plain f32 math, dropping `CellCoord`s.
        // Unordered, the per-frame winner is nondeterministic: invisible while
        // all cells ≈ 0, a whole-frame white/black strobe in site-anchored
        // scenes (Solar Grid at ~5e7 cells → losing frames put the Moon 1e11 m
        // away). Constraining the high-precision set AFTER the compat system
        // makes big_space overwrite the plain values every frame, in both
        // schedules big_space registers them in.
        app.configure_sets(
            PostStartup,
            BigSpaceSystems::PropagateHighPrecision
                .after(big_space::bevy_compat::propagate_parent_transforms),
        );
        app.configure_sets(
            PostUpdate,
            BigSpaceSystems::PropagateHighPrecision
                .after(big_space::bevy_compat::propagate_parent_transforms),
        );
    }
}

/// Startup guarantee — create the shell up front (race-free). Correctness does
/// not depend on this running before other consumers: they all call
/// [`ensure_world_root`], which is create-or-get.
fn setup_world(world: &mut World) {
    ensure_world_root(world);
}

/// Enforces the invariant the [`OriginAnchor`] doc promises: it is the *default*
/// holder of the single `FloatingOrigin`, present even with no camera.
///
/// big_space mandates exactly one `FloatingOrigin` per `BigSpace` but provides no
/// recovery — its [`BigSpace::find_floating_origin`] only logs an error on zero
/// (and, by design, the origin "doesn't need to be a camera"). A camera (avatar,
/// celestial observer, …) *claims* the origin from the anchor for precision while
/// it lives; when that camera is despawned — e.g. `ClearScene` empties the
/// viewport, leaving an intentionally camera-less world — the origin would
/// vanish. This hands it back to the persistent anchor (exactly where a headless
/// server keeps it), so the cleared scene correctly stays camera-less while the
/// coordinate frame survives. No-op on every frame a camera holds the origin.
fn anchor_owns_origin_by_default(
    mut commands: Commands,
    q_origins: Query<(), With<FloatingOrigin>>,
    q_anchor: Query<Entity, With<OriginAnchor>>,
) {
    if !q_origins.is_empty() {
        return;
    }
    if let Some(anchor) = q_anchor.iter().next() {
        commands.entity(anchor).insert(FloatingOrigin);
    }
}
