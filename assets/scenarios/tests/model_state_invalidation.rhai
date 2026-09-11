// TEST-ONLY. A standard USD instance-input edit invalidates generic model
// state, and the owning backend reapplies its own lifecycle without mutating
// a second instance or the shared Modelica source asset.

const ROOT = "/ModelStateInvalidationTest";
const EDITABLE = ROOT + "/EditableOscillator";
const UNTOUCHED = ROOT + "/UntouchedOscillator";
const SOURCE = "lunco://models/Oscillator.mo";

fn query_ok(record) {
    record != () && record.ok != false
}

fn close_scalar(actual, expected) {
    actual != () && (actual - expected).abs() <= 0.0001
}

fn find_document() {
    let docs = query("ListOpenDocuments");
    if docs == () || docs.open_documents == () { return -1; }
    for item in docs.open_documents {
        if item.kind == "usd" {
            let root = query("QueryUsdPrim", #{
                doc_id: item.doc_id,
                path: ROOT,
                children: false,
            });
            if query_ok(root) { return item.doc_id; }
        }
    }
    -1
}

fn fail(me, message) {
    if !me.done {
        me.done = true;
        report_verdict([message], "MODEL STATE INVALIDATION", "MODEL_STATE_INVALIDATION");
    }
}

fn on_start(me) {
    this.doc = -1;
    this.editable = -1;
    this.untouched = -1;
    this.elapsed = 0.0;
    this.phase_elapsed = 0.0;
    this.after_elapsed = 0.0;
    this.before_peak = 0.0;
    this.after_peak = 0.0;
    this.untouched_peak = 0.0;
    this.wait_generation = -1;
    this.phase = 0;
    this.saw_reset = false;
    this.after_started = false;
    this.done = false;
}

fn on_tick(me) {
    if this.done { return; }
    this.elapsed += dt();
    if this.elapsed > 35.0 {
        fail(this, "model state invalidation acceptance timed out at phase " + this.phase);
        return;
    }

    if this.doc < 0 { this.doc = find_document(); }
    if this.editable < 0 { this.editable = find(EDITABLE); }
    if this.untouched < 0 { this.untouched = find(UNTOUCHED); }
    if this.doc < 0 || this.editable < 0 || this.untouched < 0 { return; }

    let signal = get(this.editable, "signal");
    let untouched_signal = get(this.untouched, "signal");
    if this.phase == 0 {
        if signal == () || untouched_signal == () { return; }
        this.phase_elapsed += dt();
        if signal.abs() > this.before_peak { this.before_peak = signal.abs(); }
        if untouched_signal.abs() > this.untouched_peak {
            this.untouched_peak = untouched_signal.abs();
        }
        if this.phase_elapsed < 2.5 { return; }

        let generation = assembly_edit::describe(this.doc).generation;
        let ack = assembly_edit::batch(this.doc, "Change oscillator instance amplitude", [#{
            SetAttribute: #{
                edit_target: "@root@",
                path: EDITABLE,
                name: "inputs:amplitude",
                type_name: "float",
                value: "1.6",
            },
        }], generation);
        if !query_ok(ack) {
            fail(this, "standard USD instance-input edit was rejected: " + ack.error);
            return;
        }
        this.wait_generation = generation;
        this.phase = 1;
        return;
    }

    if assembly_edit::describe(this.doc).generation <= this.wait_generation { return; }
    if !this.saw_reset {
        if signal == () { this.saw_reset = true; }
        return;
    }
    if !this.after_started {
        if signal == () { return; }
        this.after_started = true;
    }

    this.after_elapsed += dt();
    if signal.abs() > this.after_peak { this.after_peak = signal.abs(); }
    if untouched_signal.abs() > this.untouched_peak {
        this.untouched_peak = untouched_signal.abs();
    }
    if this.after_elapsed < 2.5 { return; }

    let edited = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: EDITABLE,
        attrs: ["inputs:amplitude", "info:sourceAsset"],
    });
    let untouched = query("QueryUsdPrim", #{
        doc_id: this.doc,
        path: UNTOUCHED,
        attrs: ["inputs:amplitude", "info:sourceAsset"],
    });
    let checks = [];
    checks.push(t_true(this.before_peak > 0.4 && this.before_peak < 0.6,
        "initial compiled instance uses amplitude 0.5"));
    checks.push(t_true(this.after_peak > 1.4 && this.after_peak < 1.7,
        "edited instance recompiles and uses amplitude 1.6"));
    checks.push(t_true(this.untouched_peak > 0.2 && this.untouched_peak < 0.3,
        "second instance keeps its independent amplitude 0.25"));
    checks.push(t_true(query_ok(edited)
            && close_scalar(edited.attrs["inputs:amplitude"], 1.6)
            && edited.attrs["info:sourceAsset"] == SOURCE,
        "USD stores the override on the edited instance and preserves its source"));
    checks.push(t_true(query_ok(untouched)
            && close_scalar(untouched.attrs["inputs:amplitude"], 0.25)
            && untouched.attrs["info:sourceAsset"] == SOURCE,
        "the untouched instance and shared source identity remain unchanged"));
    this.done = true;
    report_verdict(checks, "MODEL STATE INVALIDATION", "MODEL_STATE_INVALIDATION");
}
