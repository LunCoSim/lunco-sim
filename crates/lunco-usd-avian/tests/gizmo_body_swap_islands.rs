//! REPRODUCED: `JointCollisionDisabled` on a joint whose two bodies are ALREADY
//! TOUCHING corrupts Avian's island contact list. This is an upstream avian bug,
//! and `lunco-usd-avian` hits it on its normal path.
//!
//! ## The bug
//!
//! `on_disable_joint_collision` (`joint_graph/plugin.rs:290-295`) deletes the
//! bodies' contact edges straight out of the `ContactGraph` with
//! `remove_edge_by_id` and NEVER calls `IslandManager::remove_contact`. Avian's
//! own correct pattern is three lines away in `narrow_phase/mod.rs:447-459` —
//! `let has_island = contact_edge.island.is_some();` … `islands.remove_contact(…)`
//! BEFORE the delete. The joint path just skips it.
//!
//! So the island's intrusive contact linked list keeps a node whose `ContactId`
//! has been freed — and `ContactId`s are RECYCLED (`stable_graph.rs:307`). The
//! next island op that walks that list dies on a freed slot.
//!
//! ## What is PROVEN, and what is NOT
//!
//! PROVEN here: the corruption above is real, it panics
//! (`islands/mod.rs:608:18`, `remove_contact` unwrapping `contact_island.prev`),
//! it fires on our exact bundle-insert path, and it needs no `validate` feature.
//! That alone is a bug we must fix.
//!
//! NOT PROVEN: that this is what caused the LIVE crash. That one was
//! `islands/mod.rs:547` — `add_contact` unwrapping `island.head_contact` — on a
//! gizmo drag. Every attempt here lands on `:608` instead, INCLUDING
//! `corrupted_island_then_a_new_contact_is_added`, which corrupts the island and
//! then deliberately drives a fresh `add_contact` into it. So `:547` remains
//! un-reproduced. The two lines are the same class of dangling-list-node fault
//! and the crashed scene does author revolute joints with `JointCollisionDisabled`
//! — which makes this a strong SUSPECT for the live crash, not a closed case.
//! Do not write it up as solved until `:547` itself is reproduced.
//!
//! ## It is TIMING, not the bundle — and that is the whole fix
//!
//! The bundle is CORRECT and is not the bug. Bevy writes a whole bundle before
//! firing observers, so `add_joint_to_graph` reads `Has<JointCollisionDisabled>
//! == true` and the `JointGraphEdge` is BORN `collision_disabled`; the broad
//! phase then never creates the pair at all (`bvh_broad_phase.rs:275-283`) and no
//! contact edge ever exists to be mis-deleted. That is
//! `joint_bundle_attached_before_contact_never_forms_a_pair`, green.
//!
//! Born-disabled prevents a pair from FORMING; it does not clean up one that
//! already exists. So the rule is: **the joint must be attached before the first
//! narrow phase that could put its bodies in contact.** Attach the identical
//! bundle onto already-touching bodies and it walks into
//! `on_disable_joint_collision` and corrupts the island —
//! `joint_and_collision_disabled_inserted_as_one_bundle`, `#[should_panic]`.
//!
//! This was OUR bug on the authored-joint path. `build_usd_physics_joints` gates
//! on `With<Position>` (needed — attaching before admission panics `merge_islands`
//! "Neither body … is in an island") but used to run in `Update`, where that gate
//! can only open AFTER avian has already stepped. The joint therefore always
//! arrived a tick late, into live contacts. It now runs in
//! `PhysicsSystems::Prepare` — after admission has flushed, before
//! `StepSimulation`'s broad/narrow phase — which is the one window where both
//! conditions hold. `lunco-usd-sim`'s synthesized wheel joint had hit this exact
//! race earlier and was fixed by attaching synchronously; the authored path kept
//! the bug until now.
//!
//! The policy itself lives in ONE place, `lunco_usd_avian::joint_bundle` — the
//! only sanctioned way to attach a joint, so the marker cannot drift away from
//! its joint per call site or per joint type.
//!
//! The `#[should_panic]` tests below fail WITHOUT the `avian-validate` feature —
//! they are real crashes in an ordinary build, not diagnostic artifacts. They are
//! `should_panic` because the fault is upstream and we are not forking avian: if a
//! future avian bump fixes `on_disable_joint_collision`, they go green-when-
//! expected-to-panic and tell us the constraint has lifted.
//!
//! ## Read this before trusting a green in this file
//!
//! `joint_collision_disabled_on_already_touching_bodies` is GREEN and PROVES
//! NOTHING: its two boxes sit side by side abutting at exactly zero penetration
//! and never form a touching contact. Move them into a gravity-pressed stack
//! (`stacked_app`, guarded by `stacked_bodies_are_actually_touching`) and the same
//! sequence panics. It is kept as a monument to that false negative — an
//! "already touching" test that never touched. Any physics test here must PROVE
//! its precondition holds; `harness_actually_steps_physics` guards the other
//! trap (a bare `app.update()` loop steps NO physics at all).
//!
//! ## Still-green shapes, now meaningful as regression guards
//!
//! The gizmo's Dynamic→Kinematic swap and the scene-teardown path stay green,
//! including under `validate` (which asserts AT the corrupting mutation rather
//! than at the delayed unwrap). So the gizmo drag did not CAUSE the live crash —
//! it was merely the next `add_contact` to trip over corruption already seeded by
//! a joint. lunco's gear joint is likewise ruled out: it is a PD force law, not
//! an avian joint, registers nothing with the `JointGraph`, and adds no
//! `JointCollisionDisabled`.
//!
//! Diagnostic (asserts at the corrupting mutation, graph-wide, slow):
//!   cargo test -j2 -p lunco-usd-avian -p lunco-physics \
//!       --features lunco-physics/avian-validate --test gizmo_body_swap_islands

