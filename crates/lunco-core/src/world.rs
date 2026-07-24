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
    ///
    /// A **PRECISION** knob, not an extent knob. big_space derives
    /// `maximum_distance_from_origin = cell_edge/2 + switching_threshold`, and
    /// `translation_to_grid` short-circuits below it — returning cell `(0,0,0)`
    /// and the *whole* position as a raw **f32** `Transform`. A large threshold
    /// therefore disables cell binning outright: at 1e10 (the historical value)
    /// every entity inside 1e10 m stayed in cell 0, so f32 ULP alone bounded
    /// precision — **32 m at Earth–Moon distance**, 64 m at 1e9 m.
    ///
    /// Cells are `i64`, so a small threshold costs nothing (1 AU / 2 km ≈ 7.5e7
    /// cells). Keep it at 100 m — the same value the root grid below has always
    /// used, and the same rule `big_space_setup.rs` states for every celestial
    /// grid: f32 ULP at `edge/2 + 100` = 1100 m is ≈ 0.12 mm.
    pub switching_threshold: f32,
}

impl Default for WorldGridConfig {
    fn default() -> Self {
        Self {
            cell_edge_length: 2000.0,
            switching_threshold: 100.0,
        }
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
    //
    // It carries `BigSpace` **and a `Grid`** — big_space's high-precision
    // propagation only writes a root's `GlobalTransform` when both live on the
    // SAME entity (`propagation.rs`: the root query is `(&Grid, &mut
    // GlobalTransform), With<BigSpace>`), and only processes a cell-entity when
    // its direct parent is a `Grid`. Without the root `Grid`, neither the root
    // nor the `WorldGrid` child below ever got an origin-relative
    // `GlobalTransform` from big_space: both were written exclusively by the
    // plain f32 bevy-compat pass — as IDENTITY, always. That was accidentally
    // correct while the floating origin's cell stayed (0,0,0), and became "the
    // world jumps around the camera" the moment the origin travelled (orbital
    // view, doc 47 Phase 6): every Transform-only entity composing off the
    // root/WorldGrid rendered in surface convention while the rest of the
    // world moved in origin-relative convention.
    //
    // The root grid's `switching_threshold` is deliberately SMALL (not the
    // WorldGrid's 1e10): it bounds the f32 remainder of the origin's pose in
    // this grid (`edge/2 + threshold`), i.e. it is a PRECISION knob — see
    // `docs/architecture/46` and the cell-edge rule in `big_space_setup.rs`.
    //
    // NO `Transform` on the root — big_space's canonical root shape (its
    // validator: `BigSpace + Grid + GlobalTransform`, WITHOUT `Transform`/
    // `CellCoord`). A root `Transform` re-arms the plain-f32 bevy-compat
    // pass over this whole tree (racing big_space's writers — held off only
    // by ordering), and it was load-bearing for avian TWICE:
    //
    // 1. avian's default GT-based transform sync — severed by Phase 5's
    //    `BigSpacePhysicsBridgePlugin` (owns Position ↔ cell/Transform).
    // 2. avian's `propagate_collider_transforms`, whose root query skips
    //    Transform-less tree roots, freezing `ColliderTransform` (offset AND
    //    SCALE — `update_collider_scale` reads it). Measured 2026-07-11: the
    //    4000×-scaled sandbox Ground collider collapsed to ~1 m and rovers
    //    sank at ~17 m/s. Severed by the bridge's
    //    `propagate_collider_transforms_rootless`, which computes every
    //    collider's transform from its `ColliderOf` chain with no root
    //    involved (`bridge_physics.rs::scaled_child_collider_ground_*`).
    //
    // Consequence: apps that spawn the world shell AND avian physics MUST
    // register `BigSpacePhysicsBridgePlugin` (the sandbox does).
    let root = world
        .spawn((
            BigSpace::default(),
            Grid::new(cfg.cell_edge_length, 100.0),
            WorldRoot,
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
                anchor_owns_origin_by_default.before(BigSpaceSystems::RecenterLargeTransforms),
            )
            .add_systems(bevy::prelude::First, dbg_origin_bracket_first)
            .add_systems(bevy::prelude::Last, dbg_origin_bracket_last);

        // Named companion to big_space's validator: that one dumps component
        // lists but no `Name`s, which makes chasing a violation a guessing
        // game. This audit logs the one class that actually corrupts poses —
        // a `CellCoord` entity whose direct parent is not a `Grid` (big_space
        // silently skips it, so it renders via the f32 compat convention
        // while everything around it is origin-relative). Warns once per
        // entity. Opt-in: `LUNCO_CELL_AUDIT=1`.
        if std::env::var("LUNCO_CELL_AUDIT").is_ok_and(|v| v == "1") {
            app.init_resource::<CellAuditReported>().add_systems(
                PostUpdate,
                audit_cells_under_non_grid_parents.after(BigSpaceSystems::PropagateHighPrecision),
            );
        }

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
fn dbg_origin_bracket_first(
    mut printed: Local<u32>,
    q: Query<(&CellCoord, &Transform), With<FloatingOrigin>>,
) {
    if *printed > 120 {
        return;
    }
    if let Ok((c, tf)) = q.single() {
        if c.y.saturating_abs() > 3 {
            *printed += 1;
            bevy::log::warn!(
                "[BRACKET-First] cell.y={} tf.y={:.1}",
                c.y,
                tf.translation.y
            );
        }
    }
}

fn dbg_origin_bracket_last(
    mut printed: Local<u32>,
    q: Query<(&CellCoord, &Transform), With<FloatingOrigin>>,
) {
    if *printed > 120 {
        return;
    }
    if let Ok((c, tf)) = q.single() {
        if c.y.saturating_abs() > 3 {
            *printed += 1;
            bevy::log::warn!(
                "[BRACKET-Last ] cell.y={} tf.y={:.1}",
                c.y,
                tf.translation.y
            );
        }
    }
}

fn anchor_owns_origin_by_default(
    mut commands: Commands,
    q_origins: Query<(), With<FloatingOrigin>>,
    q_anchor: Query<Entity, With<OriginAnchor>>,
) {
    if !q_origins.is_empty() {
        return;
    }
    if let Some(anchor) = q_anchor.iter().next() {
        commands.entity(anchor).try_insert(FloatingOrigin);
    }
}

/// Once-per-entity dedup for [`audit_cells_under_non_grid_parents`].
#[derive(Resource, Default)]
pub struct CellAuditReported(bevy::platform::collections::HashSet<Entity>);

/// `LUNCO_CELL_AUDIT=1`: name every `CellCoord` entity whose direct parent is
/// not a `Grid`. big_space's high-precision propagation only processes a
/// cell-entity under a `Grid` parent — anywhere else the `CellCoord` is dead
/// weight and the entity silently falls to the f32 compat pass (doc 45,
/// violation class 2). The fix at the offending spawn/reparent site is to
/// remove the `CellCoord` (plain `Transform` child) or parent to a grid.
fn audit_cells_under_non_grid_parents(
    mut reported: ResMut<CellAuditReported>,
    q_cells: Query<(Entity, &ChildOf), With<CellCoord>>,
    q_grids: Query<(), With<Grid>>,
    q_names: Query<&Name>,
) {
    for (e, child_of) in q_cells.iter() {
        let parent = child_of.parent();
        if q_grids.get(parent).is_ok() || reported.0.contains(&e) {
            continue;
        }
        reported.0.insert(e);
        let name = q_names
            .get(e)
            .map(|n| n.as_str().to_owned())
            .unwrap_or_else(|_| format!("{e:?}"));
        let parent_name = q_names
            .get(parent)
            .map(|n| n.as_str().to_owned())
            .unwrap_or_else(|_| format!("{parent:?}"));
        bevy::log::warn!(
            "[cell-audit] `{name}` ({e:?}) carries CellCoord but its parent \
             `{parent_name}` ({parent:?}) is not a Grid — big_space will not \
             propagate it (doc 45 class 2)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;

    /// The canonical `WorldGrid` must actually BIN into cells.
    ///
    /// `switching_threshold` bounds `maximum_distance_from_origin = edge/2 +
    /// threshold`, below which `translation_to_grid` returns cell `(0,0,0)` and
    /// the entire position as a raw **f32**. At the historical `1e10` that
    /// covered the whole Earth–Moon system: everything sat in cell 0 and the
    /// f32 `Transform` alone carried 3.8e8 m, where one ULP is **32 m**.
    ///
    /// This asserts the two properties that make the grid a high-precision
    /// grid at all: a distant point gets a NON-ZERO cell, and its f32 remainder
    /// stays inside `max_distance` (so its ULP is sub-millimetre).
    #[test]
    fn world_grid_bins_cells_at_lunar_distance() {
        let cfg = WorldGridConfig::default();
        let grid = Grid::new(cfg.cell_edge_length, cfg.switching_threshold);
        let max_dist = (cfg.cell_edge_length / 2.0 + cfg.switching_threshold) as f64;

        // Earth–Moon distance: the case the review measured 32 m of ULP at.
        let p = DVec3::new(3.844e8, 0.0, 0.0);
        let (cell, offset) = grid.translation_to_grid(p);

        assert_ne!(
            cell,
            CellCoord::default(),
            "a point at 3.8e8 m must NOT stay in cell (0,0,0) — a raw f32 there \
             has 32 m of ULP (switching_threshold is a precision knob: {} m)",
            cfg.switching_threshold
        );
        assert!(
            (offset.abs().max_element() as f64) <= max_dist + 1e-3,
            "the f32 remainder {offset:?} must stay within max_distance {max_dist} m"
        );

        // The decomposition is still exact: cells (i64) carry the magnitude.
        let back = grid.grid_position_double(&cell, &Transform::from_translation(offset));
        assert!(
            (back - p).length() < 1e-2,
            "cell+offset must reassemble to the input, off by {} m",
            (back - p).length()
        );
    }
}
