use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lunco_sysml_ast::{
    SysmlAnalysis, SysmlDiagnosticKind, SysmlElement, SysmlElementHandle,
    SysmlRequirementConstraintKind, SysmlSourceRef,
};
use lunco_sysml_ir::{
    RequirementAuditCode, RequirementAuditPolicy, RequirementAuditSeverity, audit_requirements,
};
use serde::Serialize;

#[derive(Serialize)]
struct JsonReport<'a> {
    source_count: usize,
    source_revision: u64,
    source_fingerprint: u64,
    policy: RequirementAuditPolicy,
    requirement_summary: RequirementSummary,
    diagnostics: &'a [lunco_sysml_ast::SysmlDiagnostic],
    audit: JsonAuditReport<'a>,
}

#[derive(Serialize)]
struct JsonAuditReport<'a> {
    source_revision: u64,
    source_fingerprint: u64,
    has_errors: bool,
    findings: Vec<JsonAuditFinding<'a>>,
}

#[derive(Serialize)]
struct JsonAuditFinding<'a> {
    code: RequirementAuditCode,
    severity: RequirementAuditSeverity,
    element: Option<ElementContext<'a>>,
    source: &'a SysmlSourceRef,
    message: &'a str,
}

#[derive(Serialize)]
struct RequirementSummary {
    definitions: usize,
    definitions_with_formal_require_constraint: usize,
    definitions_without_formal_require_constraint: usize,
    definitions_with_resolved_verification_link: usize,
    definitions_without_resolved_verification_link: usize,
    external_verifier_execution_checked: bool,
}

#[derive(Serialize)]
struct ElementContext<'a> {
    handle: SysmlElementHandle,
    qualified_name: &'a str,
    short_name: Option<&'a str>,
    kind: &'a str,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sysml-audit: {message}");
            eprintln!("Try `sysml-audit --help` for usage.");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut paths = Vec::new();
    let mut policy = RequirementAuditPolicy::default();
    let mut json = false;
    let mut args = env::args_os().skip(1);

    while let Some(argument) = args.next() {
        let argument = argument
            .to_str()
            .ok_or_else(|| "arguments must be valid UTF-8".to_owned())?;
        match argument {
            "--help" | "-h" => {
                print_help();
                return Ok(ExitCode::SUCCESS);
            }
            "--engineering-review" => policy = RequirementAuditPolicy::engineering_review(),
            "--require-short-name" => policy.require_short_name = true,
            "--require-typed-subject" => policy.require_typed_subject = true,
            "--require-verification" => policy.require_verification = true,
            "--require-formal-constraint" => policy.require_formal_constraint = true,
            "--format" => {
                let format = args
                    .next()
                    .ok_or_else(|| "`--format` needs `text` or `json`".to_owned())?;
                match format.to_str() {
                    Some("text") => json = false,
                    Some("json") => json = true,
                    _ => return Err("`--format` accepts only `text` or `json`".to_owned()),
                }
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown option `{value}`"));
            }
            _ => paths.push(PathBuf::from(argument)),
        }
    }

    if paths.is_empty() {
        return Err("provide one or more SysML/KerML source files or directories".to_owned());
    }

    let mut sources = BTreeSet::new();
    for path in &paths {
        collect_sources(path, &mut sources)?;
    }
    if sources.is_empty() {
        return Err("the provided paths contain no `.sysml` or `.kerml` source files".to_owned());
    }

    let files = sources
        .iter()
        .map(|path| {
            let text = fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let name = path.to_string_lossy().into_owned();
            Ok((name, text))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let analysis = SysmlAnalysis::from_files(files);
    let audit = audit_requirements(&analysis, policy);
    let failed = !analysis.diagnostics().is_empty() || audit.has_errors();
    let requirement_summary = requirement_summary(&analysis);

    if json {
        let json_audit = JsonAuditReport {
            source_revision: audit.source_revision,
            source_fingerprint: audit.source_fingerprint,
            has_errors: audit.has_errors(),
            findings: audit
                .findings
                .iter()
                .map(|finding| JsonAuditFinding {
                    code: finding.code,
                    severity: finding.severity,
                    element: analysis
                        .elements()
                        .iter()
                        .find(|element| element.handle == finding.element)
                        .map(element_context),
                    source: &finding.source,
                    message: &finding.message,
                })
                .collect(),
        };
        let report = JsonReport {
            source_count: analysis.files().len(),
            source_revision: audit.source_revision,
            source_fingerprint: audit.source_fingerprint,
            policy,
            requirement_summary,
            diagnostics: analysis.diagnostics(),
            audit: json_audit,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("cannot serialize report: {error}"))?
        );
    } else {
        print_text_report(&analysis, policy, &requirement_summary, &audit);
    }

    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn collect_sources(path: &Path, sources: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_file() {
        if is_sysml_source(path) {
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
            sources.insert(canonical);
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    let entries = fs::read_dir(path)
        .map_err(|error| format!("cannot read directory {}: {error}", path.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read directory entry: {error}"))?;
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", entry.path().display()))?;
        if file_type.is_dir() && ignored_directory(&entry.file_name()) {
            continue;
        }
        collect_sources(&entry.path(), sources)?;
    }
    Ok(())
}

fn is_sysml_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("sysml" | "kerml")
    )
}

fn ignored_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(".git" | ".hg" | ".svn" | "target" | "node_modules" | "build")
    )
}

