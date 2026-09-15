// SysML v2 structural lint policy.
//
// Rust owns parsing/resolution and supplies typed `facts`; Rhai owns the
// project-independent authoring rules. This policy is intentionally about
// traceability shape, not a project-specific requirement id or dimension.

fn finding(rule, severity, subject, message) {
    let out = #{};
    out.rule = rule;
    out.severity = severity;
    out.subject = subject;
    out.message = message;
    out
}

fn has_requirement_target(target, requirements) {
    for requirement in requirements {
        let name = requirement.qualified_name;
        if name == target || name.ends_with("::" + target) {
            return true;
        }
    }
    false
}

fn requirement_is_verified(requirement, verifications) {
    let qualified = requirement.qualified_name;
    let short = qualified;
    if qualified.contains("::") {
        let pieces = qualified.split("::");
        short = pieces[pieces.len() - 1];
    }
    for verification in verifications {
        for target in verification.verifies {
            let exact = target == qualified;
            let local = target == short;
            let suffix = qualified.ends_with("::" + target);
            if exact || local || suffix { return true; }
        }
    }
    false
}

fn requirement_documentation(facts) {
    let out = [];
    for requirement in facts.requirements {
        if requirement.kind != "RequirementDefinition" { continue; }
        if !requirement.documentation.is_empty() { continue; }
        out.push(finding(
            "sysml-requirement-missing-documentation",
            "warn",
            requirement.qualified_name,
            "requirement definition has no documentation block; describe its intent before binding verification evidence",
        ));
    }
    out
}

fn requirement_subjects(facts) {
    let out = [];
    for requirement in facts.requirements {
        if requirement.kind != "RequirementUsage" { continue; }
        if !requirement.subjects.is_empty() { continue; }
        out.push(finding(
            "sysml-requirement-usage-missing-subject",
            "error",
            requirement.qualified_name,
            "requirement usage must name the model element it constrains",
        ));
    }
    out
}

fn verification_shape(facts) {
    let out = [];
    for verification in facts.verifications {
        if verification.subjects.is_empty() {
            out.push(finding(
                "sysml-verification-missing-subject",
                "warn",
                verification.qualified_name,
                "verification case has no subject; bind its execution/test owner explicitly",
            ));
        }
        if verification.verifies.is_empty() {
            out.push(finding(
                "sysml-verification-without-requirement",
                "error",
                verification.qualified_name,
                "verification case must verify at least one requirement usage",
            ));
            continue;
        }
        for target in verification.verifies {
            if has_requirement_target(target, facts.requirements) { continue; }
            let message = "verification target `";
            message += target;
            message += "` does not resolve to a requirement in the loaded SysML source set";
            out.push(finding(
                "sysml-verification-target-unresolved",
                "error",
                verification.qualified_name,
                message,
            ));
        }
    }
    out
}

fn unverified_usages(facts) {
    let out = [];
    for requirement in facts.requirements {
        if requirement.kind != "RequirementUsage" { continue; }
        if requirement_is_verified(requirement, facts.verifications) { continue; }
        out.push(finding(
            "sysml-requirement-usage-unverified",
            "warn",
            requirement.qualified_name,
            "requirement usage is not covered by any verification case",
        ));
    }
    out
}

fn lint_sysml(facts) {
    let out = [];
    for item in requirement_documentation(facts) { out.push(item); }
    for item in requirement_subjects(facts) { out.push(item); }
    for item in verification_shape(facts) { out.push(item); }
    for item in unverified_usages(facts) { out.push(item); }
    out
}
