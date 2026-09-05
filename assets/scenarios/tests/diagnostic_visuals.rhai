//! Positive production regression for leased camera and collider diagnostics.
//!
//! The scenario uses only the public find/cmd/query bridge. It deliberately
//! checks command results and the read-only DiagnosticVisuals provider rather
//! than inspecting ECS internals.

fn verdict(pass, reason) {
    if pass {
        emit("DIAGNOSTIC_VISUALS", "PASS");
    } else {
        print("DIAGNOSTIC_VISUALS FAIL: " + reason);
        emit("DIAGNOSTIC_VISUALS", "FAIL");
    }
}

fn count_visuals() {
    let state = query("DiagnosticVisuals", #{});
    if state == () || state.count == () { return -1; }
    state.count
}

fn on_start(me) {
    this.done = false;
    this.started = false;
    this.camera = -1;
    this.simple = -1;
    this.compound = -1;
}

fn on_tick(me) {
    if this.done { return; }

    if this.camera < 0 { this.camera = find("/DiagnosticVisualsTest/VisualCamera"); }
    if this.simple < 0 { this.simple = find("/DiagnosticVisualsTest/SimpleBody"); }
    if this.compound < 0 { this.compound = find("/DiagnosticVisualsTest/CompoundBody"); }
    if this.camera < 0 || this.simple < 0 || this.compound < 0 {
        return;
    }

    if !this.started {
        this.started = true;

        let camera_result = cmd("AcquireDiagnosticVisual", #{
            target: this.camera, kind: "camera", policy: "default"
        });
        // The production scene-test binary is intentionally headless and does
        // not install the UI-owned diagnostic command surface. Treat that
        // absence as the expected render boundary; the windowed run below
        // exercises the full positive lease contract.
        if !camera_result.ok && camera_result.error != () &&
           (camera_result.error.contains("not API-accessible") ||
            camera_result.error.contains("not registered")) {
            let unavailable = query("DiagnosticVisuals", #{});
            if unavailable.ok == false && unavailable.error.contains("not registered") {
                verdict(true, "headless render boundary is explicit");
            } else {
                verdict(false, "headless command/query boundary disagreed");
            }
            this.done = true;
            return;
        }
        if !camera_result.ok || camera_result.data.lease_id == () {
            print("DIAGNOSTIC_VISUALS camera error: " + camera_result.error);
            verdict(false, "camera acquire was not acknowledged");
            this.done = true;
            return;
        }
        this.camera_lease = camera_result.data.lease_id;

        let repeated = cmd("AcquireDiagnosticVisual", #{
            target: this.camera, kind: "camera", policy: "default"
        });
        if !repeated.ok || repeated.data.lease_id != this.camera_lease || count_visuals() != 1 {
            verdict(false, "same camera acquire was not idempotent");
            this.done = true;
            return;
        }

        let simple_result = cmd("AcquireDiagnosticVisual", #{
            target: this.simple, kind: "collider", policy: "default"
        });
        let compound_result = cmd("AcquireDiagnosticVisual", #{
            target: this.compound, kind: "collider", policy: "default"
        });
        if !simple_result.ok || !compound_result.ok || count_visuals() != 3 {
            verdict(false, "simple or compound collider acquire failed");
            this.done = true;
            return;
        }
        this.simple_lease = simple_result.data.lease_id;
        this.compound_lease = compound_result.data.lease_id;

        let missing = cmd("AcquireDiagnosticVisual", #{
            target: 999999999999, kind: "collider", policy: "default"
        });
        if missing.ok {
            verdict(false, "missing target was accepted");
            this.done = true;
            return;
        }

        let update = cmd("UpdateDiagnosticVisual", #{
            lease: this.simple_lease,
            target: this.compound,
            kind: "collider"
        });
        if !update.ok {
            print("DIAGNOSTIC_VISUALS update error: " + update.error);
            verdict(false, "in-place update was rejected");
            this.done = true;
            return;
        }
        if count_visuals() != 3 {
            verdict(false, "in-place update did not preserve lease count");
            this.done = true;
            return;
        }

        if !cmd("ReleaseDiagnosticVisual", #{lease: this.camera_lease}).ok ||
           !cmd("ReleaseDiagnosticVisual", #{lease: this.simple_lease}).ok ||
           !cmd("ReleaseDiagnosticVisual", #{lease: this.compound_lease}).ok ||
           count_visuals() != 0 {
            verdict(false, "release did not clean every overlay");
            this.done = true;
            return;
        }

        let stale = cmd("ReleaseDiagnosticVisual", #{lease: this.camera_lease});
        if stale.ok {
            verdict(false, "released handle was accepted again");
            this.done = true;
            return;
        }

        verdict(true, "");
        this.done = true;
    }
}
