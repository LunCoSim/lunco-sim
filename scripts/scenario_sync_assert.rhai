// Scene-graph assertion for the scenario-sync integration test
// (scripts/scenario_sync_test.sh). Run against the HOST serving the twin — NOT
// the client: a networked client deliberately does not run scripts
// (`scripts_run_here` is false for `NetworkRole::Client`, so authoritative
// scenario logic isn't double-applied), so `drain_world_scripts` never executes
// there and a rhai assertion would hang. The client's synced scene is asserted
// separately with the synchronous `ListEntities` API (scripts/api/entities_present.py).
//
// The prim paths to check come from `ASSERT_PRIMS`, injected by the harness as a
// prelude (`rhai_eval.py <port> -e 'let ASSERT_PRIMS = [...];' -f this`). Each
// entity's Bevy `Name` is its full USD prim path, so `find(path) >= 0` proves the
// prim is present in this peer's scene graph. Verdict on the LAST line:
// `SYNC_OK` or `SYNC_FAIL missing: …`.

let missing = "";
for path in ASSERT_PRIMS {
    if find(path) < 0 {
        missing += path + " ";
    }
}

if missing == "" {
    print("SYNC_OK");
} else {
    print("SYNC_FAIL missing: " + missing);
}
