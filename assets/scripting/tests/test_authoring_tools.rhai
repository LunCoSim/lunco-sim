// Pure contract tests for the generic authoring facades. Asset-backed tests
// remain Twin-owned; this file checks only deterministic planning and routing.

import "authoring_session" as authoring_session;

fn require(condition, message) {
    if !condition { throw message; }
}

let usd = authoring_session::capabilities("usd");
require(usd.ok && usd.grouped_apply && usd.undoable && usd.preview,
    "USD authoring capabilities must expose grouped undoable preview edits");
require(usd.lifecycle.len() == 4, "USD must expose explicit lifecycle verbs");

let modelica = authoring_session::capabilities("modelica");
require(modelica.ok && modelica.generation_checked && modelica.source_checkpoint,
    "Modelica authoring capabilities must expose generation and source checks");

let sysml = authoring_session::capabilities("sysml");
require(sysml.ok && sysml.grouped_apply && sysml.undoable,
    "SysML authoring capabilities must expose grouped undoable edits");
require(sysml.lifecycle[0] == "open", "SysML lifecycle must start with explicit open");

let rhai = authoring_session::capabilities("rhai");
require(rhai.ok && rhai.generation_checked && rhai.source_checkpoint,
    "Rhai authoring capabilities must expose generation and source checks");

let unsupported = authoring_session::capabilities("python");
require(!unsupported.ok && unsupported.stage == "domain",
    "unsupported authoring domains must fail loudly");

let missing_document = authoring_session::inspect("sysml", 0);
require(!missing_document.ok && missing_document.stage == "inspect",
    "the unassigned document sentinel must not select an active tab");

let empty = authoring_session::plan("sysml", [], "empty", 0);
require(!empty.ok && empty.stage == "plan",
    "empty operation plans must be rejected before command admission");

let missing_target = authoring_session::plan("usd", [#{ AddPrim: #{} }], "bad", 0);
require(!missing_target.ok && missing_target.stage == "plan",
    "USD plans without an explicit edit target must be rejected");

let good = authoring_session::plan("sysml",
    [#{ EditText: #{ range_start: 0, range_end: 0, replacement: "package P {}" } }],
    "author source", 4);
require(good.ok && good.operation_count == 1 && good.parent_generation == 4,
    "valid SysML plan must be dry and retain its generation cursor");

print("TESTS_OK authoring_tools_contract");
