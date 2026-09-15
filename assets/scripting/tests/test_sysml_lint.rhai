// SysML lint policy self-test. The data and expected rules are authored in
// Rhai so this is the same shape a Twin requirement test uses at runtime.

fn require(condition, message) {
    if !condition { throw message; }
}

let facts = #{
    requirements: [#{
        qualified_name: "Example::r1",
        kind: "RequirementUsage",
        documentation: [],
        subjects: []
    }],
    verifications: [#{
        qualified_name: "Example::V",
        subjects: [],
        verifies: ["missing"]
    }]
};

let findings = lint_sysml(facts);
require(findings.len() == 4, "all structural SysML faults must be reported");

let rules = [];
for finding in findings { rules.push(finding.rule); }
require(rules.contains("sysml-requirement-usage-missing-subject"),
    "missing requirement subject must be reported");
require(rules.contains("sysml-verification-missing-subject"),
    "missing verification subject must be reported");
require(rules.contains("sysml-verification-target-unresolved"),
    "unresolved verification target must be reported");
require(rules.contains("sysml-requirement-usage-unverified"),
    "unverified requirement usage must be reported");

let clean = #{
    requirements: [#{
        qualified_name: "Example::r1",
        kind: "RequirementUsage",
        documentation: ["documented"],
        subjects: [#{name: "subject", type_name: "Part"}]
    }],
    verifications: [#{
        qualified_name: "Example::V",
        subjects: [#{name: "subject", type_name: "Part"}],
        verifies: ["r1"]
    }]
};
require(lint_sysml(clean).is_empty(), "a fully traced requirement must lint clean");
print("SYSML_LINT_SELFTEST PASS");
