//! `ValidateAsset` and `ValidateTwin` — read-only pre-flight checks for asset
//! files and Twin-wide resolver namespaces.
//!
//! ## The light-path contract
//!
//! This is the PARSE-ONLY tier: no solver instance, no scene load, no
//! `AssetServer`, no GPU, no ECS state read or written. Every check here is a
//! pure function over file bytes (plus, for `.usda`, the referenced layers the
//! composer opens), so it is safe to call from anywhere — the HTTP API of a
//! running instance, or `luncosim --validate <path>` before any app exists.
//! Asset authors get "will this load?" in milliseconds instead of finding out
//! by spawning it into a live sim.
//!
//! Per extension:
//! - `.mo` — the same `lunco_modelica_ast::parse_to_syntax` + AST extraction
//!   the USD-cosim dispatcher runs (`lunco-usd-sim-cosim/src/lib.rs`); the
//!   reloadable `lint.modelica` policy decides which AST constructs are
//!   actionable. NO compile, NO `ModelicaCommand` dispatch.
//! - `.usda` — parse the layer (`usda_to_data`), compose the file
//!   (`compose_file_to_stage`), then run the SAME `WheelParams::read` the
//!   spawner runs on every `PhysxVehicleWheelAPI` prim — a wheel that would
//!   refuse to spawn fails validation here, with the exact attribute names.
//!   Control bindings are checked against the same authority the loader uses
//!   (`lunco_control_core::parse_user_intent`): `ControlBinding` load is deliberately
//!   TOLERANT — an unknown intent warns and is skipped — so a typo silently
//!   costs one control at runtime. This is where that becomes a hard error, on
//!   purpose: tolerant load, strict pre-flight.
//! - `.wgsl` — reflect the `Material` param schema (`ParamSchema::parse`).
//!   Full naga module validation is deliberately absent: naga is not a direct
//!   dependency of this crate and the light path adds none.
//! - `.sysml`/`.kerml` — the shared SysML parser/resolver, with no document
//!   mutation or requirement execution.
//! - `.rhai` — `rhai::Engine::compile` only; nothing is executed.
//!
//! Registered as [`ApiQueryProvider`]s (they return data, like
//! `lunco_scene_queries::usd_prim_query`), so one implementation answers rhai `query()`,
//! Python, raw HTTP and MCP:
//! `{"type":"ExecuteCommand","command":"ValidateAsset","params":{"path":"lunco://models/X.mo"}}`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::{ApiQueryError, ApiQueryResult, api_param_str};
use lunco_api_core::ApiErrorCode;
use lunco_api_core::{ApiValue, api_value};
use lunco_hooks::HookValue as H;
use lunco_usd_bevy_stage::{UsdRead, canonical::CanonicalStage};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The structured verdict on one asset file. Serialized verbatim as the
/// query's `data` payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationReport {
    /// The path as the caller gave it.
    pub path: String,
    /// Asset kind, from the extension: `modelica` | `usd` | `sysml` | `wgsl` | `rhai`.
    pub kind: String,
    /// True iff `errors` is empty — the file would survive the load path.
    pub ok: bool,
    /// Human-readable, with `line N:` prefixes where the parser gives them.
    pub errors: Vec<String>,
    /// Non-fatal diagnostics produced by the loader or authored lint policy.
    pub warnings: Vec<String>,
    /// Structured findings produced by the authored lint policy. `errors` and
    /// `warnings` remain the concise display surface; automation should use
    /// these stable rule identifiers and fields instead of parsing prose.
    pub findings: Vec<ValidationFinding>,
    /// Kind-specific extras: `model`/`params`/`inputs` (.mo),
    /// `wheel_prims` (.usda), `shader_params` (.wgsl).
    pub info: serde_json::Value,
    /// Domain-shaped facts handed to the authored lint rules for this kind
    /// (`apply_lint_policy`). Not part of the API payload — it is the linter's
    /// input, not the caller's answer — so it is skipped on serialization and
    /// taken (not cloned) when the rules run.
    #[serde(skip)]
    pub(crate) lint_facts: Option<H>,
    /// The immutable semantic snapshot backing this report. Kept in-process
    /// for typed query projections; never serialized through `info`/JSON.
    #[serde(skip)]
    pub sysml_analysis: Option<std::sync::Arc<lunco_sysml_ast::SysmlAnalysis>>,
}

