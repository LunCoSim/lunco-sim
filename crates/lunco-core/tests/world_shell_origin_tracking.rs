//! Regression test for the "world jumps around the camera in orbital view"
//! class (2026-07-10).
//!
//! big_space's high-precision propagation writes a ROOT's `GlobalTransform`
//! only when `Grid` and `BigSpace` live on the SAME entity, and writes a
//! cell-entity only when its direct parent is a `Grid`. The old shell split
//! them (`WorldRoot` = BigSpace only, `WorldGrid` = Grid only), so BOTH the
//! root's and the WorldGrid's GlobalTransforms were left to the plain-f32
//! bevy-compat pass, which writes IDENTITY forever — accidentally correct
//! while the floating origin's cell stayed (0,0,0), and wrong by the full
//! camera distance once the origin travels (orbital view). Every
//! Transform-only entity composing off them then renders in the wrong
//! convention and stands still while the world moves: "planets jump around".
//!
//! This test moves the floating origin far away and asserts the WorldGrid's
//! rendered pose tracks it (origin-relative), as big_space's contract
//! requires.

use bevy::prelude::*;
use big_space::plugin::BigSpaceMinimalPlugins;
use big_space::prelude::{CellCoord, FloatingOrigin, Grid};
use lunco_core::{ensure_world_root, OriginAnchor, WorldGrid, WorldShellPlugin};

#[test]
fn world_grid_global_transform_tracks_traveling_origin() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(BigSpaceMinimalPlugins)
        .add_plugins(WorldShellPlugin);

    // Frame 0: shell exists, origin on the anchor at cell 0.
    app.update();

    let world_grid = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<WorldGrid>>();
        q.single(app.world()).unwrap()
    };

    // A "camera" far from the scene, in real cells of the WorldGrid — the
    // orbital-view shape (WorldGrid edge is 2 km; threshold 1e10 means
    // translation_to_grid would keep small offsets in f32, so write the cell
    // directly the way `orbit_system` does after `translation_to_grid` of an
    // astronomical position).
    const CELLS: i64 = 100_000; // 2e8 m — Earth-ish range
    let camera = app
        .world_mut()
        .spawn((
            Transform::default(),
            GlobalTransform::default(),
            CellCoord::new(CELLS, 0, 0),
            ChildOf(world_grid),
        ))
        .id();

    // Claim the origin from the anchor, exactly like a camera does.
    let anchor = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, With<OriginAnchor>>();
        q.single(app.world()).unwrap()
    };
    app.world_mut().entity_mut(anchor).remove::<FloatingOrigin>();
    app.world_mut().entity_mut(camera).insert(FloatingOrigin);

    app.update();
    app.update(); // second frame: past any command-flush / tagging latency

    let edge = {
        let mut q = app.world_mut().query::<&Grid>();
        q.get(app.world(), world_grid).unwrap().cell_edge_length() as f64
    };
    let expected = -(CELLS as f64) * edge; // origin-relative: grid origin is behind the camera

    let gt = {
        let mut q = app.world_mut().query::<&GlobalTransform>();
        *q.get(app.world(), world_grid).unwrap()
    };
    let x = gt.translation().x as f64;

    println!("WorldGrid GT.x = {x:.3e}, expected ≈ {expected:.3e}");
    assert!(
        (x - expected).abs() < 1.0,
        "WorldGrid's GlobalTransform must track the floating origin \
         (expected x ≈ {expected:.3e}, got {x:.3e}). If it is ~0, the root \
         grid's pose is being written by the f32 compat pass instead of \
         big_space — the 'world jumps around the camera' regression."
    );

    // The victim class: a Transform-only entity mounted directly on the
    // WorldGrid must ALSO render origin-relative.
    let prop = app
        .world_mut()
        .spawn((
            Transform::from_xyz(5.0, 0.0, 0.0),
            GlobalTransform::default(),
            ChildOf(world_grid),
        ))
        .id();
    app.update();
    app.update();
    let gt = {
        let mut q = app.world_mut().query::<&GlobalTransform>();
        *q.get(app.world(), prop).unwrap()
    };
    let x = gt.translation().x as f64;
    println!("low-precision child GT.x = {x:.3e}, expected ≈ {:.3e}", expected + 5.0);
    // Tolerance is ULP-scale: GlobalTransform is f32, and at 2e8 magnitude one
    // ULP is 16 m — a 5 m local offset is below representable resolution. The
    // assertion guards the CONVENTION (origin-relative vs the compat pass's
    // absolute ~0), not sub-ULP arithmetic.
    assert!(
        (x - (expected + 5.0)).abs() < 64.0,
        "Transform-only children of the WorldGrid must inherit the \
         origin-relative pose (expected x ≈ {:.3e}, got {x:.3e})",
        expected + 5.0
    );
}

#[test]
fn ensure_world_root_root_owns_grid_and_bigspace() {
    // The structural contract itself: Grid and BigSpace on the same root.
    let mut world = World::new();
    let grid = ensure_world_root(&mut world);
    let root = world.get::<ChildOf>(grid).unwrap().parent();
    assert!(
        world.get::<Grid>(root).is_some(),
        "WorldRoot must carry a Grid: big_space's high-precision pass only \
         writes root/child-grid GlobalTransforms when the root is a real grid"
    );
    assert!(
        world.get::<big_space::prelude::BigSpace>(root).is_some(),
        "WorldRoot must carry BigSpace"
    );
    assert!(
        world.get::<Transform>(root).is_none(),
        "WorldRoot must NOT carry a Transform: big_space's canonical root \
         shape, and a root Transform re-arms the plain-f32 whole-tree \
         propagation race. Physics apps must register \
         BigSpacePhysicsBridgePlugin, whose rootless ColliderTransform \
         propagation replaces the avian pass that needed this Transform \
         (without the bridge, scale-carrying colliders collapse to unit \
         size — measured 2026-07-11)."
    );
}
