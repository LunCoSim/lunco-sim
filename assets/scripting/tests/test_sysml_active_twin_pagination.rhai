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
        && source.source_revision_hex != ()
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
    let covered = 16;
    while covered < largest_table {
        covered += 16;
        expected_page_count += 1;
    }
    checks.push(t_true(largest_table > 16
        && source.source_page_count > 1
        && source.source_page_count == expected_page_count,
        "the large active-Twin source reader traverses every bounded fact page"));

    let names = #{};
    let unique_requirement_identities = true;
    for record in source.requirement_records {
        let name = record.qualified_name;
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
        "the compact Rhai requirement index preserves every qualified identity exactly once"));

    let linked_verification = false;
    for record in source.verification_records {
        if record.qualified_name != () && record.verifies != ()
            && record.verifies.len() > 0 {
            linked_verification = true;
        }
    }
    checks.push(t_true(linked_verification,
        "the compact Rhai verification index retains source-backed requirement links"));
}

t_report(checks);