impl ValidationReport {
    fn new(path: &str, kind: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: kind.to_string(),
            ok: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            findings: Vec::new(),
            info: json!({}),
            lint_facts: None,
            sysml_analysis: None,
        }
    }

    fn error(mut self, msg: impl Into<String>) -> Self {
        self.errors.push(msg.into());
        self.ok = false;
        self
    }

    fn finish(mut self) -> Self {
        self.ok = self.errors.is_empty();
        self
    }
}

/// Machine-readable finding emitted by a source or asset lint policy.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationFinding {
    pub domain: String,
    pub rule: String,
    pub severity: String,
    pub subject: String,
    pub message: String,
}

/// Resolve the caller's reference to a disk file, mirroring the engine's
/// `lunco://` mount (`lunco-assets`): a path that exists as given (absolute or
/// cwd-relative — the CLI case) wins; otherwise `lunco://x` and bare library
/// paths resolve against `<cwd>/assets` exactly as the `AssetServer` would.
/// Native-only by construction — there is no local filesystem on wasm.
fn resolve(reference: &str) -> Result<PathBuf, String> {
    let as_given = Path::new(reference);
    if as_given.is_file() {
        return Ok(as_given.to_path_buf());
    }
    match lunco_assets_core::engine_asset_local_path(reference) {
        Some(p) if p.is_file() => Ok(p),
        Some(p) => Err(format!(
            "file not found: `{reference}` (tried as given, then {})",
            p.display()
        )),
        None => Err(format!(
            "`{reference}` carries a scheme this pre-flight check cannot resolve \
             without a running instance (twin://, http…) — pass a lunco:// or \
             filesystem path"
        )),
    }
}

/// Validate one asset file, dispatching on its extension. Pure: reads the
/// file (and, for `.usda`, its referenced layers) and applies the authored lint
/// policy. This is an explicit validation entry point for the CLI and API; a
/// startup subsystem that only needs loader acceptance should use
/// [`validate_asset_loadability`].
pub fn validate_asset(reference: &str) -> ValidationReport {
    validate_asset_with_policy(reference, true)
}

/// Check whether the runtime loader accepts one asset, without running authored
/// lint policies. Use this from automatic discovery/startup paths that need to
/// hide assets which cannot load; policy lint remains an explicit user action.
pub fn validate_asset_loadability(reference: &str) -> ValidationReport {
    validate_asset_with_policy(reference, false)
}

fn validate_asset_with_policy(reference: &str, apply_authored_policy: bool) -> ValidationReport {
    let path = match resolve(reference) {
        Ok(p) => p,
        Err(e) => return ValidationReport::new(reference, "unknown").error(e),
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let text = match lunco_assets_core::read_asset_file_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return ValidationReport::new(reference, "unknown")
                .error(format!("cannot read {}: {e}", path.display()));
        }
    };
    let report = match ext.as_str() {
        "mo" => validate_modelica(reference, &path, &text),
        "usda" => validate_usda(reference, &path, &text),
        "sysml" | "kerml" => validate_sysml(reference, &path, &text),
        "wgsl" => validate_wgsl(reference, &text),
        "rhai" => validate_rhai(reference, &text),
        other => ValidationReport::new(reference, "unknown").error(format!(
            "unsupported extension `.{other}` — supported: .mo, .usda, .sysml, .kerml, .wgsl, .rhai"
        )),
    };
    if apply_authored_policy {
        apply_lint_policy(report, &text)
    } else {
        report
    }
}

// ─── .sysml / .kerml ───────────────────────────────────────────────────────

/// Parse and resolve one SysML source file through the same pure AST boundary
/// used by the runtime document. Standard-library diagnostics are excluded from
/// the report because this pre-flight call concerns only the supplied file.
fn validate_sysml(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let analysis = std::sync::Arc::new(lunco_sysml_ast::SysmlAnalysis::build(
        [(path.to_string_lossy().to_string(), text.to_owned())],
        true,
        lunco_hash::fnv1a64(text.as_bytes()),
    ));
    finish_sysml_report(reference, analysis)
}

