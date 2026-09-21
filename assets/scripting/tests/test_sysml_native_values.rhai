// Production Rhai coverage for native typed SysML reads, explicit query
// failures, and the non-fatal RuntimeDiagnostics warning shown to authors.

import "sysml_requirements" as sysml_requirements;

let checks = [];
let path = "assets/scripting/tests/fixtures/sysml_native_values.sysml";
let missing_source_path = "assets/scripting/tests/fixtures/missing.sysml";
let qualified_position_attribute = "NativeValues::Lander::position";
let report = sysml_report(path);
checks.push(t_true(report.ok == true, "valid SysML fixture returns a source report"));
let contract_facts = query("AnalyzeSysml", #{path: path,
    tables: ["requirements", "verifications"]});
checks.push(t_true(contract_facts.ok == true && contract_facts.analysis != ()
    && contract_facts.analysis.source_revision_hex == report.analysis.source_revision_hex
    && contract_facts.analysis.requirements.len() > 0
    && contract_facts.analysis.verifications.len() > 0,
    "Rhai-selected fact query returns requirement and verification records from the same source generation"));

let first_fact_page = query("AnalyzeSysml", #{path: path,
    tables: ["requirements", "verifications"], offset: 0, limit: 1});
let second_fact_page = query("AnalyzeSysml", #{path: path,
    tables: ["requirements", "verifications"], offset: 1, limit: 1});
checks.push(t_true(first_fact_page.ok == true && second_fact_page.ok == true
    && first_fact_page.analysis.page.offset == 0
    && first_fact_page.analysis.page.limit == 1
    && first_fact_page.analysis.page.tables.requirements.total == 2
    && first_fact_page.analysis.page.tables.requirements.returned == 1
    && first_fact_page.analysis.page.tables.requirements.has_more == true
    && second_fact_page.analysis.page.offset == 1
    && second_fact_page.analysis.page.tables.requirements.returned == 1
    && second_fact_page.analysis.page.tables.requirements.has_more == false
    && first_fact_page.analysis.source_revision_hex
        == second_fact_page.analysis.source_revision_hex,
    "the generic typed-facts query pages requirement records with stable source identity and totals"));
checks.push(t_true(first_fact_page.analysis.page.tables.verifications.total == 1
    && first_fact_page.analysis.page.tables.verifications.returned == 1
    && first_fact_page.analysis.page.tables.verifications.has_more == false
    && second_fact_page.analysis.page.tables.verifications.returned == 0,
    "fact-table pages preserve independent table lengths under one bounded offset"));

let calibration = sysml_requirements::vector(report,
    "NativeValues::Lander::calibration");
let station_names = sysml_requirements::string_vector(report,
    "NativeValues::Lander::stationNames");
checks.push(t_true(type_of(calibration) == "array" && calibration.len() == 3
    && calibration[0] == 0.25 && calibration[2] == 0.75,
    "Rhai classifies a resolved Real collection and reads its values without Rust policy flags"));
checks.push(t_true(type_of(station_names) == "array" && station_names.len() == 2
    && station_names[0] == "fore" && station_names[1] == "aft",
    "Rhai classifies a resolved String collection and reads its values without Rust policy flags"));

let native = sysml_analysis(path);
checks.push(t_true(native.ok == true && native.analysis != ()
    && native.analysis.source_revision_hex == report.analysis.source_revision_hex,
    "the native typed snapshot and policy-neutral query identify the same source generation"));
let native_position = sysml_value_from_report(native, qualified_position_attribute);
checks.push(t_true(native_position.ok == true && native_position.found == true
    && type_of(native_position.value) == "Vec3"
    && native_position.value.x == 1.0000000000000002
    && native_position.value.y == -2.0 && native_position.value.z == 3.5,
    "a typed native snapshot reuses its already projected literal value"));

let rail_length_attribute = ();
let rail_offset_attribute = ();
let rail_stations_attribute = ();
let skirt_length_attribute = ();
let mode_attribute = ();
for attribute in native.analysis.attributes {
    if attribute.qualified_name == "NativeValues::Lander::railLength" {
        rail_length_attribute = attribute;
    }
    if attribute.qualified_name == "NativeValues::Lander::railOffset" {
        rail_offset_attribute = attribute;
    }
    if attribute.qualified_name == "NativeValues::Lander::railStations" {
        rail_stations_attribute = attribute;
    }
    if attribute.qualified_name == "NativeValues::Lander::skirtLength" {
        skirt_length_attribute = attribute;
    }
    if attribute.qualified_name == "NativeValues::Lander::mode" {
        mode_attribute = attribute;
    }
}
let rail_length = sysml_value(path, "NativeValues::Lander::railLength");
checks.push(t_true(rail_length_attribute != ()
    && type_of(rail_length_attribute.declared_type) == "SysmlType"
    && type_of(rail_length_attribute.declared_type.primitive) == "SysmlPrimitiveType"
    && rail_length.ok == true && type_of(rail_length.value) == "f64"
    && rail_length.value == rail_length_attribute.typed_value
    && rail_stations_attribute != ()
    && rail_stations_attribute.declared_type.is_collection
    && rail_stations_attribute.declared_type.dimensions.len() == 1
    && rail_stations_attribute.declared_type.dimensions[0]
        == rail_stations_attribute.typed_value.len()
    && type_of(rail_stations_attribute.typed_value) == "array",
    "numeric SysML scalars and fixed arrays retain typed cardinality and values in Rhai"));

