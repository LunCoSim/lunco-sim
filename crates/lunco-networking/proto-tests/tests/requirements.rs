//! Networking requirements test suite — "is everything we need there?"
//!
//! These cover the BACKEND-AGNOSTIC core (run now, no bevy/lightyear):
//!   • M1 deterministic identity from provenance
//!   • Mechanism selection (M2–M7) routing matches the documented case matrix
//!   • Axis-contradiction rejection (enforced-by-design invariants)
//!   • Gap A: big_space cell+offset rebasing math
//!
//! Backend-dependent requirements (M2 replication arrives, M4 input mutates
//! server, prediction/interpolation, host-client) become headless crossbeam
//! integration tests once the backend is committed (lightyear; original
//! NETWORKING_TEST_PLAN.md is in git history).

use lunco_net_proto_tests::identity::{content, derive_id, canonicalize_path, Provenance};
use lunco_net_proto_tests::rebase::{GridPos, CELL_SIZE};
use lunco_net_proto_tests::sync_class::*;

// ───────────────────────── M1 — deterministic identity ─────────────────────────

#[test]
fn m1_identity_is_deterministic_across_processes() {
    // Two independent "processes" loading the same USD prim must agree, no coordination.
    let proc_a = derive_id(&content("usd", "scene.usda", "/World/Rover1"));
    let proc_b = derive_id(&content("usd", "scene.usda", "/World/Rover1"));
    assert_eq!(proc_a, proc_b);
    assert!(proc_a.is_some());
}

#[test]
fn m1_identity_fits_53_bits() {
    let id = derive_id(&content("usd", "scene.usda", "/World/Rover1")).unwrap();
    assert!(id <= (1u64 << 53) - 1, "id must stay in JS-safe 53-bit space");
}

#[test]
fn m1_namespace_isolates_identity() {
    // Same source+path under different content namespaces ⇒ different ids.
    let usd = derive_id(&content("usd", "s", "/A"));
    let gltf = derive_id(&content("gltf", "s", "/A"));
    assert_ne!(usd, gltf, "namespaces must not collide (extensibility seam)");
}

#[test]
fn m1_distinct_paths_distinct_ids() {
    let a = derive_id(&content("usd", "scene.usda", "/World/Rover1"));
    let b = derive_id(&content("usd", "scene.usda", "/World/Rover2"));
    assert_ne!(a, b);
}

#[test]
fn m1_path_canonicalization_is_stable() {
    // Trailing slash, backslashes, and doubled slashes must not change identity.
    assert_eq!(canonicalize_path("/World/Rover1/"), "/World/Rover1");
    assert_eq!(canonicalize_path("\\World\\Rover1"), "/World/Rover1");
    assert_eq!(canonicalize_path("/World//Rover1"), "/World/Rover1");

    let canonical = derive_id(&content("usd", "s", "/World/Rover1"));
    for variant in ["/World/Rover1/", "\\World\\Rover1", "/World//Rover1"] {
        assert_eq!(
            derive_id(&content("usd", "s", variant)),
            canonical,
            "path variant {variant:?} must derive the same id"
        );
    }
}

#[test]
fn m1_authoritative_and_local_have_no_derived_id() {
    // Runtime-born → server-allocated (not derived here); Local → never networked.
    assert_eq!(derive_id(&Provenance::Authoritative), None);
    assert_eq!(derive_id(&Provenance::Local), None);
}

#[test]
fn m1_derived_depends_on_parent_and_role() {
    let parent = derive_id(&content("usd", "s", "/World/Rover1")).unwrap();
    let other_parent = derive_id(&content("usd", "s", "/World/Rover2")).unwrap();

    let wheel_fl = derive_id(&Provenance::Derived { parent, role: "wheel.FL".into() });
    let wheel_fr = derive_id(&Provenance::Derived { parent, role: "wheel.FR".into() });
    let wheel_fl_other = derive_id(&Provenance::Derived { parent: other_parent, role: "wheel.FL".into() });

    assert!(wheel_fl.is_some());
    assert_ne!(wheel_fl, wheel_fr, "different roles ⇒ different ids");
    assert_ne!(wheel_fl, wheel_fl_other, "same role under different parent ⇒ different ids");
    // determinism
    assert_eq!(wheel_fl, derive_id(&Provenance::Derived { parent, role: "wheel.FL".into() }));
}

#[test]
fn m1_no_collisions_in_realistic_sample() {
    // Not a proof, but catches gross derivation bugs: a few thousand prims unique.
    use std::collections::HashSet;
    let mut ids = HashSet::new();
    for i in 0..5000 {
        let id = derive_id(&content("usd", "scene.usda", &format!("/World/Prim_{i}"))).unwrap();
        assert!(ids.insert(id), "collision at prim {i} — revisit 53-bit policy");
    }
}

// ──────────────────── Mechanism selection (the case matrix) ────────────────────

fn cls(
    temporal: Temporal,
    authority: Authority,
    computability: Computability,
    from_content: bool,
    local_only: bool,
) -> Classification {
    Classification { temporal, authority, computability, from_content, local_only, pure_function_of_synced: false }
}

#[test]
fn select_driven_rover_pose_is_predicted() {
    // Continuous + locally avian-computable ⇒ M2-Predicted.
    let r = classify(&cls(Temporal::Continuous, Authority::Server, Computability::Predictable, true, false)).unwrap();
    assert_eq!(r, (Mechanism::M2State, Role::Predicted));
}

#[test]
fn select_cosim_driven_body_is_interpolated() {
    // Continuous + Opaque (server-only Modelica forces) ⇒ M2-Interpolated, even if owned.
    let r = classify(&cls(Temporal::Continuous, Authority::Server, Computability::Opaque, true, false)).unwrap();
    assert_eq!(r, (Mechanism::M2State, Role::Interpolated));
}

