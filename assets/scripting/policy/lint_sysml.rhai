// SysML v2 structural lint policy.
//
// Rust owns parsing/resolution and supplies typed `facts`; Rhai owns the
// project-independent authoring rules. This policy checks source-backed
// relationships and typed-data shape, not project-specific IDs or values.

fn finding(rule, severity, subject, message) {
    let out = #{};
    out.rule = rule;
    out.severity = severity;
    out.subject = subject;
    out.message = message;
    out
}

fn located_subject(subject, file, start) {
    subject + " @ " + file + ":" + start.to_string()
}

fn semantic_subject(record) {
    if record.element == () { return record.qualified_name; }
    let element = record.element;
    if element.file == () || element.start == () { return record.qualified_name; }
    located_subject(record.qualified_name, element.file, element.start)
}

fn same_element_handle(left, right) {
    if left == () || right == () { return false; }
    left.source_revision == right.source_revision
        && left.source_fingerprint == right.source_fingerprint
        && left.element_id == right.element_id
}

fn verification_has_requirement(verification, requirement) {
    let requirement_handle = requirement.element.handle;
    for target_handle in verification.verified_requirements {
        if same_element_handle(target_handle, requirement_handle) { return true; }
    }
    false
}

fn requirement_is_verified(requirement, verifications) {
    for verification in verifications {
        if verification_has_requirement(verification, requirement) { return true; }
    }
    false
}

