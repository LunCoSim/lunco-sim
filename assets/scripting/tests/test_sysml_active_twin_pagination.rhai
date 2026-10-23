// Production Rhai integration check for a full active-Twin source report.
// Run with a Twin mounted; this exercises the same policy path Griffin uses.

import "sysml_requirements" as sysml_requirements;

let checks = [];
let contract = query("ReadActiveTwinContract", #{});
checks.push(t_true(contract.open == true,
    "an active Twin is mounted for the source pagination check"));

if contract.open == true {
    let source = sysml_requirements::source();
    checks.push(t_true(source.ok == true
        && source.source_revision != ()
        && source.source_page_count > 0
        && source.source_fact_totals != ()
        && source.source_fact_totals.requirements == source.requirement_records.len()
        && source.source_fact_totals.verifications == source.verification_records.len(),
        "the Rhai source policy assembles all paged identities from one stable typed source"));

    let largest_table = source.source_fact_totals.requirements;
    if source.source_fact_totals.verifications > largest_table {
        largest_table = source.source_fact_totals.verifications;
    }
    let expected_page_count = 1;
    let covered = 8;
    while covered < largest_table {
        covered += 8;
        expected_page_count += 1;
    }
    checks.push(t_true(largest_table > 8
        && source.source_page_count > 1
        && source.source_page_count == expected_page_count,
        "the large active-Twin source reader traverses every bounded fact page"));

    let names = #{};
    let unique_requirement_identities = true;
    let pinned_identities = true;
    for record in source.requirement_records {
        let name = record.qualified_name;
        if record.element.handle.source_revision != source.source_revision {
            pinned_identities = false;
        }
        if name == () || name == "" {
            unique_requirement_identities = false;
        } else if names[name] == true {
            unique_requirement_identities = false;
        } else {
            names[name] = true;
        }
    }
    checks.push(t_true(unique_requirement_identities
        && source.requirement_records.len() > 0,
        "the native Rhai requirement index preserves every qualified identity exactly once"));

    checks.push(t_true(type_of(source.requirement_records[0]) == "SysmlRequirement"
        && type_of(source.verification_records[0]) == "SysmlVerification",
        "full source records stay native rather than accumulating flattened strings"));

    let linked_verification = false;
    let complete_coverage = true;
    let coverage_edges = 0;
    for record in source.verification_records {
        if record.element.handle.source_revision != source.source_revision {
            pinned_identities = false;
        }
        for target in record.verified_requirements {
            coverage_edges += 1;
            if !source.requirement_coverage.covers(record.element.handle, target) {
                complete_coverage = false;
            }
        }
        if record.qualified_name != () && record.verifies != ()
            && record.verifies.len() > 0 {
            linked_verification = true;
        }
    }
    checks.push(t_true(linked_verification,
        "the native Rhai verification index retains source-backed requirement links"));
    checks.push(t_true(pinned_identities && complete_coverage && coverage_edges > 0,
        "every identity and resolved coverage edge belongs to the pinned source"));
}

t_report(checks);