fn finish_sysml_report(
    reference: &str,
    analysis: std::sync::Arc<lunco_sysml_ast::SysmlAnalysis>,
) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "sysml");
    for diagnostic in analysis.diagnostics() {
        report.errors.push(format!(
            "{}:{}..{}: {}",
            diagnostic.file, diagnostic.start, diagnostic.end, diagnostic.message
        ));
    }
    // The validated report intentionally contains no second, JSON-shaped copy
    // of the semantic AST. `sysml_analysis` is the typed in-process snapshot;
    // the generic AnalyzeSysml query projects only the tables a caller asks
    // for, and Rhai policies own interpretation.
    // Keep structural lint facts on the same immutable snapshot. Validation
    // callers can run `lint.sysml`; policy-neutral analysis callers consume
    // `sysml_analysis` without making their result depend on lint findings.
    report.lint_facts = Some(lunco_sysml_ast::lint_facts::sysml_facts(&analysis));
    report.sysml_analysis = Some(analysis);
    report.finish()
}

/// One Twin-level lint finding in the pre-flight response.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TwinValidationFinding {
    /// Stable authored rule id.
    pub rule: String,
    /// Policy severity.
    pub severity: String,
    /// Name or Twin subject involved.
    pub subject: String,
    /// Actionable diagnostic text.
    pub message: String,
}

/// Read-only Twin-wide resolver namespace pre-flight report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TwinValidationReport {
    /// Folder supplied by the caller.
    pub path: String,
    /// Twin display name, or the folder name for an unmanifested folder.
    pub twin: String,
    /// Absolute root inspected by the namespace reader.
    pub root: String,
    /// `warn` by default; `error` makes collisions fail the report.
    pub policy: String,
    /// True iff no policy finding has error severity and the Twin opened.
    pub ok: bool,
    /// Error-severity policy findings and open/read failures.
    pub errors: Vec<String>,
    /// Warning/info policy findings.
    pub warnings: Vec<String>,
    /// Every resolver entry used to form the collision index.
    pub entries: Vec<crate::twin_lint::NamespaceEntry>,
    /// Only names ambiguous in an actual resolver scope.
    pub collisions: Vec<crate::twin_lint::NamespaceCollision>,
    /// Source files the read-only index could not inspect.
    pub read_errors: Vec<String>,
    /// Structured policy findings, for callers that do not parse display lines.
    pub findings: Vec<TwinValidationFinding>,
}

fn twin_validation_error(
    reference: &str,
    policy: &str,
    message: impl Into<String>,
) -> TwinValidationReport {
    let message = message.into();
    TwinValidationReport {
        path: reference.to_string(),
        twin: String::new(),
        root: String::new(),
        policy: policy.to_string(),
        ok: false,
        errors: vec![message],
        warnings: Vec::new(),
        entries: Vec::new(),
        collisions: Vec::new(),
        read_errors: Vec::new(),
        findings: Vec::new(),
    }
}

fn resolve_twin_root(reference: &str) -> Result<PathBuf, String> {
    let as_given = Path::new(reference);
    if as_given.is_dir() {
        return Ok(as_given.to_path_buf());
    }
    match lunco_assets_core::engine_asset_local_path(reference) {
        Some(path) if path.is_dir() => Ok(path),
        Some(path) => Err(format!(
            "Twin folder not found: `{reference}` (resolved `{}` is not a directory)",
            path.display()
        )),
        None => Err(format!(
            "`{reference}` is not a local Twin folder; pass a filesystem path or a lunco:// asset directory"
        )),
    }
}

