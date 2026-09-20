// SysML lint policy self-test. The data and expected rules are authored in
// Rhai so this is the same shape a Twin requirement test uses at runtime.

let checks = [];
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

let linted = invoke_hook("lint.sysml", [facts]);
checks.push(t_true(linted.ok == true,
    "the structural SysML policy is installed through its named hook"));
let findings = linted.value;
checks.push(t_true(findings.len() == 4,
    "the structural policy reports all authored traceability findings"));

let rules = [];
for finding in findings { rules.push(finding.rule); }
checks.push(t_true(rules.contains("sysml-requirement-usage-missing-subject"),
    "the structural policy identifies an unbound requirement subject"));
checks.push(t_true(rules.contains("sysml-verification-missing-subject"),
    "the structural policy identifies a verification without a subject"));
checks.push(t_true(rules.contains("sysml-verification-target-unresolved"),
    "the structural policy identifies an unresolved verification target"));
checks.push(t_true(rules.contains("sysml-requirement-usage-unverified"),
    "the structural policy identifies an uncovered requirement usage"));

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
let clean_lint = invoke_hook("lint.sysml", [clean]);
checks.push(t_true(clean_lint.ok == true && clean_lint.value.is_empty(),
    "a fully traced requirement is accepted by the same structural policy"));
t_report(checks);