use avian3d::prelude::*;
use bevy::math::DVec3;
use bevy::time::TimeUpdateStrategy;
use std::time::Duration;
use bevy::prelude::*;

mod support;

/// The scene shape that crashed: a static ground, a rig body, and two dynamic
/// "rockers" revolute-jointed to the rig and resting ON the ground, so every
/// body is island-linked through live touching contacts AND joints.
struct Rig {
    rocker_l: Entity,
    #[allow(dead_code)]
    rocker_r: Entity,
}

fn spawn_rig(world: &mut World) -> Rig {
    world.spawn((
        RigidBody::Static,
        Collider::cuboid(50.0, 1.0, 50.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));

    let rig = world
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.5, 0.5, 0.5),
            Transform::from_xyz(0.0, 1.5, 0.0),
        ))
        .id();

    let rocker_l = world
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.5, 0.25, 0.25),
            Transform::from_xyz(-1.2, 0.25, 0.0),
        ))
        .id();
    let rocker_r = world
        .spawn((
            RigidBody::Dynamic,
            Collider::cuboid(0.5, 0.25, 0.25),
            Transform::from_xyz(1.2, 0.25, 0.0),
        ))
        .id();

    world.spawn(RevoluteJoint::new(rig, rocker_l).with_aligned_axis(DVec3::Z));
    world.spawn(RevoluteJoint::new(rig, rocker_r).with_aligned_axis(DVec3::Z));

    Rig { rocker_l, rocker_r }
}

fn settled_app() -> (App, Rig) {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    // WITHOUT this, the test is a no-op that passes for the wrong reason.
    // `Time<Fixed>` accrues from the real clock, and 120 back-to-back `update()`
    // calls burn ~0 wall time, so `FixedMain` — and therefore the whole physics
    // schedule — never runs at all. `harness_actually_steps_physics` below locks
    // that in.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    let rig = spawn_rig(app.world_mut());
    app.finish();
    app.cleanup();
    // Let the rockers land and their contacts get island-linked.
    for _ in 0..120 {
        app.update();
    }
    (app, rig)
}