/// Validate all resolver namespaces visible from one Twin folder.
///
/// This is the pure pre-flight counterpart of `RunLint { scope: "twin" }`:
/// both call the same Twin inspector and authored `lint.twin` policy. The
/// explicit folder argument keeps this API independent of an active ECS
/// workspace and makes it suitable for CI.
pub fn validate_twin(reference: &str, requested_policy: &str) -> TwinValidationReport {
    let policy = match crate::twin_lint::policy_name(requested_policy) {
        Ok(policy) => policy,
        Err(message) => return twin_validation_error(reference, requested_policy, message),
    };
    let root = match resolve_twin_root(reference) {
        Ok(root) => root,
        Err(message) => return twin_validation_error(reference, policy, message),
    };
    let mode = match lunco_twin::TwinMode::open(&root) {
        Ok(mode) => mode,
        Err(error) => return twin_validation_error(reference, policy, error.to_string()),
    };
    let twin = match mode {
        lunco_twin::TwinMode::Folder(twin) | lunco_twin::TwinMode::Twin(twin) => twin,
        lunco_twin::TwinMode::Orphan(path) => {
            return twin_validation_error(
                reference,
                policy,
                format!(
                    "Twin validation requires a folder, got file `{}`",
                    path.display()
                ),
            );
        }
    };
    let snapshot = crate::twin_lint::inspect_twin(&twin);
    let lint_findings = lunco_lint::run_lint("twin", crate::twin_lint::facts(&snapshot, policy));
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut findings = Vec::with_capacity(lint_findings.len());
    for finding in lint_findings {
        let line = finding.line();
        match finding.severity {
            lunco_lint::LintSeverity::Error => errors.push(line),
            _ => warnings.push(line),
        }
        findings.push(TwinValidationFinding {
            rule: finding.rule,
            severity: finding.severity.as_str().to_string(),
            subject: finding.subject,
            message: finding.message,
        });
    }
    if twin.manifest.as_ref().is_some_and(|manifest| {
        manifest.sysml.is_some()
            || manifest.verification.is_some()
            || !manifest.components.is_empty()
    }) {
        if let Err(source_errors) = twin.discover_sysml_sources_checked() {
            errors.extend(
                source_errors
                    .into_iter()
                    .map(|error| format!("Twin SysML source set: {error}")),
            );
        }
        errors.extend(
            twin.verification_registry_errors()
                .into_iter()
                .map(|error| format!("Twin verification registry: {error}")),
        );
        errors.extend(
            twin.component_registry_errors()
                .into_iter()
                .map(|error| format!("Twin component registry: {error}")),
        );
    }
    TwinValidationReport {
        path: reference.to_string(),
        twin: snapshot.twin.clone(),
        root: snapshot.root.clone(),
        policy: policy.to_string(),
        ok: errors.is_empty(),
        errors,
        warnings,
        entries: snapshot.entries.clone(),
        collisions: snapshot.collisions.clone(),
        read_errors: snapshot.read_errors,
        findings,
    }
}

/// Consult the DOMAIN's authored lint rules and fold their findings into the
/// report.
///
/// The checks above are what the LOADER itself would refuse — compiled, because
/// they are the loader's own code paths. This is the other half: rules about what
/// is merely WRONG, authored in `assets/scripting/policy/lint_<domain>.rhai` and
/// replaceable on a running sim (`bind_policy("lint.usd", …)`). One linter per
/// domain: a USD rule, a Modelica rule and a script rule share no vocabulary.
///
/// The facts are the ones the pre-flight already computed — `report.info` per
/// kind, plus the source text for the text-shaped domains — so a rule author gets
/// the same picture the validator has. The USD domain additionally hands over the
/// full physics projection (see [`usd_lint_facts`]).
///
/// Findings never fail a file that the loader would accept: `error` severities
/// join `errors` (and flip `ok`), everything else joins `warnings`.
fn apply_lint_policy(mut report: ValidationReport, text: &str) -> ValidationReport {
    if report.kind == "unknown" {
        return report;
    }
    let mut facts = vec![
        ("path".to_string(), H::str(report.path.clone())),
        ("kind".to_string(), H::str(report.kind.clone())),
        ("ok".to_string(), H::Bool(report.ok)),
        (
            "errors".to_string(),
            H::Array(report.errors.iter().cloned().map(H::str).collect()),
        ),
        // The source itself, for the text-shaped domains: a rule about a script
        // ("a test scenario that never emits a verdict") needs the text, and
        // shipping it costs one clone of a file already in memory.
        ("source".to_string(), H::str(text.to_string())),
    ];
    // The domain's own facts are MERGED IN AT TOP LEVEL, not nested under a key.
    // A rule must see the identical shape whether it was reached from here or
    // from `RunLint` on the live scene — nest them here and `facts.bodies` is
    // suddenly `facts.subject.bodies`, every USD rule silently matches nothing,
    // and the linter reports a clean bill of health for a broken file. That is
    // exactly what happened the first time this was wired, and what
    // `the_deliberately_broken_scene_still_fails_the_same_gate` now pins.
    if let Some(H::Map(domain_facts)) = report.lint_facts.take() {
        facts.extend(domain_facts);
    }

    for finding in lunco_lint::run_lint(&report.kind, H::Map(facts)) {
        let line = finding.line();
        report.findings.push(ValidationFinding {
            domain: finding.domain,
            rule: finding.rule,
            severity: finding.severity.as_str().to_owned(),
            subject: finding.subject,
            message: finding.message,
        });
        match finding.severity {
            lunco_lint::LintSeverity::Error => report.errors.push(line),
            _ => report.warnings.push(line),
        }
    }
    report.finish()
}

// ─── .mo ────────────────────────────────────────────────────────────────────

