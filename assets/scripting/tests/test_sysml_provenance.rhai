// Production Rhai check that typed SysML element references produce structured
// provenance with one nominal SourceLocator per source.

import "sysml_requirements" as sysml_requirements;

let checks = [];
let path = "assets/scripting/tests/fixtures/sysml_provenance.sysml";
let report = sysml_report(path);
checks.push(t_true(report.ok == true,
    "the standard SysML evidence fixture parses through the production source API"));

let requirement = sysml_requirements::requirement_handle_by_short_name(report, "GR-001");
checks.push(t_true(requirement.ok == true,
    "the standard requirement short name resolves to a source-scoped element handle"));
let evidence = sysml_requirements::provenance(report, requirement.handle);
checks.push(t_true(evidence.ok == true,
    "the requirements policy follows resolved requirement and source relationships"));
checks.push(t_true(evidence.evidence.requirement == requirement.handle
    && evidence.evidence.sources.len() == 1
    && evidence.evidence.sources[0].locator == "https://example.org/griffin-study"
    && evidence.evidence.sources[0].locator_type == "EvidenceCatalog::SourceLocator"
    && evidence.evidence.sources[0].role.type == "EvidenceCatalog::EvidenceSourceRole"
    && evidence.evidence.sources[0].role.literal == "Reference"
    && evidence.evidence.sources[0].element != (),
    "provenance retains typed requirement/source handles, enum role, and nominal locator"));
checks.push(t_true(evidence.evidence.source_span.file != ()
    && evidence.evidence.source_span.start < evidence.evidence.source_span.end
    && evidence.source_revision == report.analysis.source_revision,
    "evidence carries a source relationship span and the exact analysis revision"));

t_report(checks);
