use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lunco_sysml_ast::{SysmlAnalysis, SysmlDiagnosticKind};
use lunco_sysml_ir::{RequirementAuditPolicy, RequirementAuditSeverity, audit_requirements};
use serde::Serialize;

#[derive(Serialize)]
struct JsonReport<'a> {
    source_count: usize,
    source_revision: u64,
    source_fingerprint: u64,
    policy: RequirementAuditPolicy,
    diagnostics: &'a [lunco_sysml_ast::SysmlDiagnostic],
    audit: lunco_sysml_ir::RequirementAuditReport,
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

    if json {
        let report = JsonReport {
            source_count: analysis.files().len(),
            source_revision: audit.source_revision,
            source_fingerprint: audit.source_fingerprint,
            policy,
            diagnostics: analysis.diagnostics(),
            audit,
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| format!("cannot serialize report: {error}"))?
        );
    } else {
        print_text_report(&analysis, policy, &audit);
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
            println!(
                "  {severity} {:?} {}:{}..{} {}",
                finding.code,
                finding.source.file,
                finding.source.start,
                finding.source.end,
                finding.message
            );
        }
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
