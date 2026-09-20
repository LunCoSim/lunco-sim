// The Workspace API exposes typed Twin manifest records independently of
// SysML validation, leaving component/verification policy to Rhai.

let checks = [];
let contract = query("ReadActiveTwinContract", #{});
checks.push(t_true(type_of(contract) == "map"
    && type_of(contract.open) == "bool",
    "the workspace exposes an explicit active-Twin contract state"));

if contract.open == true {
    checks.push(t_true(type_of(contract.name) == "string"
        && type_of(contract.components) == "array"
        && type_of(contract.verification_cases) == "array"
        && type_of(contract.component_errors) == "array"
        && type_of(contract.verification_errors) == "array"
        && type_of(contract.sysml_source_errors) == "array",
        "an open Twin contract exposes typed manifest registries and diagnostics"));
    for component in contract.components {
        checks.push(t_true(type_of(component.name) == "string"
            && type_of(component.requirements) == "string"
            && type_of(component.verification) == "string",
            "component ownership remains an explicit typed record at the Rhai boundary"));
    }
    for verification in contract.verification_cases {
        checks.push(t_true(type_of(verification.name) == "string"
            && type_of(verification.scene) == "string"
            && type_of(verification.script) == "string",
            "verification mappings remain explicit typed records at the Rhai boundary"));
    }
}

t_report(checks);