#[test]
fn select_possess_is_command() {
    let r = classify(&cls(Temporal::Discrete, Authority::Server, Computability::Opaque, false, false)).unwrap();
    assert_eq!(r.0, Mechanism::M3Command);
}

#[test]
fn select_intent_is_input() {
    let r = classify(&cls(Temporal::HighRateInput, Authority::ClientOwned, Computability::Predictable, false, false)).unwrap();
    assert_eq!(r.0, Mechanism::M4Input);
}

#[test]
fn select_modelica_text_is_crdt() {
    let r = classify(&cls(Temporal::ConcurrentText, Authority::Shared, Computability::Opaque, true, false)).unwrap();
    assert_eq!(r.0, Mechanism::M5Crdt);
}

#[test]
fn select_camera_is_local() {
    let r = classify(&cls(Temporal::Continuous, Authority::LocalOnly, Computability::Reconstructible, false, true)).unwrap();
    assert_eq!(r, (Mechanism::M7Local, Role::NotApplicable));
}

#[test]
fn select_content_static_is_content() {
    // Cosim wiring / structure from USD ⇒ M1 (reconstructed, not transmitted).
    let r = classify(&cls(Temporal::Static, Authority::Server, Computability::Reconstructible, true, false)).unwrap();
    assert_eq!(r.0, Mechanism::M1Content);
}

#[test]
fn select_runtime_static_is_command() {
    // Runtime-born but static (not content) ⇒ one reliable command, not M1.
    let r = classify(&cls(Temporal::Static, Authority::Server, Computability::Opaque, false, false)).unwrap();
    assert_eq!(r.0, Mechanism::M3Command);
}

#[test]
fn select_derived_value_recomputed_is_local() {
    // Step 0.5: pure function of already-synced state (e.g. lighting=f(clock,ephemeris)).
    let mut c = cls(Temporal::Continuous, Authority::Server, Computability::Reconstructible, true, false);
    c.pure_function_of_synced = true;
    let r = classify(&c).unwrap();
    assert_eq!(r.0, Mechanism::M7Local, "derived-from-synced must be recomputed, not replicated");
}

// ───────────────── Enforced-by-design: contradictions rejected ─────────────────

#[test]
fn contradiction_local_provenance_must_be_localonly() {
    let bad = cls(Temporal::Continuous, Authority::Server, Computability::Reconstructible, false, /*local_only*/ true);
    assert!(matches!(classify(&bad), Err(SyncError::Contradiction(_))));
}

#[test]
fn contradiction_cannot_predict_opaque() {
    assert!(validate_prediction(Computability::Predictable, Role::Predicted).is_ok());
    assert!(matches!(
        validate_prediction(Computability::Opaque, Role::Predicted),
        Err(SyncError::Contradiction(_))
    ));
    // Interpolating an opaque entity is fine.
    assert!(validate_prediction(Computability::Opaque, Role::Interpolated).is_ok());
}

// ───────────────────── Gap A — big_space rebasing math ─────────────────────

#[test]
fn rebase_preserves_absolute_world_position() {
    let p = GridPos::new([3, -2, 5], [1234.5, 6789.0, 42.0]);
    // Rebasing into origin O then adding O*CELL must reconstruct the world position.
    let origin = [1, 1, 1];
    let local = p.rebase_to(origin);
    let mut reconstructed = [0.0; 3];
    for i in 0..3 {
        reconstructed[i] = local[i] + origin[i] as f64 * CELL_SIZE;
    }
    let w = p.world();
    for i in 0..3 {
        assert!((reconstructed[i] - w[i]).abs() < 1e-6, "axis {i}: {reconstructed:?} vs {w:?}");
    }
}

#[test]
fn rebase_two_clients_agree_on_world() {
    // Two clients with different floating origins must see the same absolute world.
    let p = GridPos::new([10, 0, -7], [500.0, 0.0, 9999.0]);
    let client_a = [10, 0, -7]; // origin near the entity
    let client_b = [0, 0, 0]; // origin far away
    let la = p.rebase_to(client_a);
    let lb = p.rebase_to(client_b);

    let mut wa = [0.0; 3];
    let mut wb = [0.0; 3];
    for i in 0..3 {
        wa[i] = la[i] + client_a[i] as f64 * CELL_SIZE;
        wb[i] = lb[i] + client_b[i] as f64 * CELL_SIZE;
    }
    for i in 0..3 {
        assert!((wa[i] - wb[i]).abs() < 1e-6, "clients disagree on axis {i}");
    }
    // Client A (origin at the entity's cell) sees small, metric-scale coords.
    assert!(la.iter().all(|&v| v.abs() < CELL_SIZE), "near-origin client should have small coords");
}

#[test]
fn world_roundtrip_is_stable() {
    let world = [123_456.75, -987_654.25, 5_000.5];
    let g = GridPos::from_world(world);
    let back = g.world();
    for i in 0..3 {
        assert!((back[i] - world[i]).abs() < 1e-6, "roundtrip axis {i}");
    }
}

#[test]
fn offset_normalization_is_bounded() {
    // The quantization-is-cheap premise: normalized offset ∈ [0, CELL_SIZE).
    let unnormalized = GridPos::new([0, 0, 0], [25_000.0, -3.0, CELL_SIZE + 1.0]);
    let n = unnormalized.normalized();
    assert!(n.offset_is_bounded(), "offset {:?} not bounded", n.offset);
    // Normalization preserves world position.
    let w0 = unnormalized.world();
    let w1 = n.world();
    for i in 0..3 {
        assert!((w0[i] - w1[i]).abs() < 1e-6, "normalize changed world on axis {i}");
    }
}