/// Rumoca PARSE phase only — the same call + extraction the USD-cosim
/// dispatcher makes (`dispatch_loaded_modelica_sources`). The authored
/// `lint.modelica` policy receives the resulting AST facts. No compile.
fn validate_modelica(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "modelica");

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("validate.mo");
    let syntax = lunco_modelica_ast::parse_to_syntax(text, file_name);
    if syntax.has_errors() {
        match syntax.parse_error() {
            Some(joined) => report.errors.extend(joined.lines().map(|l| l.to_string())),
            None => report
                .errors
                .push("parse failed (no diagnostic text from rumoca)".to_string()),
        }
    }

    // Lenient parsing still yields usable name/parameter/input snapshots —
    // same recovery semantics the cosim dispatcher relies on. The lint facts
    // below are projected from this exact AST, not from a second parse.
    let ast = syntax.best_effort();
    let interface = lunco_modelica_ast::ast_extract::parse_model_interface_from_ast(ast);
    let model_name = interface.model_name.clone();
    let parameters: std::collections::BTreeMap<String, f64> = interface
        .parameters
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();
    let inputs: std::collections::BTreeMap<String, f64> = interface
        .inputs
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect();

    // The domain's own facts, for the authored rules. Merged at TOP LEVEL by
    // `apply_lint_policy` — see the warning there about nesting. All
    // declaration and equation facts come from the already-recovered AST.
    report.lint_facts =
        Some(lunco_modelica_ast::lint_facts::modelica_facts_from_interface(ast, &interface));

    report.info = json!({
        "model": model_name,
        "params": parameters,
        "inputs": inputs,
        // Outputs are not knowable at the parse phase — they are the model's
        // variables, which exist only after a compile.
        "outputs": serde_json::Value::Null,
    });
    report.finish()
}

// ─── .usda ──────────────────────────────────────────────────────────────────

/// Parse the layer, compose the file, then run the spawner's own
/// `WheelParams::read` over every `PhysxVehicleWheelAPI` prim.
fn validate_usda(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "usd");

    // The layer's own syntax first: a compose error on a referenced layer
    // should not mask a typo in THIS file.
    if let Err(e) = lunco_usd_authoring::author::usda_to_data(text) {
        return report.error(format!("usda parse: {e}"));
    }

    let engine_assets = engine_assets_root();
    let stage = match lunco_usd_bevy_stage::compose::compose_file_to_stage_with_assets(
        path,
        Some(engine_assets.as_path()),
    ) {
        Ok(s) => s,
        Err(e) => return report.error(format!("compose: {e}")),
    };
    let stage = CanonicalStage::from_stage(stage, path.to_string_lossy().to_string());
    let view = stage.view();

    // The physics projection the `lint.usd` rules read — the SAME complete facts
    // the live lint command hands them, including USD-sim gear drives, so a rule
    // cannot pass here and fire at load, or the reverse.
    let (lint_facts, control_binding_info) =
        crate::lint_command::usd_physics_facts_with_control_info(&view);
    report.lint_facts = Some(lint_facts);

    // Every composed wheel must satisfy the ONE reader both wheel kinds spawn
    // through — `Err(missing)` here is exactly the refusal the spawner logs.
    let attachment_topology = lunco_usd_sim_authoring::collect_wheel_attachment_topology(&view);
    let mut wheel_prims = Vec::new();
    for prim in view.prim_paths() {
        if !view.has_api_schema(&prim, "PhysxVehicleWheelAPI") {
            continue;
        }
        let Some(attachment) = attachment_topology.binding_for(prim.as_str()) else {
            let reason = if attachment_topology.is_invalid(prim.as_str()) {
                "has malformed or ambiguous PhysxVehicleWheelAttachmentAPI topology"
            } else {
                "has no PhysxVehicleWheelAttachmentAPI binding"
            };
            report.errors.push(format!(
                "wheel {} would refuse to spawn — {}",
                prim.as_str(),
                reason
            ));
            wheel_prims.push(json!({
                "prim": prim.as_str(),
                "ok": false,
                "missing": [reason],
            }));
            continue;
        };
        let suspension = openusd::sdf::Path::new(&attachment.suspension).ok();
        let tire = openusd::sdf::Path::new(&attachment.tire).ok();
        match lunco_usd_sim_authoring::WheelParams::read(
            &view,
            &prim,
            suspension.as_ref(),
            tire.as_ref(),
        ) {
            Ok(_) => wheel_prims.push(json!({ "prim": prim.as_str(), "ok": true })),
            Err(missing) => {
                report.errors.push(format!(
                    "wheel {} would refuse to spawn — missing required attributes: {}",
                    prim.as_str(),
                    missing.join(", ")
                ));
                wheel_prims.push(json!({
                    "prim": prim.as_str(),
                    "ok": false,
                    "missing": missing,
                }));
            }
        }
    }
    report.info = json!({
        "wheel_prims": wheel_prims,
        "control_bindings": control_binding_info,
    });
    report.finish()
}