/// Baseline: the rig itself is fine. If this fails, the repro below proves
/// nothing about the body swap.
#[test]
fn jointed_rockers_resting_on_ground_step_cleanly() {
    let (mut app, _rig) = settled_app();
    for _ in 0..120 {
        app.update();
    }
}

/// THE REPRO. `capture_gizmo_start` does exactly this: `try_insert(RigidBody::
/// Kinematic)` on the dragged body, which fires Avian's `remove_body_on::<Insert,
/// RigidBody>` observer and drains that body's contact edges — leaving the island's
/// `head_contact` dangling if any edge skipped the unlink.
#[test]
fn dynamic_to_kinematic_swap_on_a_jointed_resting_body() {
    let (mut app, rig) = settled_app();

    app.world_mut()
        .entity_mut(rig.rocker_l)
        .insert(RigidBody::Kinematic);

    // The live crash landed on the first narrow-phase run after the swap.
    for _ in 0..120 {
        app.update();
    }
}

/// The full drag cycle: swap to Kinematic, drive it with a velocity for a while
/// (the gizmo writes `LinearVelocity` every frame), then hand the original body
/// kind back on release.
#[test]
fn full_gizmo_drag_cycle_on_a_jointed_resting_body() {
    let (mut app, rig) = settled_app();

    app.world_mut()
        .entity_mut(rig.rocker_l)
        .insert(RigidBody::Kinematic);
    for _ in 0..30 {
        app.world_mut()
            .entity_mut(rig.rocker_l)
            .insert(LinearVelocity(DVec3::new(0.0, 1.0, 0.0)));
        app.update();
    }

    app.world_mut()
        .entity_mut(rig.rocker_l)
        .insert(LinearVelocity(DVec3::ZERO));
    app.world_mut()
        .entity_mut(rig.rocker_l)
        .insert(RigidBody::Dynamic);
    for _ in 0..120 {
        app.update();
    }
}

/// Harness self-check: does `app.update()` in a loop actually STEP physics?
/// `Time<Fixed>` accrues from the real clock, and 120 back-to-back `update()`
/// calls take ~0 wall time — so if this fails, every test above is vacuous and
/// proves nothing.
#[test]
fn harness_actually_steps_physics() {
    let (app, _rig) = settled_app();
    let elapsed = app.world().resource::<Time<Fixed>>().elapsed();
    assert!(
        elapsed >= Duration::from_secs_f64(1.0),
        "physics never stepped: Time<Fixed> elapsed={elapsed:?} after 120 updates \
         — every test in this file would be vacuously green"
    );
}

/// SECOND CANDIDATE: `JointCollisionDisabled` added to a joint whose two bodies
/// are ALREADY touching. `on_disable_joint_collision`
/// (`joint_graph/plugin.rs:290-295`) drops those contact edges straight out of
/// the `ContactGraph` with `remove_edge_by_id` and never calls
/// `IslandManager::remove_contact` — so the island keeps pointing at a freed id.
///
/// The live scene builds its joints AFTER the bodies are spawned and settling
/// ("Resolved gear joint /DiffRigTest/Rig/Differential" lands well after the
/// spawn), which is exactly this order.
#[test]
fn joint_collision_disabled_on_already_touching_bodies() {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));

    let (a, b) = {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(50.0, 1.0, 50.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        // Two boxes side by side, touching each other AND the ground.
        let a = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(0.5, 0.5, 0.5),
                Transform::from_xyz(-0.25, 0.25, 0.0),
            ))
            .id();
        let b = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(0.5, 0.5, 0.5),
                Transform::from_xyz(0.25, 0.25, 0.0),
            ))
            .id();
        (a, b)
    };
    app.finish();
    app.cleanup();

    // Settle: let the a<->b contact become touching and island-linked.
    for _ in 0..120 {
        app.update();
    }

    // Now joint them with collision between them disabled.
    app.world_mut()
        .spawn((FixedJoint::new(a, b), JointCollisionDisabled));

    for _ in 0..120 {
        app.update();
    }
}

