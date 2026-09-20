// Production Rhai coverage for native typed SysML reads, explicit query
// failures, and the non-fatal RuntimeDiagnostics warning shown to authors.

import "sysml_requirements" as sysml_requirements;

let checks = [];
let path = "assets/scripting/tests/fixtures/sysml_native_values.sysml";
let missing_source_path = "assets/scripting/tests/fixtures/missing.sysml";
let qualified_position_attribute = "NativeValues::Lander::position";
let report = sysml_report(path);
checks.push(t_true(report.ok == true, "valid SysML fixture returns a source report"));

let direct = sysml_value(path, qualified_position_attribute);
checks.push(t_true(direct.ok == true && direct.found == true
    && type_of(direct.value) == "Vec3"
    && direct.value.x == 1.0000000000000002
    && direct.value.y == -2.0 && direct.value.z == 3.5,
    "standard Cartesian three-vector is preserved as a native f64 Vec3"));

let position_alias = sysml_value(path, "NativeValues::Lander::positionAlias");
checks.push(t_true(position_alias.ok == true && position_alias.found == true
    && type_of(position_alias.value) == "Vec3"
    && position_alias.value.x == 4.5 && position_alias.value.y == -6.0
    && position_alias.value.z == 7.25,
    "a SysML Position specialization of CartesianThreeVectorValue uses native Vec3"));

let position_stations = sysml_value(path, "NativeValues::Lander::positionStations");
checks.push(t_true(position_stations.ok == true && position_stations.found == true
    && type_of(position_stations.value) == "array"
    && type_of(position_stations.value[0]) == "Vec3"
    && position_stations.value[0].x == 1.0
    && position_stations.value[1].y == 5.0,
    "a fixed SysML Position collection retains native Vec3 station values"));

let from_report = sysml_value_from_report(report, qualified_position_attribute);
checks.push(t_true(from_report.ok == true && from_report.found == true
    && type_of(from_report.value) == "Vec3"
    && from_report.value.x == direct.value.x,
    "full reports expose the same typed value without a second conversion path"));

let summary = query("AnalyzeSysml", #{path: path,
    tables: ["requirements", "verifications"]});
let unprojected = sysml_value_from_report(summary, qualified_position_attribute);
checks.push(t_true(unprojected.ok == true && unprojected.found == false,
    "an intentionally omitted fact table is distinct from a failed lookup"));
let lazy = sysml_requirements::native_value(summary, qualified_position_attribute);
checks.push(t_true(type_of(lazy) == "Vec3" && lazy.x == direct.value.x,
    "Rhai resolves an unloaded value through the typed selected query"));

let missing = sysml_value(path, "NativeValues::Lander::missing");
checks.push(t_true(missing.ok == false && missing.found == false
    && missing.error.contains("missing"),
    "a missing qualified attribute is a visible structured error, not unit"));
let missing_from_helper = sysml_requirements::native_value(report,
    "NativeValues::Lander::missing");
checks.push(t_true(type_of(missing_from_helper) == "map"
    && missing_from_helper.ok == false
    && missing_from_helper.error.contains("missing"),
    "Rhai helper preserves the specific typed-query error"));
let warning_report = query("RuntimeDiagnostics", #{});
let has_warning = false;
for finding in warning_report.findings {
    if finding.producer == "rhai-sysml" && finding.code == "sysml-query"
        && finding.severity == "warning" { has_warning = true; }
}
checks.push(t_true(has_warning,
    "failed typed lookup appears as a non-fatal authoring warning"));

let broken = sysml_value(missing_source_path, qualified_position_attribute);
checks.push(t_true(broken.ok == false && broken.error != (),
    "source-loading errors return to Rhai without aborting the app"));
let broken_report = sysml_report(missing_source_path);
let failed_evaluation = sysml_requirements::evaluate(broken_report, []);
let source_error = if broken_report.error != () { broken_report.error }
    else if broken_report.errors != () && broken_report.errors.len() > 0 {
        broken_report.errors[0]
    } else { "" };
checks.push(t_true(failed_evaluation.ok == false
    && source_error != "" && failed_evaluation.error == source_error,
    "requirement evaluation preserves the source provider error"));

let repaired = sysml_value(path, qualified_position_attribute);
let after_repair = query("RuntimeDiagnostics", #{});
let fixture_warning = false;
let unrelated_warning = false;
for finding in after_repair.findings {
    if finding.producer == "rhai-sysml" && finding.code == "sysml-query"
        && finding.subject == path { fixture_warning = true; }
    if finding.producer == "rhai-sysml" && finding.code == "sysml-query"
        && finding.subject == missing_source_path { unrelated_warning = true; }
}
checks.push(t_true(repaired.ok == true && !fixture_warning,
    "a later valid read clears the warning for the repaired source"));
checks.push(t_true(unrelated_warning,
    "a valid read does not erase an unrelated source warning"));

t_report(checks);