fn print_text_report(
    analysis: &SysmlAnalysis,
    policy: RequirementAuditPolicy,
    requirement_summary: &RequirementSummary,
    audit: &lunco_sysml_ir::RequirementAuditReport,
) {
    println!("SysML requirement audit");
    println!("Sources: {}", analysis.files().len());
    println!("Revision: {}", audit.source_revision);
    println!("Fingerprint: {:016x}", audit.source_fingerprint);
    println!(
        "Policy: short_name={}, typed_subject={}, verification={}, formal_constraint={}",
        policy.require_short_name,
        policy.require_typed_subject,
        policy.require_verification,
        policy.require_formal_constraint
    );
    println!(
        "Requirement definitions: {} ({} with formal `require`, {} without formal `require`)",
        requirement_summary.definitions,
        requirement_summary.definitions_with_formal_require_constraint,
        requirement_summary.definitions_without_formal_require_constraint
    );
    println!(
        "Resolved verification links: {}/{} definitions; external verifier execution: not checked",
        requirement_summary.definitions_with_resolved_verification_link,
        requirement_summary.definitions
    );

    if analysis.diagnostics().is_empty() {
        println!("Source diagnostics: none");
    } else {
        println!("Source diagnostics:");
        for diagnostic in analysis.diagnostics() {
            let kind = match diagnostic.kind {
                SysmlDiagnosticKind::Syntax => "syntax",
                SysmlDiagnosticKind::Name => "name",
                SysmlDiagnosticKind::Collision => "collision",
            };
            println!(
                "  ERROR {kind} {}:{}..{} {}",
                diagnostic.file, diagnostic.start, diagnostic.end, diagnostic.message
            );
        }
    }

    if audit.findings.is_empty() {
        println!("Requirement findings: none");
    } else {
        println!("Requirement findings:");
        for finding in &audit.findings {
            let severity = match finding.severity {
                RequirementAuditSeverity::Info => "INFO",
                RequirementAuditSeverity::Warning => "WARN",
                RequirementAuditSeverity::Error => "ERROR",
            };
            let element_name = analysis
                .elements()
                .iter()
                .find(|element| element.handle == finding.element)
                .map(|element| element.qualified_name.as_str())
                .unwrap_or("<unresolved element>");
            println!(
                "  {severity} {:?} {element_name} {}:{}..{} {}",
                finding.code,
                finding.source.file,
                finding.source.start,
                finding.source.end,
                finding.message
            );
        }
    }
}

fn requirement_summary(analysis: &SysmlAnalysis) -> RequirementSummary {
    let definitions = analysis
        .requirements()
        .iter()
        .filter(|requirement| requirement.element.kind == "RequirementDefinition")
        .collect::<Vec<_>>();
    let definitions_with_formal_require_constraint = definitions
        .iter()
        .filter(|requirement| {
            requirement
                .constraints
                .iter()
                .any(|constraint| constraint.kind == SysmlRequirementConstraintKind::Require)
        })
        .count();

    let verification_policy = RequirementAuditPolicy {
        require_verification: true,
        ..RequirementAuditPolicy::default()
    };
    let verification_audit = audit_requirements(analysis, verification_policy);
    let definitions_without_resolved_verification_link = verification_audit
        .findings
        .iter()
        .filter(|finding| finding.code == RequirementAuditCode::MissingVerification)
        .count();

    RequirementSummary {
        definitions: definitions.len(),
        definitions_with_formal_require_constraint,
        definitions_without_formal_require_constraint: definitions.len()
            - definitions_with_formal_require_constraint,
        definitions_with_resolved_verification_link: definitions.len()
            - definitions_without_resolved_verification_link,
        definitions_without_resolved_verification_link,
        external_verifier_execution_checked: false,
    }
}

fn element_context(element: &SysmlElement) -> ElementContext<'_> {
    ElementContext {
        handle: element.handle,
        qualified_name: &element.qualified_name,
        short_name: element.short_name.as_deref(),
        kind: &element.kind,
    }
}

fn print_help() {
    println!(
        "Usage: sysml-audit [OPTIONS] <SOURCE-FILE-OR-DIRECTORY>...\n\
         Options:\n\
         --engineering-review       require short names, typed subjects, and verification\n\
         --require-short-name       require a short name on every requirement usage\n\
         --require-typed-subject    require a typed subject on every requirement usage\n\
         --require-verification     require resolved verification coverage\n\
         --require-formal-constraint require a formal `require` constraint\n\
         --format text|json         select human-readable or machine-readable output\n\n\
         Policies are opt-in. Source syntax/name errors and invalid audit findings exit nonzero.\n\
         Example: sysml-audit --engineering-review twins/astrobotic-griffin-1"
    );
}