/// THIRD CANDIDATE, and the one the live log actually points at: SCENE TEARDOWN
/// seeds the corruption, and a later `add_contact` — in the crash, the gizmo's —
/// is merely the thing that trips over it.
///
/// The live sequence was three scene switches (moonbase twin -> school twin ->
/// differential rig), each a mass `try_despawn` of live physics bodies, and only
/// THEN a drag. The root `Cargo.toml` already records that avian's teardown
/// "still left islands holding contacts" through every despawn order tried.
/// This replays that: settle a rig, despawn it wholesale, build a fresh one on
/// the same world, and let its contacts start touching.
#[test]
fn despawn_live_bodies_then_build_a_fresh_rig() {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    let rig = spawn_rig(app.world_mut());
    app.finish();
    app.cleanup();
    for _ in 0..120 {
        app.update();
    }

    // "[scene] cleanup: N entities despawned" — bodies AND joints go at once.
    let doomed: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<RigidBody>>()
        .iter(app.world())
        .collect();
    for e in doomed {
        app.world_mut().entity_mut(e).despawn();
    }
    let _ = rig;
    app.update();

    // Fresh scene on the same world, contacts start touching again.
    spawn_rig(app.world_mut());
    for _ in 0..240 {
        app.update();
    }
}

/// Build a stack whose A<->B contact is UNAMBIGUOUSLY touching and island-linked:
/// B rests on A, gravity presses them together. (The earlier
/// `joint_collision_disabled_on_already_touching_bodies` put two boxes side by
/// side abutting at exactly zero penetration — they may never have formed a
/// touching contact at all, which would make its green meaningless.)
fn stacked_app() -> (App, Entity, Entity) {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    let (a, b) = {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(50.0, 1.0, 50.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let a = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ))
            .id();
        let b = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(0.0, 1.5, 0.0),
            ))
            .id();
        (a, b)
    };
    app.finish();
    app.cleanup();
    for _ in 0..180 {
        app.update();
    }
    (app, a, b)
}

/// Guard: the stack really is in contact before the interesting part runs.
#[test]
fn stacked_bodies_are_actually_touching() {
    let (mut app, a, b) = stacked_app();
    let graph = app.world().resource::<ContactGraph>();
    let touching = graph
        .contact_pairs_with(a)
        .any(|p| (p.body1 == Some(b) || p.body2 == Some(b)) && p.is_touching());
    assert!(touching, "A<->B never formed a touching contact — any test built on this proves nothing");
    let _ = &mut app;
}

/// THE REFINED CANDIDATE. `JointCollisionDisabled` added to a joint that is
/// ALREADY REGISTERED in the JointGraph, whose bodies are ALREADY touching.
///
/// This is the ordering `on_disable_joint_collision` actually needs to do damage:
/// it bails at `let Some([body1, body2]) = joint_graph.bodies_of(entity) else
/// { return; }` (`joint_graph/plugin.rs:258`), so the joint edge must exist
/// first. Then it deletes the touching contact edges with `remove_edge_by_id`
/// and never calls `IslandManager::remove_contact` (`:290-295`).
#[test]
#[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
fn joint_collision_disabled_added_after_joint_is_registered() {
    let (mut app, a, b) = stacked_app();

    // Frame 1: joint only — gets into the JointGraph.
    let joint = app.world_mut().spawn(FixedJoint::new(a, b)).id();
    app.update();

    // Frame 2: NOW disable collision on it, with the A<->B contact live.
    app.world_mut().entity_mut(joint).insert(JointCollisionDisabled);

    for _ in 0..240 {
        app.update();
    }
}

