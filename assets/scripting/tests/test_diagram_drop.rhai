let checks = [];
let facts = #{ source: "lunco://models/3-control.mo", scope: "/System", target: "/System/Rover", paths: ["/System/Rover/_3_control"] };
let program = invoke_hook("diagram.drop.plan", [facts]);
checks.push(t_true(program.ok, "drop policy installed"));
if program.ok {
    checks.push(t_true(program.value.kind == "program" && program.value.parent == facts.target, "program attaches to explicit USD owner"));
    checks.push(t_true(program.value.name == "_3_control_2", "names are valid and unique"));
    checks.push(t_true(invoke_hook("diagram.drop.plan", [facts]).value == program.value, "same facts yield same plan"));
}
let reference = invoke_hook("diagram.drop.plan", [#{ source: "lunco://parts/rover.usda", scope: "/System", target: "", paths: [] }]);
checks.push(t_true(reference.ok && reference.value.kind == "reference" && reference.value.parent == "/System", "USD source adds a reference under the current scope"));
let absent = invoke_hook("diagram.drop.plan", [#{ source: "control.rhai", scope: "/System", target: "", paths: [] }]);
checks.push(t_true(absent.ok && absent.value.error != (), "program without a host rejects visibly"));
let unsupported = invoke_hook("diagram.drop.plan", [#{ source: "notes.txt", scope: "/System", target: "", paths: [] }]);
checks.push(t_true(unsupported.ok && unsupported.value.error != (), "unsupported source rejects visibly"));
let system = invoke_hook("diagram.open.plan", [#{ path: "/System", has_children: true, programs: [] }]);
checks.push(t_true(system.ok && system.value.action == "scope", "systems open their USD topology"));
let model = invoke_hook("diagram.open.plan", [#{ path: "/Model", has_children: false, programs: [#{ valid: true, backend: "Modelica", source: "lunco://models/Model.mo" }] }]);
checks.push(t_true(model.ok && model.value.action == "modelica" && model.value.source == "lunco://models/Model.mo", "leaf models retain their exact source identity"));
let script = invoke_hook("diagram.open.plan", [#{ path: "/Script", has_children: false, programs: [#{ valid: true, backend: "Rhai", source: "control.rhai" }] }]);
checks.push(t_true(script.ok && script.value.action == "source", "script nodes open source"));
let invalid = invoke_hook("diagram.open.plan", [#{ path: "/Invalid", has_children: false, programs: [#{ valid: false, backend: "Modelica", source: "invalid" }] }]);
checks.push(t_true(invalid.ok && invalid.value.error != (), "invalid attached metadata rejects navigation"));
report_verdict(checks, "DIAGRAM DROP POLICY", "DIAGRAM_DROP_POLICY");
