//! The **world shell** — the single, persistent big_space coordinate root that
//! every scene mounts into.
//!
//! ## Why this exists
//!
//! The live 3D world is a `BigSpace` root + a canonical `Grid` (the `WorldGrid`)
//! + exactly one persistent origin-tracking `Grid`. Per
//! `docs/architecture/21-domain-usd.md` the
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
//! The shell is **render-free and headless-complete**. The single
//! `FloatingOrigin` lives on a persistent [`OriginAnchor`] grid, never on a
//! camera or avatar. A windowed client updates the anchor's `(CellCoord,
//! Transform)` split from the selected camera's authoritative f64 pose; a
//! headless server leaves it at the world origin. There is always exactly one
//! valid origin owner, and camera projection never changes BigSpace hierarchy
//! archetypes.

use crate::{DiagnosticSeverity, RuntimeDiagnostic, RuntimeDiagnostics};
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

/// The single Avian coordinate partition currently mounted in the world.
///
/// Rendering may contain many nested BigSpace grids, but one Avian world must
/// not put bodies from different local frames at the same numeric origin. The
/// application/scene-mount owner binds this resource explicitly when it selects
/// a physics frame; the persistent world shell only creates topology and never
/// installs a frame as a convenience default.
#[derive(Resource, Debug, Clone, Copy)]
pub struct ActivePhysicsFrame(pub Entity);

/// The set [`setup_world`] runs in. Subsystems that need the shell to exist
/// (e.g. celestial's hierarchy) order `.after(WorldShellSet)`. Ordering is a
/// convenience — `ensure_world_root` is create-or-get, so it is never required
/// for correctness, only to avoid a redundant shell spawn.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldShellSet;

/// The persistent high-precision holder of the single `FloatingOrigin`.
///
/// This is itself a grid-direct origin-tracking frame. The viewport reconciler
/// moves its `(CellCoord, Transform)` split to the selected camera's `WorldGrid`
/// pose; the camera never
/// receives or removes `FloatingOrigin`. Keeping the marker on this valid
/// `Grid` avoids mixing render presentation ownership with BigSpace hierarchy
/// ownership and also works without a camera.
#[derive(Component, Debug, Default, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct OriginAnchor;

/// Startup configuration for the canonical [`WorldGrid`]. A *resource* so the
/// binary or scene host can choose the frame before the world shell is created;
/// core has one authoritative contract rather than copied `Grid::new` values.
/// Existing grids are not migrated when this resource changes, so it is a
/// topology setting and must be set before [`WorldShellPlugin`] startup.
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

impl WorldGridConfig {
    /// Validate the topology values before BigSpace sees them.
    pub fn validate(self) -> Result<(), String> {
        if !self.cell_edge_length.is_finite() || self.cell_edge_length <= 0.0 {
            return Err(format!(
                "cell_edge_length must be finite and positive, got {}",
                self.cell_edge_length
            ));
        }
        if !self.switching_threshold.is_finite() || self.switching_threshold < 0.0 {
            return Err(format!(
                "switching_threshold must be finite and non-negative, got {}",
                self.switching_threshold
            ));
        }
        Ok(())
    }

    /// Construct the canonical BigSpace grid from this startup setting.
    pub fn grid(self) -> Grid {
        Grid::new(self.cell_edge_length, self.switching_threshold)
    }
}

/// Idempotent **create-or-get** for the world shell. Returns the [`WorldGrid`]
/// entity scenes mount under.
///
/// First call spawns `BigSpace` root → `WorldGrid` → [`OriginAnchor`] (a
/// grid-direct entity carrying the single `FloatingOrigin`). Subsequent calls return the existing
/// `WorldGrid`. Safe to call from `Startup`, from `LoadScene`, from celestial
/// setup — order-independent, which is the whole point.
pub fn ensure_world_root(world: &mut World) -> Entity {
    let existing: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<WorldGrid>>();
        q.iter(world).collect()
    };
    match existing.as_slice() {
        [grid] => return *grid,
        [] => {}
        _ => panic!(
            "WorldShell contract violated: expected exactly one WorldGrid, found {}",
            existing.len()
        ),
    }

    let cfg = world
        .get_resource::<WorldGridConfig>()
        .copied()
        .unwrap_or_default();
    cfg.validate()
        .unwrap_or_else(|error| panic!("invalid WorldGridConfig: {error}"));

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
    // The root and WorldGrid use the same deliberately SMALL threshold. It
    // bounds the f32 remainder of the origin's pose in each grid
    // (`edge/2 + threshold`), i.e. it is a PRECISION knob — see
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
            cfg.grid(),
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
            cfg.grid(),
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

    // The persistent origin-tracking grid. Its `(CellCoord, Transform)` split
    // is updated from the active viewport camera by lunco-usd-bevy; on a
    // headless server it stays at the world origin. It is a Grid itself because
    // BigSpace validation requires a FloatingOrigin holder to remain a valid
    // grid-frame archetype.
    world.spawn((
        OriginAnchor,
        cfg.grid(),
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
            // Validate the `OriginAnchor`'s documented role — the sole holder of
            // the origin — every frame, not just at startup. Runs in `PostUpdate`
            // immediately before big_space's own origin finder
            // (`RecenterLargeTransforms`) so an invalid ownership/archetype state
            // is reported at its owner before BigSpace propagates the hierarchy.
            .add_systems(
                PostUpdate,
                validate_origin_anchor_contract.before(BigSpaceSystems::RecenterLargeTransforms),
            );

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

        // The canonical shell deliberately has no `Transform` on `WorldRoot`.
        // Consequently Bevy's f32 compatibility propagation cannot enter the
        // BigSpace tree, while big_space owns every origin-relative
        // `GlobalTransform`. Do not order the two propagation mechanisms as a
        // corrective pass: the component contract itself prevents a dual writer.
    }
}

