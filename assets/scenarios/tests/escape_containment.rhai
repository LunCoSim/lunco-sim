// Negative production fixture for the scene-scoped physics escape boundary.
//
// The scene deliberately starts Escapee below the static world. The expected
// result is a structured terminal fault plus a physics hold, with the process
// still alive so the host can report the fault and be rebooted cleanly. The
// luncosim test harness owns the terminal verdict because RuntimeFaults is an
// engine boundary, not a mutable scenario blackboard.

fn on_start(me) {
    expect_runtime_fault("physics-body-escaped");
    print("[escape] expecting terminal physics-body-escaped for the authored Escapee");
}

fn on_tick(me) {
    // Intentionally no pose correction and no recovery command. The fixture
    // verifies containment by stopping physics at the root fault boundary.
}