/// The shipped `lunco://` root for a parse-only tool.
///
/// Asset-root discovery is shared with the runtime AssetServer. Keeping this
/// function as a thin owner call means validation never guesses a repository
/// layout from the crate's manifest directory.
pub(crate) fn engine_assets_root() -> PathBuf {
    lunco_assets_core::assets_dir_abs()
}

// ─── .wgsl ──────────────────────────────────────────────────────────────────

/// Reflect the dynamic-param schema. Module-level (naga) validation is
/// skipped on purpose — see module docs.
fn validate_wgsl(reference: &str, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "wgsl");
    match lunco_materials::ParamSchema::parse(text) {
        Some(schema) => {
            let params: Vec<serde_json::Value> = schema
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "name": f.name,
                        "type": format!("{:?}", f.ty),
                        "offset": f.offset,
                        "ui": format!("{:?}", f.ui),
                        "default": f.default.as_ref().map(|d| format!("{d:?}")),
                    })
                })
                .collect();
            if !lunco_materials::is_prop_pickable_source(text) {
                report.warnings.push(
                    "not prop-pickable: declares an `//!@engine` field that is not \
                     prop-fillable per the engine-param registry — the picker will \
                     skip it (it still loads as a scene shader)"
                        .to_string(),
                );
            }
            report.info = json!({
                "shader_params": params,
                "uniform_size": schema.size,
            });
        }
        None => {
            report.warnings.push(
                "no reflectable `Material` struct — the shader exposes no tunable \
                 params and cannot be driven by SetObjectProperty"
                    .to_string(),
            );
            report.info = json!({ "shader_params": [] });
        }
    }
    report.finish()
}

// ─── .rhai ──────────────────────────────────────────────────────────────────

/// Compile-only — a bare engine with no bindings; nothing runs. Scripts using
/// LunCo's registered functions still compile: rhai resolves names at call
/// time, not compile time.
fn validate_rhai(reference: &str, text: &str) -> ValidationReport {
    let report = ValidationReport::new(reference, "rhai");
    let mut engine = rhai::Engine::new();
    // Preflight and live execution must share the same parser/resource policy.
    // The comparison scenario uses the supported function depth; a bare Rhai
    // engine would reject it before the production scripting engine sees it.
    lunco_hooks_rhai::rhai_limits::apply(&mut engine);
    match engine.compile(text) {
        Ok(_) => report.finish(),
        // rhai's Display includes "line N, position M".
        Err(e) => report.error(format!("rhai compile: {e}")),
    }
}

// ─── CLI ────────────────────────────────────────────────────────────────────

/// One-shot CLI leg (`luncosim --validate <path>…`): print each report
/// human-readably, return the process exit code (0 = all ok, 1 = any failed).
/// No Bevy `App` is ever constructed on this path.
pub fn run_cli(paths: &[String]) -> i32 {
    let mut failed = false;
    for p in paths {
        let report = validate_asset(p);
        let verdict = if report.ok { "OK" } else { "FAIL" };
        println!("{verdict}  {} ({})", report.path, report.kind);
        for e in &report.errors {
            println!("  error: {e}");
        }
        for w in &report.warnings {
            println!("  warning: {w}");
        }
        failed |= !report.ok;
    }
    if failed { 1 } else { 0 }
}

// ─── API registration ───────────────────────────────────────────────────────

/// `ValidateAsset { path }` → [`ValidationReport`].
struct ValidateAssetProvider;

impl ApiQueryProvider for ValidateAssetProvider {
    fn name(&self) -> &'static str {
        "ValidateAsset"
    }

    fn execute(&self, _world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(path) = api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ValidateAsset requires params.path (string): a lunco:// or filesystem path",
            ));
        };
        let report = validate_asset(path);
        Ok(Some(lunco_api_core::api_value_from_serializable(&report)?))
    }
}

