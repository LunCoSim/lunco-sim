// Production Rhai check that evidence is projected from typed SysML
// attributes into a structured record with source spans and decoded strings.

import "sysml_requirements" as sysml_requirements;

let checks = [];
let path = "assets/scripting/tests/fixtures/sysml_provenance.sysml";
let report = sysml_report(path);
checks.push(t_true(report.ok == true,
    "the standard SysML evidence fixture parses through the production source API"));

let evidence = sysml_requirements::provenance(report, "GR-001");
checks.push(t_true(evidence.ok == true,
    "the requirements policy groups evidence attributes by their owning element"));
checks.push(t_true(evidence.evidence.requirement_id == "GR-001"
    && evidence.evidence.qualified_requirement == "Griffin::GR001"
    && evidence.evidence.source_reference == "https://example.org/griffin-study"
    && evidence.evidence.rationale == "sets a traceable first-pass geometry datum"
    && evidence.evidence.status == "study_assumption",
    "provenance fields remain structured decoded values rather than quoted comment text"));
checks.push(t_true(evidence.evidence.source_span.file != ()
    && evidence.evidence.source_span.start < evidence.evidence.source_span.end
    && evidence.source_revision_hex == report.analysis.source_revision_hex,
    "evidence carries a source span and the exact analysis revision"));

t_report(checks);