/// OUR ACTUAL CODE PATH. `lunco-usd-avian/src/lib.rs:1080-1144` inserts the joint
/// and `JointCollisionDisabled` as ONE bundle onto an already-spawned joint prim
/// whose bodies may already be touching. Bevy fires `Add` per component in bundle
/// order, so the joint registers in the JointGraph first and
/// `on_disable_joint_collision` then finds `bodies_of` = Some — i.e. the bundle
/// form is NOT protected by the early return.
#[test]
#[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
fn joint_and_collision_disabled_inserted_as_one_bundle() {
    let (mut app, a, b) = stacked_app();

    app.world_mut()
        .spawn((FixedJoint::new(a, b), JointCollisionDisabled));

    for _ in 0..240 {
        app.update();
    }
}

/// Closes the attribution gap: reproduce the LIVE line (`islands/mod.rs:547`,
/// `add_contact` unwrapping a freed `head_contact`) — not just the `:608`
/// `remove_contact` variant — from the same joint-seeded corruption.
///
/// Corrupt the island via `JointCollisionDisabled` on a touching pair, then drop
/// a fresh body onto the stack so a NEW contact is ADDED to that same island.
/// `add_contact` links the newcomer against `island.head_contact` — which is the
/// freed id.
#[test]
#[should_panic(expected = "called `Option::unwrap()` on a `None` value")]
fn corrupted_island_then_a_new_contact_is_added() {
    let (mut app, a, b) = stacked_app();

    // Seed the corruption exactly as our joint path does.
    app.world_mut()
        .spawn((FixedJoint::new(a, b), JointCollisionDisabled));
    app.update();

    // Now force an add_contact into that island: a third box falls onto the stack.
    app.world_mut().spawn((
        RigidBody::Dynamic,
        Collider::cuboid(1.0, 1.0, 1.0),
        Transform::from_xyz(0.0, 3.0, 0.0),
    ));

    for _ in 0..240 {
        app.update();
    }
}

/// THE COMPLIANT PATTERN, and the contract `lunco_usd_avian::joint_bundle` +
/// the `PhysicsSystems::Prepare` placement exist to guarantee.
///
/// Attach the joint (marker in the SAME bundle) BEFORE the two bodies have ever
/// touched. The `JointGraphEdge` is born `collision_disabled`, so the broad phase
/// never creates the pair (`bvh_broad_phase.rs:275-283`) — no contact edge is ever
/// born, so `on_disable_joint_collision` has nothing to delete and the island's
/// contact list cannot be corrupted. The bodies are then driven together by
/// gravity for 4 simulated seconds, which WOULD have produced a touching contact
/// without the joint.
///
/// Contrast `joint_and_collision_disabled_inserted_as_one_bundle`, which uses the
/// identical bundle but attaches it once the bodies are already touching, and
/// panics. Bundle alone is not enough — the ordering is half the contract.
#[test]
fn joint_bundle_attached_before_contact_never_forms_a_pair() {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    let (a, b) = {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(50.0, 1.0, 50.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        let a = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(0.0, 0.5, 0.0),
            ))
            .id();
        // Well clear of `a` — no contact can exist yet at attach time.
        let b = world
            .spawn((
                RigidBody::Dynamic,
                Collider::cuboid(1.0, 1.0, 1.0),
                Transform::from_xyz(0.0, 4.0, 0.0),
            ))
            .id();
        (a, b)
    };
    app.finish();
    app.cleanup();

    // Admit the bodies to the island graph (this is what the `With<Position>`
    // gate waits for) — while they are still far apart.
    app.update();

    // Attach through the production choke point.
    app.world_mut()
        .spawn(lunco_usd_avian::joint_bundle(FixedJoint::new(a, b)));

    // Gravity would slam `b` onto `a` here. Born-disabled means the pair is never
    // even created.
    for _ in 0..240 {
        app.update();
    }

    let graph = app.world().resource::<ContactGraph>();
    let pair = graph
        .contact_pairs_with(a)
        .any(|p| p.body1 == Some(b) || p.body2 == Some(b));
    assert!(
        !pair,
        "a contact pair formed between joint-collision-disabled bodies — the \
         born-disabled JointGraphEdge did not reach the broad phase"
    );
}