/// SysML source validation and structural `lint.sysml` policy. Semantic fact
/// selection is provided separately by policy-neutral `AnalyzeSysml`;
/// workflow-specific interpretation belongs to Rhai policies.
struct ValidateSysmlProvider;

impl ApiQueryProvider for ValidateSysmlProvider {
    fn name(&self) -> &'static str {
        "ValidateSysml"
    }

    fn execute(&self, world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(path) = api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ValidateSysml requires params.path (string): a filesystem path or twin:// URI",
            ));
        };
        let report = validate_sysml_reference(world, path, true);
        let analysis = report.sysml_analysis.as_deref();
        let source_files: Vec<_> = analysis
            .into_iter()
            .flat_map(|analysis| analysis.files())
            .map(|file| ApiValue::str(file.name.clone()))
            .collect();
        let source_revision = analysis
            .map(|analysis| ApiValue::UInt(analysis.source_revision()))
            .unwrap_or(ApiValue::Unit);
        let findings = lunco_api_core::api_value_from_serializable(&report.findings)?;
        Ok(Some(api_value!({
            "path": report.path,
            "kind": report.kind,
            "ok": report.ok,
            "errors": report.errors,
            "warnings": report.warnings,
            "findings": findings,
            "source_files": ApiValue::Array(source_files),
            "source_revision": source_revision,
        })))
    }
}

pub fn analyze_sysml_reference(world: &World, reference: &str) -> ValidationReport {
    validate_sysml_reference(world, reference, false)
}

pub(crate) fn validate_sysml_reference(
    world: &World,
    reference: &str,
    apply_structural_policy: bool,
) -> ValidationReport {
    if let Some(name) = reference.strip_prefix("twin://") {
        if !name.is_empty() && !name.contains('/') && !name.contains('\\') {
            return validate_sysml_twin(world, name, reference, apply_structural_policy);
        }
    }
    let Some((name, relative)) = lunco_assets_core::parse_twin_uri(reference) else {
        return validate_asset_with_policy(reference, apply_structural_policy);
    };
    let Some(roots) = world.get_resource::<lunco_assets_core::TwinRoots>() else {
        return ValidationReport::new(reference, "sysml")
            .error("SysML twin:// source query requires the TwinRoots asset registry");
    };
    let path = match roots.resolve_file(name, Path::new(relative)) {
        Ok(Some(path)) => path,
        Ok(None) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("Twin `{name}` is not mounted"));
        }
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot resolve {reference}: {error}"));
        }
    };
    let text = match lunco_assets_core::read_asset_file_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot read {}: {error}", path.display()));
        }
    };
    let kind = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    if !matches!(kind.to_ascii_lowercase().as_str(), "sysml" | "kerml") {
        return ValidationReport::new(reference, "unknown")
            .error("SysML twin:// source path must end in .sysml or .kerml");
    }
    let report = validate_sysml(reference, &path, &text);
    if apply_structural_policy {
        apply_lint_policy(report, &text)
    } else {
        report
    }
}