fn requirement_documentation(facts) {
    let out = [];
    for requirement in facts.requirements {
        if requirement.kind != "RequirementDefinition" { continue; }
        if !requirement.documentation.is_empty() { continue; }
        let subject = semantic_subject(requirement);
        out.push(finding(
            "sysml-requirement-missing-documentation",
            "warn",
            subject,
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
        let subject = semantic_subject(requirement);
        out.push(finding(
            "sysml-requirement-usage-missing-subject",
            "error",
            subject,
            "requirement usage must name the model element it constrains",
        ));
    }
    out
}

fn verification_shape(facts) {
    let out = [];
    for verification in facts.verifications {
        let subject = semantic_subject(verification);
        if verification.subjects.is_empty() {
            out.push(finding(
                "sysml-verification-missing-subject",
                "warn",
                subject,
                "verification case has no subject; bind its execution/test owner explicitly",
            ));
        }
        if verification.verified_requirements.is_empty() {
            out.push(finding(
                "sysml-verification-without-requirement",
                "error",
                subject,
                "verification objective must resolve at least one `verifiedRequirement` relationship",
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
        let subject = semantic_subject(requirement);
        out.push(finding(
            "sysml-requirement-usage-unverified",
            "warn",
            subject,
            "requirement usage is not covered by any verification case",
        ));
    }
    out
}

fn source_record_has_locator(source_owner, attributes) {
    for attribute in attributes {
        if !same_element_handle(attribute.owner_handle, source_owner) { continue; }
        if attribute.name != "locator" || attribute.value == () { continue; }
        if attribute.value.string_value == () { continue; }
        if attribute.value.string_value == "" { continue; }
        return true;
    }
    false
}

fn evidence_owner_has_source(evidence_owner, references, attributes) {
    for source_link in references {
        if !same_element_handle(source_link.from_owner, evidence_owner) { continue; }
        if source_link.from_feature_name != "sources" { continue; }
        if source_link.target == () { continue; }
        if source_record_has_locator(source_link.target, attributes) { return true; }
    }
    false
}

fn requirement_has_source_evidence(requirement, references, attributes) {
    for target_link in references {
        if target_link.from_feature_name != "targetRequirement" { continue; }
        if !same_element_handle(target_link.target, requirement.element.handle) { continue; }
        if target_link.from_owner == () { continue; }
        if evidence_owner_has_source(target_link.from_owner, references, attributes) {
            return true;
        }
    }
    false
}

fn requirement_source_evidence(facts) {
    let out = [];
    // AnalyzeSysml selections may omit either table. Do not report a false
    // missing-source result when the caller supplied a partial fact set.
    if facts.references == () || facts.attributes == () { return out; }
    for requirement in facts.requirements {
        if requirement.kind != "RequirementUsage" { continue; }
        if requirement_has_source_evidence(requirement, facts.references, facts.attributes) {
            continue;
        }
        let subject = semantic_subject(requirement);
        out.push(finding(
            "sysml-requirement-usage-missing-source",
            "warn",
            subject,
            "requirement usage has no typed source-evidence link with a populated locator; add its source or parent requirement to the evidence catalog",
        ));
    }
    out
}

fn requirement_has_required_constraint(requirement) {
    for constraint in requirement.constraints {
        if constraint.kind == "require" { return true; }
    }
    false
}

fn requirement_formal_acceptance(facts) {
    let out = [];
    for requirement in facts.requirements {
        if requirement.kind != "RequirementUsage" || requirement.constraints == () { continue; }
        if requirement_has_required_constraint(requirement) { continue; }
        let subject = semantic_subject(requirement);
        out.push(finding(
            "sysml-requirement-usage-missing-formal-acceptance",
            "warn",
            subject,
            "requirement usage has no inherited or owned require constraint; add a formal acceptance predicate, or define the finite qualitative rubric in its verification case",
        ));
    }
    out
}

fn looks_like_identifier(value) {
    if value == "" || !value.contains_alphabetic() { return false; }
    let forbidden = [" ", "\t", "\n", "/", "\\", ".", ":", "-", ";",
        "=", "(", ")", ",", "\"", "'", "?", "!", "&", "|", "+", "[", "]",
        "@", "#", "$", "%", "^", "*", "{", "}", "<", ">", "~", "`"];
    for character in forbidden {
        if value.contains(character) { return false; }
    }
    for segment in value.split("_") {
        if segment == "" { return false; }
    }
    true
}

fn string_identity_collection(attribute) {
    if attribute.declared_type == () { return false; }
    let declared = attribute.declared_type;
    if declared.value_category != "Primitive" || declared.primitive != "String" {
        return false;
    }
    if attribute.value == () { return false; }
    let values = attribute.value.elements;
    if values == () || values.len() < 2 { return false; }
    for value in values {
        if value.string_value == () || !looks_like_identifier(value.string_value) {
            return false;
        }
    }
    true
}

fn scalar_string_identity(attribute) {
    if attribute.declared_type == () || attribute.value == () { return false; }
    let declared = attribute.declared_type;
    if declared.value_category != "Primitive" || declared.primitive != "String" {
        return false;
    }
    if attribute.value.string_value == () { return false; }
    let value = attribute.value.string_value;
    if value.contains(",") {
        let values = value.split(",");
        if values.len() < 2 { return false; }
        for item in values {
            if !looks_like_identifier(item.trimmed()) { return false; }
        }
        return true;
    }
    let finite_name = attribute.name.ends_with("Name")
        || attribute.name.ends_with("Mode")
        || attribute.name.ends_with("State")
        || attribute.name.ends_with("Kind")
        || attribute.name.ends_with("Profile")
        || attribute.name.ends_with("Configuration")
        || attribute.name.ends_with("Type");
    finite_name && looks_like_identifier(value)
}

fn string_encoded_identities(facts) {
    let out = [];
    if facts.attributes == () { return out; }
    for attribute in facts.attributes {
        if !string_identity_collection(attribute) && !scalar_string_identity(attribute) {
            continue;
        }
        out.push(finding(
            "sysml-string-encoded-identities",
            "warn",
            located_subject(attribute.qualified_name, attribute.file, attribute.start),
            "identifier-like values are stored as String; use a typed enum for a closed choice set, or typed part usages/references when the values identify modeled members",
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
    for item in requirement_source_evidence(facts) { out.push(item); }
    for item in requirement_formal_acceptance(facts) { out.push(item); }
    for item in string_encoded_identities(facts) { out.push(item); }
    out
}