/// Startup guarantee — create the shell up front (race-free). Correctness does
/// not depend on this running before other consumers: they all call
/// [`ensure_world_root`], which is create-or-get.
fn setup_world(world: &mut World) {
    ensure_world_root(world);
}

/// Validate the invariant the [`OriginAnchor`] doc promises: it is the sole
/// holder of the single `FloatingOrigin` and is a valid grid-direct frame.
///
/// This deliberately reports invalid state instead of repairing it. Camera,
/// avatar, and scene systems do not participate in origin ownership; any extra
/// origin marker or malformed anchor must remain visible to both this owner
/// diagnostic and BigSpace's own hierarchy validator.
fn validate_origin_anchor_contract(
    q_origins: Query<Entity, With<FloatingOrigin>>,
    q_anchors: Query<Entity, With<OriginAnchor>>,
    q_valid_anchors: Query<
        Entity,
        (
            With<OriginAnchor>,
            With<Grid>,
            With<CellCoord>,
            With<Transform>,
            With<GlobalTransform>,
            With<ChildOf>,
        ),
    >,
    q_parents: Query<&ChildOf>,
    q_world_roots: Query<Entity, With<WorldRoot>>,
    q_world_grids: Query<Entity, With<WorldGrid>>,
    q_valid_world_roots: Query<
        Entity,
        (
            With<WorldRoot>,
            With<BigSpace>,
            With<Grid>,
            With<GlobalTransform>,
            Without<Transform>,
            Without<CellCoord>,
        ),
    >,
    q_valid_world_grids: Query<
        Entity,
        (
            With<WorldGrid>,
            With<Grid>,
            With<CellCoord>,
            With<Transform>,
            With<GlobalTransform>,
            With<ChildOf>,
        ),
    >,
    config: Res<WorldGridConfig>,
    diagnostics: Option<ResMut<RuntimeDiagnostics>>,
) {
    let anchors: Vec<Entity> = q_anchors.iter().collect();
    let origins: Vec<Entity> = q_origins.iter().collect();
    let world_roots: Vec<Entity> = q_world_roots.iter().collect();
    let world_grids: Vec<Entity> = q_world_grids.iter().collect();
    let mut errors: Vec<(&'static str, String)> = Vec::new();

    if let Err(error) = config.validate() {
        errors.push((
            "world-config",
            format!("[world-config] invalid canonical grid settings: {error}"),
        ));
    }

    if world_roots.len() != 1 {
        errors.push((
            "world-shell",
            format!(
                "[world-shell] expected exactly one WorldRoot, found {}",
                world_roots.len()
            ),
        ));
    }
    if world_grids.len() != 1 {
        errors.push((
            "world-shell",
            format!(
                "[world-shell] expected exactly one WorldGrid, found {}",
                world_grids.len()
            ),
        ));
    }
    if let Some(&root) = world_roots.first() {
        if q_valid_world_roots.get(root).is_err() {
            errors.push(("world-shell", format!(
                "[world-shell] WorldRoot {root:?} must carry BigSpace and Grid with GlobalTransform, without Transform or CellCoord"
            )));
        }
    }
    if let Some(&grid) = world_grids.first() {
        if q_valid_world_grids.get(grid).is_err() {
            errors.push(("world-shell", format!(
                "[world-shell] WorldGrid {grid:?} must be a Grid-direct frame with CellCoord, Transform, and GlobalTransform"
            )));
        } else if q_parents
            .get(grid)
            .ok()
            .is_none_or(|parent| q_valid_world_roots.get(parent.parent()).is_err())
        {
            errors.push((
                "world-shell",
                format!("[world-shell] WorldGrid {grid:?} must be a direct child of WorldRoot"),
            ));
        }
    }

    if anchors.len() != 1 {
        errors.push((
            "world-origin",
            format!(
                "[world-origin] expected exactly one OriginAnchor, found {}",
                anchors.len()
            ),
        ));
    }
    if origins.len() != 1 {
        errors.push((
            "world-origin",
            format!(
                "[world-origin] expected exactly one FloatingOrigin owner, found {}",
                origins.len()
            ),
        ));
    }

    if let Some(&anchor) = anchors.first() {
        if q_valid_anchors.get(anchor).is_err() {
            errors.push(("world-origin", format!(
                "[world-origin] OriginAnchor {anchor:?} must be a Grid-direct frame with Grid, CellCoord, Transform, GlobalTransform, and ChildOf"
            )));
        } else if q_parents
            .get(anchor)
            .ok()
            .is_none_or(|parent| q_valid_world_grids.get(parent.parent()).is_err())
        {
            errors.push((
                "world-origin",
                format!(
                    "[world-origin] OriginAnchor {anchor:?} must be a direct child of WorldGrid"
                ),
            ));
        }

        if origins.first().copied() != Some(anchor) {
            errors.push((
                "world-origin",
                format!(
                    "[world-origin] OriginAnchor {anchor:?} is not the sole FloatingOrigin owner"
                ),
            ));
        }
    }

    if let Some(mut diagnostics) = diagnostics {
        let findings = errors.iter().map(|(code, message)| RuntimeDiagnostic {
            code: (*code).to_string(),
            severity: DiagnosticSeverity::Error,
            producer: "world-shell".to_string(),
            subject: match *code {
                "world-shell" => "WorldShell",
                "world-config" => "WorldGridConfig",
                _ => "OriginAnchor",
            }
            .to_string(),
            message: message.clone(),
        });
        diagnostics.replace_producer("world-shell", findings);
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
    use crate::RuntimeDiagnostics;
    use bevy::math::DVec3;
    use big_space::plugin::BigSpaceMinimalPlugins;

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

    #[test]
    #[should_panic(expected = "expected exactly one WorldGrid")]
    fn ensure_world_root_rejects_duplicate_world_grids() {
        let mut world = World::new();
        world.spawn(WorldGrid);
        world.spawn(WorldGrid);

        ensure_world_root(&mut world);
    }

    #[test]
    fn invalid_extra_origin_is_reported_without_repairing_the_owner_contract() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(BigSpaceMinimalPlugins)
            .init_resource::<RuntimeDiagnostics>()
            .add_plugins(WorldShellPlugin);
        app.update();

        let stray = app.world_mut().spawn(FloatingOrigin).id();
        app.update();

        let diagnostics = app.world().resource::<RuntimeDiagnostics>();
        assert!(diagnostics.findings.iter().any(|finding| {
            finding.producer == "world-shell"
                && finding.code == "world-origin"
                && finding.message.contains("exactly one FloatingOrigin owner")
        }));
        assert!(
            app.world().get::<FloatingOrigin>(stray).is_some(),
            "the world-shell owner must report an invalid extra origin, not silently remove it"
        );
    }

    #[test]
    fn misparented_origin_anchor_is_reported_even_when_its_parent_is_a_grid() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(BigSpaceMinimalPlugins)
            .init_resource::<RuntimeDiagnostics>()
            .add_plugins(WorldShellPlugin);
        app.update();
        let grid_config = *app.world().resource::<WorldGridConfig>();

        let world_grid = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<WorldGrid>>();
            query.single(world).expect("canonical WorldGrid")
        };
        let nested_grid = app
            .world_mut()
            .spawn((
                grid_config.grid(),
                CellCoord::default(),
                Transform::default(),
                GlobalTransform::default(),
                ChildOf(world_grid),
            ))
            .id();
        let anchor = {
            let world = app.world_mut();
            let mut query = world.query_filtered::<Entity, With<OriginAnchor>>();
            query.single(world).expect("canonical OriginAnchor")
        };
        app.world_mut()
            .entity_mut(anchor)
            .insert(ChildOf(nested_grid));

        app.update();

        let diagnostics = app.world().resource::<RuntimeDiagnostics>();
        assert!(diagnostics.findings.iter().any(|finding| {
            finding.producer == "world-shell"
                && finding.code == "world-origin"
                && finding.subject == "OriginAnchor"
                && finding.message.contains("direct child of WorldGrid")
        }));
        assert!(
            app.world().get::<ChildOf>(anchor).is_some(),
            "invalid topology must remain visible to the owner instead of being repaired"
        );
    }

    #[test]
    fn changed_invalid_grid_settings_are_reported_by_the_world_owner() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(BigSpaceMinimalPlugins)
            .init_resource::<RuntimeDiagnostics>()
            .add_plugins(WorldShellPlugin);
        app.update();

        app.world_mut()
            .resource_mut::<WorldGridConfig>()
            .cell_edge_length = 0.0;
        app.update();

        let diagnostics = app.world().resource::<RuntimeDiagnostics>();
        assert!(diagnostics.findings.iter().any(|finding| {
            finding.code == "world-config"
                && finding.subject == "WorldGridConfig"
                && finding.message.contains("finite and positive")
        }));
    }
}