fn validate_sysml_twin(
    world: &World,
    name: &str,
    reference: &str,
    apply_structural_policy: bool,
) -> ValidationReport {
    let Some(workspace) = world.get_resource::<lunco_workspace::WorkspaceResource>() else {
        return ValidationReport::new(reference, "sysml").error(
            "SysML twin:// source query requires the mounted WorkspaceResource; open the Twin first",
        );
    };
    let Some(roots) = world.get_resource::<lunco_assets_core::TwinRoots>() else {
        return ValidationReport::new(reference, "sysml")
            .error("SysML twin:// source query requires the TwinRoots asset registry");
    };
    let root = match roots.root_of(name) {
        Ok(Some(root)) => root,
        Ok(None) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("Twin `{name}` is not mounted"));
        }
        Err(error) => {
            return ValidationReport::new(reference, "sysml")
                .error(format!("cannot resolve {reference}: {error}"));
        }
    };
    let Some((_, twin)) = workspace.twins().find(|(_, twin)| twin.root == root) else {
        return ValidationReport::new(reference, "sysml").error(format!(
            "Twin `{name}` is mounted in TwinRoots but has no matching Workspace entry; reopen it through the Workspace",
        ));
    };
    let relative_sources = match twin.discover_sysml_sources_checked() {
        Ok(sources) => sources,
        Err(errors) => {
            let mut report = ValidationReport::new(reference, "sysml");
            report.errors.extend(
                errors
                    .into_iter()
                    .map(|error| format!("Twin `{name}` SysML source set: {error}")),
            );
            return report.finish();
        }
    };

    let mut sources = Vec::with_capacity(relative_sources.len());
    let mut policy_source = String::new();
    let mut revision_input = Vec::new();
    for relative in relative_sources {
        let text = match roots.overlay_bytes(name, &relative) {
            Ok(Some(bytes)) => match String::from_utf8((*bytes).clone()) {
                Ok(text) => text,
                Err(error) => {
                    return ValidationReport::new(reference, "sysml").error(format!(
                        "Twin `{name}` SysML overlay `{}` is not UTF-8: {error}",
                        relative.display()
                    ));
                }
            },
            Ok(None) => {
                let path = match roots.resolve_file(name, &relative) {
                    Ok(Some(path)) => path,
                    Ok(None) => {
                        return ValidationReport::new(reference, "sysml").error(format!(
                            "Twin `{name}` SysML source `{}` cannot be resolved by TwinRoots",
                            relative.display()
                        ));
                    }
                    Err(error) => {
                        return ValidationReport::new(reference, "sysml").error(format!(
                            "cannot resolve Twin `{name}` SysML source `{}`: {error}",
                            relative.display()
                        ));
                    }
                };
                match lunco_assets_core::read_asset_file_string(&path) {
                    Ok(text) => text,
                    Err(error) => {
                        return ValidationReport::new(reference, "sysml")
                            .error(format!("cannot read {}: {error}", path.display()));
                    }
                }
            }
            Err(error) => {
                return ValidationReport::new(reference, "sysml").error(format!(
                    "cannot read Twin `{name}` SysML source `{}`: {error}",
                    relative.display()
                ));
            }
        };
        let logical = lunco_assets_core::twin_uri(name, &relative);
        revision_input.extend_from_slice(logical.as_bytes());
        revision_input.push(0);
        revision_input.extend_from_slice(text.as_bytes());
        policy_source.push_str(&text);
        policy_source.push('\n');
        sources.push((logical, text));
    }
    let analysis = lunco_sysml_ast::SysmlAnalysis::build_cached(
        sources,
        true,
        lunco_hash::fnv1a64(&revision_input),
    );
    let report = finish_sysml_report(reference, analysis);
    if apply_structural_policy {
        apply_lint_policy(report, &policy_source)
    } else {
        report
    }
}

/// `ValidateTwin { path, policy? }` → [`TwinValidationReport`].
struct ValidateTwinProvider;

impl ApiQueryProvider for ValidateTwinProvider {
    fn name(&self) -> &'static str {
        "ValidateTwin"
    }

    fn execute(&self, _world: &World, params: &ApiValue) -> ApiQueryResult {
        let Some(path) = api_param_str(params, "path") else {
            return Err(ApiQueryError::new(
                ApiErrorCode::DeserializationError,
                "ValidateTwin requires params.path (string): a Twin folder path",
            ));
        };
        let policy = match params.get("policy") {
            None => "warn",
            Some(ApiValue::Str(policy)) => policy.as_str(),
            Some(_) => {
                return Err(ApiQueryError::new(
                    ApiErrorCode::DeserializationError,
                    "ValidateTwin: `policy` must be a string",
                ));
            }
        };
        let report = validate_twin(path, policy);
        Ok(Some(lunco_api_core::api_value_from_serializable(&report)?))
    }
}

/// Register the providers. Called by [`crate::SceneValidationPlugin`], so any
/// host that installs validation answers `ValidateAsset`, `ValidateSysml`, and
/// `ValidateTwin`.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateAssetProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateSysmlProvider);
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateTwinProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write an authored fixture through storage and retain its temporary
    /// directory for the duration of the validation call.
    fn temp_usda(name: &str, body: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(name);
        lunco_storage::write_file_sync(&path, body.as_bytes()).expect("write temp usda");
        (dir, path)
    }

    #[test]
    fn external_twin_scene_resolves_lunco_references_for_preflight() {
        let (_dir, path) = temp_usda(
            "external_twin.usda",
            "#usda 1.0\n\
def Xform \"Battery\" (\n\
    prepend references = @lunco://components/power/battery.usda@</Battery>\n\
)\n{\n}\n",
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(
            report.ok,
            "an external Twin gets the same lunco:// mount as runtime: {:?}",
            report.errors
        );
    }

    #[test]
    fn unknown_extension_lists_supported() {
        let report = validate_asset("no/such/file.xyz");
        assert!(!report.ok);
    }
}