/// The `LoadScene` panic "Island IslandId(N) does not exist"
/// (`islands/mod.rs:1403`) — WHERE THE EVIDENCE ACTUALLY STANDS.
///
/// It is VALIDATE-ONLY: under `--features lunco-physics/avian-validate` every
/// scene change dies on it, while the same build WITHOUT the feature survives
/// repeated scene switches cleanly (verified 17-Jul-2026).
///
/// The live backtrace contains NO LUNCO FRAME — the panicking code is avian's own
/// `#[cfg(feature = "validate")]` block:
///   21: EntityWorldMut::despawn_no_free_with_caller   <- a despawn (ours; any despawn)
///   20: RawCommandQueue::apply_or_drop_queued         <- flush DURING it
///   14: islands::{impl#8}::on_remove::{closure_env#1} <- the cfg(validate) block
///    3: unwrap_or_else -> "Island IslandId(0) does not exist"
///
/// The mechanism that fits: `on_remove` (`:1338`) frees islands SYNCHRONOUSLY
/// (`remove_island`, `:1374-1376`) but queues its check DEFERRED
/// (`world.commands().queue(...)`, `:1390`). One body's on_remove leaves the
/// island populated → queues `validate(island_N)`; a later body empties and frees
/// island_N; the flush then runs the first check against a freed id.
///
/// ⚠ THIS TEST DOES NOT REPRODUCE IT. It tears a multi-body island down through
/// one recursive `try_despawn` + flush, in pure avian, and stays GREEN under the
/// feature — as did a flat variant (despawn every body directly, one queue). So
/// the interleaving above is consistent with the backtrace but NOT pinned;
/// something in the live teardown (island shape, joints, contact counts, despawn
/// order) is still unmodelled.
///
/// What is established is narrow, and worth stating exactly:
///   - the panicking frame is avian's, validate-gated, with no lunco code in the
///     unwind path;
///   - without `validate`, the same teardown is clean.
/// That is NOT proof our teardown is correct — only that this panic is not
/// evidence against it. Do not upgrade this to "pure instrumentation, our side is
/// fine" without a repro that actually fires.
///
/// Regardless: `IslandId` is an arena index (`next_push_index()`, `:463`) and
/// RECYCLED, so a deferred check can silently land on a DIFFERENT island — false
/// greens as well as false reds on despawn-heavy paths.
#[test]
fn pure_avian_recursive_despawn_of_one_island() {
    let mut app = support::headless_physics_app();
    app.add_plugins(TransformPlugin);
    app.insert_resource(Time::<Fixed>::from_hz(60.0));
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));

    let root = {
        let world = app.world_mut();
        world.spawn((
            RigidBody::Static,
            Collider::cuboid(50.0, 1.0, 50.0),
            Transform::from_xyz(0.0, -0.5, 0.0),
        ));
        // A scene root with a stack of bodies under it — one island, several
        // members, torn down recursively like `clear_scene_entities` does.
        let root = world.spawn(Transform::default()).id();
        for i in 0..4 {
            let body = world
                .spawn((
                    RigidBody::Dynamic,
                    Collider::cuboid(1.0, 1.0, 1.0),
                    Transform::from_xyz(0.0, 0.5 + i as f32 * 1.0, 0.0),
                ))
                .id();
            world.entity_mut(root).add_child(body);
        }
        root
    };
    app.finish();
    app.cleanup();

    // Settle so the stack is one island with live touching contacts.
    for _ in 0..180 {
        app.update();
    }

    // The scene clear: one recursive try_despawn, one flush.
    let world = app.world_mut();
    world.commands().entity(root).try_despawn();
    world.flush();

    app.update();
}