let rail_offset = sysml_value(path, "NativeValues::Lander::railOffset");
checks.push(t_true(rail_offset_attribute != () && rail_offset.ok == true
    && type_of(rail_offset.value) == "f64"
    && rail_offset.value == rail_offset_attribute.typed_value
    && rail_offset_attribute.declared_type.resolved_type.qualified_name
        == "NativeValues::RailCoordinate"
    && type_of(rail_offset_attribute.declared_type.primitive) == "SysmlPrimitiveType",
    "a user-defined SysML specialization of Real resolves to the native primitive type"));

let skirt_length = sysml_value(path, "NativeValues::Lander::skirtLength");
let lander_mode = sysml_value(path, "NativeValues::Lander::mode");
checks.push(t_true(skirt_length_attribute != () && mode_attribute != ()
    && skirt_length.ok == true && type_of(skirt_length.value) == "Quantity"
    && skirt_length.value.value == skirt_length_attribute.typed_value.value
    && skirt_length.value.unit == skirt_length_attribute.typed_value.unit
    && type_of(skirt_length.value.kind) == "SysmlTypeRef"
    && skirt_length.value.kind.qualified_name
        == skirt_length_attribute.declared_type.quantity_kind.qualified_name
    && lander_mode.ok == true && type_of(lander_mode.value) == "EnumValue"
    && lander_mode.value.type_ref.qualified_name
        == mode_attribute.declared_type.resolved_type.qualified_name
    && lander_mode.value.literal == mode_attribute.typed_value.literal,
    "source quantities and enumeration values arrive as native typed Rhai objects"));

let lander_record = ();
for record in native.analysis.records {
    if record.type_name == "NativeValues::Lander" { lander_record = record; }
}
let record_rail_length = if lander_record == () { () } else { lander_record.value("railLength") };
checks.push(t_true(lander_record != ()
    && lander_record.has_field("railLength")
    && lander_record.source.file.ends_with("sysml_native_values.sysml")
    && lander_record.source.revision == native.analysis.source_revision
    && type_of(record_rail_length) == "f64" && record_rail_length == rail_length.value,
    "component records preserve typed fields, identity, and source provenance"));

let rail_requirement = false;
let rail_verification = false;
for requirement in native.analysis.requirements {
    if requirement.element.qualified_name == "NativeValues::railLengthRequirement"
        && requirement.subjects.len() == 1
        && requirement.subjects[0].name == "vehicle" {
        rail_requirement = true;
    }
}
for verification in native.analysis.verifications {
    if verification.element.qualified_name == "NativeValues::VerifyRailLength"
        && verification.subjects.len() == 1
        && verification.subjects[0].name == "vehicle"
        && verification.verifies.len() == 1
        && verification.verifies[0] == "railLengthRequirement" {
        rail_verification = true;
    }
}
checks.push(t_true(rail_requirement && rail_verification,
    "SysML requirement and verification subjects and links are available to Rhai policy"));

let api_facts = query("AnalyzeSysml", #{path: path,
    tables: ["attributes", "requirements", "verifications"],
    attribute_names: ["NativeValues::Lander::railStations"]});
checks.push(t_true(api_facts.ok == true
    && api_facts.analysis.attributes.len() == 1
    && api_facts.analysis.attributes[0].declared_type.base == "Real"
    && api_facts.analysis.attributes[0].declared_type.modelica_type == "RealArray"
    && api_facts.analysis.attributes[0].declared_type.dimensions[0]
        == api_facts.analysis.attributes[0].value.elements.len()
    && api_facts.analysis.requirements.len() > 0
    && api_facts.analysis.verifications.len() > 0,
    "the public typed-facts query exposes selected parameters and traceability"));

let derived_type_facts = query("AnalyzeSysml", #{path: path,
    tables: ["attributes"],
    attribute_names: ["NativeValues::Lander::railOffset"]});
checks.push(t_true(derived_type_facts.ok == true
    && derived_type_facts.analysis.attributes.len() == 1
    && derived_type_facts.analysis.attributes[0].declared_type.primitive == "Real"
    && derived_type_facts.analysis.attributes[0].declared_type.modelica_type == "Real"
    && derived_type_facts.analysis.attributes[0].value.number_value == 1.25,
    "the generic fact query preserves scalar inheritance and Modelica lowering"));

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
let selected_attributes = query("AnalyzeSysml", #{path: path,
    tables: ["attributes"], attribute_names: [qualified_position_attribute]});
let selected_value = sysml_value_from_report(selected_attributes,
    qualified_position_attribute);
checks.push(t_true(selected_attributes.ok == true
    && selected_attributes.analysis.attributes.len() == 1
    && selected_attributes.analysis.attributes[0].qualified_name == qualified_position_attribute
    && selected_value.ok == true && selected_value.found == true
    && type_of(selected_value.value) == "Vec3"
    && selected_value.value.x == direct.value.x
    && selected_value.value.y == direct.value.y
    && selected_value.value.z == direct.value.z,
    "an attributes-only Rhai projection returns the selected native value and source identity"));
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
