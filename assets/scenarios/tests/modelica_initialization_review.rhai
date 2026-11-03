// Shared setup and verdicts for the focused initialization architecture gates.
import "lunco://scripting/tools/assembly_edit" as assembly;

const ROOT = "/World";

fn input(name, value) {
    #{name: name, type_name: "double", default_value: value, connection: ()}
}
fn attach(doc, name, model, inputs) {
    let result = assembly::attach_program(doc, #{edit_target: "@runtime@",
        host_path: ROOT, name: name, source_asset: "lunco://models/tests/" + model + ".mo",
        inputs: inputs, outputs: [#{name: "sample", type_name: "double", connections: []}],
        realtime_safe: false});
    if result.ok != true { throw(result.to_string()); }
}
fn wire(target, source) {
    #{SetConnection: #{edit_target: "@runtime@", path: ROOT + "/" + target,
        name: "inputs:feedback", type_name: "double",
        sources: [ROOT + "/" + source + ".outputs:sample"]}}
}
fn native_control_ops() {
    let owner = ROOT + "/A";
    [
      #{AddPrim: #{edit_target: "@runtime@", parent_path: owner, name: "Controls", type_name: "Scope", reference: (), reference_prim_path: ""}},
      #{AddPrim: #{edit_target: "@runtime@", parent_path: owner + "/Controls", name: "forward", type_name: "Scope", reference: (), reference_prim_path: ""}},
      #{SetApiSchemas: #{edit_target: "@runtime@", path: owner + "/Controls/forward", schemas: ["LunCoControlBindingAPI"]}},
      #{SetAttribute: #{edit_target: "@runtime@", path: owner + "/Controls/forward", name: "lunco:port", type_name: "string", value: "\"manual_axis\""}},
      #{SetAttribute: #{edit_target: "@runtime@", path: owner + "/Controls/forward", name: "lunco:factor", type_name: "double", value: "1"}},
      #{SetAttribute: #{edit_target: "@runtime@", path: owner, name: "inputs:manual_axis", type_name: "double", value: "0"}},
      #{SetAttribute: #{edit_target: "@runtime@", path: owner, name: "outputs:piloted", type_name: "double", value: "0"}},
      #{SetConnection: #{edit_target: "@runtime@", path: owner, name: "inputs:piloted", type_name: "double", sources: [owner + ".outputs:piloted"]}}
    ]
}
fn start(kind, title) {
    let state = #{kind: kind, title: title, done: false, ticks: 0, doc: (), names: [], expected: []};
    for doc in query("ListOpenDocuments").open_documents {
        if doc.kind == "usd" && doc.title == title && doc.runtime_context.kind == "local_twin" {
            state.doc = doc.doc_id;
        }
    }
    if state.doc == () { throw("initialization fixture document is missing: " + title); }
    if kind == "parameters" {
        attach(state.doc, "Expression", "InitialExpressionParameter", [input("k", 9.0)]);
        attach(state.doc, "Inherited", "InitialInheritedParameter", [input("k", 9.0)]);
        attach(state.doc, "Unbound", "InitialUnboundParameter", [input("k", 9.0)]);
        attach(state.doc, "Structural", "InitialStructuralParameter", [input("n", 3.0)]);
        state.names = ["Expression", "Inherited", "Unbound", "Structural"]; state.expected = [9.0, 9.0, 9.0, 9.0];
    } else {
        if kind == "inherited" {
            attach(state.doc, "Control", "InitialInheritedFeedback",
                [input("feedback", 0.0), input("gain", 1.0), input("bias", 1.0)]);
            attach(state.doc, "A", "InitialInheritedFeedback",
                [input("feedback", 0.0), input("gain", 1.0), input("bias", 0.0)]);
            attach(state.doc, "B", "InitialInheritedFeedback",
                [input("feedback", 0.0), input("gain", 0.5), input("bias", 1.0)]);
            state.expected = [2.0, 2.0];
        } else if kind == "state" {
            attach(state.doc, "A", "InitialBoundaryConstraint", [input("feedback", 0.0)]);
            attach(state.doc, "B", "InitialFeedback",
                [input("feedback", 0.0), input("gain", 1.0), input("bias", -1.0)]);
            state.expected = [1.0, 0.0];
        } else { throw("unknown initialization case: " + kind); }
        state.names = ["A", "B"];
        let ops = [wire("A", "B"), wire("B", "A")];
        if kind == "inherited" { ops += native_control_ops(); }
        let result = cmd("ApplyUsdOps", #{doc_id: state.doc, ops: ops});
        if result.ok != true { throw(result.to_string()); }
    }
    state
}
fn row(rows, name) {
    for item in rows { if item.name == ROOT + "/" + name { return item; } }
    ()
}
fn finish(state, failures) {
    state.done = true;
    report_verdict(failures, state.title, "MODELICA_INITIALIZATION_REVIEW");
    state
}
fn observe(state) {
    if state.done { return state; }
    state.ticks += 1;
    let ready = query("GetReadiness");
    let status = query("CosimStatus");
    if ready.faulted {
        print(state.title + " fault: " + ready.to_string());
        print(state.title + " participants: " + status.to_string());
        return finish(state, ["joint initialization must reach consistent live participants: " + ready.fault.detail]);
    }
    let waiting = false;
    for name in state.names {
        let item = row(status.entities, name);
        let rejected = item != () && item.status.starts_with("Error:");
        if rejected && state.kind != "parameters" {
            return finish(state, [name + " rejected: " + item.status]);
        }
        waiting = waiting || item == () || (!rejected && item.status != "Running" && item.status != "Idle");
    }
    if waiting {
        // Presentation remains observable while numerical admission holds ticks.
        if state.ticks >= 3600 {
            print(state.title + " pending: " + status.to_string());
            let checks = [];
            if state.kind == "inherited" {
                let control = row(status.entities, "Control");
                checks.push(t_true(control != () && control.outputs.sample == 1.0,
                    "the same inherited model initializes successfully without feedback wiring"));
            }
            checks.push(t_true(false, "initialization did not produce live participants within 3600 presentation updates"));
            return finish(state, checks);
        }
        return state;
    }
    let checks = [];
    for index in 0..state.names.len() {
        let item = row(status.entities, state.names[index]);
        let value = item.outputs.sample;
        if state.kind == "parameters" {
            checks.push(t_true(!item.status.starts_with("Error:"),
                state.names[index] + " accepts the declared parameter: " + item.status));
        }
        print(state.title + " " + state.names[index] + " expected=" + state.expected[index] + " actual=" + value);
        checks.push(t_true(value != () && value.is_finite() && abs(value - state.expected[index]) < 1.0e-9,
            state.names[index] + " preserves its authored initial solution"));
    }
    if state.kind == "inherited" {
        let a = row(status.entities, "A");
        checks.push(t_true(a.outputs.g == () && a.outputs.b == () && a.outputs.gain == (),
            "initialized parameters do not acquire public output ports"));
        let retained_state = false;
        for channel in query("ListTelemetryChannels", #{name: "x"}).channels {
            if channel.model_class == "InitialInheritedFeedback" && channel.model_variable == "x"
                && channel.active && channel.samples > 0 { retained_state = true; }
        }
        checks.push(t_true(retained_state,
            "initialized state remains available to telemetry"));
        let native = query("ReadPorts", #{api_id: find_path(ROOT + "/A"), port_names: ["piloted"]});
        let native_sample = ();
        if native != () && native.ports != () {
            for port in native.ports {
                if port.name == "piloted" && port.direction == "out" { native_sample = port.value; }
            }
        }
        checks.push(t_true(native_sample == 0.0,
            "a Modelica input does not shadow the native control-authority output"));
    }
    if state.kind != "parameters" {
        checks.push(t_true(status.last_coupled_initialization != (), "coupled initialization has a certified receipt"));
    }
    finish(state, checks)
}
