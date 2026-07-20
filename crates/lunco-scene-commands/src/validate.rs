//! `ValidateAsset` — pre-flight "does this file compile?" for asset files.
//!
//! ## The light-path contract
//!
//! This is the PARSE-ONLY tier: no solver instance, no scene load, no
//! `AssetServer`, no GPU, no ECS state read or written. Every check here is a
//! pure function over file bytes (plus, for `.usda`, the referenced layers the
//! composer opens), so it is safe to call from anywhere — the HTTP API of a
//! running instance, or `sandbox --validate <path>` before any app exists.
//! Asset authors get "will this load?" in milliseconds instead of finding out
//! by spawning it into a live sim.
//!
//! Per extension:
//! - `.mo` — the same `rumoca_phase_parse::parse_to_syntax` + AST extraction
//!   the USD-cosim dispatcher runs (`lunco-usd-sim/src/cosim.rs`), plus a lint
//!   for `if`/`when` equation constructs, which rumoca's solver path cannot
//!   handle. NO compile, NO `ModelicaCommand` dispatch.
//! - `.usda` — parse the layer (`usda_to_data`), compose the file
//!   (`compose_file_to_stage`), then run the SAME `WheelParams::read` the
//!   spawner runs on every `PhysxVehicleWheelAPI` prim — a wheel that would
//!   refuse to spawn fails validation here, with the exact attribute names.
//! - `.wgsl` — reflect the `Material` param schema (`ParamSchema::parse`).
//!   Full naga module validation is deliberately absent: naga is not a direct
//!   dependency of this crate and the light path adds none.
//! - `.rhai` — `rhai::Engine::compile` only; nothing is executed.
//!
//! Registered as an [`ApiQueryProvider`] (it returns data, like
//! [`crate::usd_prim_query`]), so one implementation answers rhai `query()`,
//! Python, raw HTTP and MCP:
//! `{"command":"ValidateAsset","params":{"path":"lunco://models/X.mo"}}`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_usd_bevy::{CanonicalStage, UsdRead};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The structured verdict on one asset file. Serialized verbatim as the
/// query's `data` payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationReport {
    /// The path as the caller gave it.
    pub path: String,
    /// Asset kind, from the extension: `modelica` | `usd` | `wgsl` | `rhai`.
    pub kind: String,
    /// True iff `errors` is empty — the file would survive the load path.
    pub ok: bool,
    /// Human-readable, with `line N:` prefixes where the parser gives them.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Kind-specific extras: `model`/`params`/`inputs` (.mo),
    /// `wheel_prims` (.usda), `shader_params` (.wgsl).
    pub info: serde_json::Value,
}

impl ValidationReport {
    fn new(path: &str, kind: &str) -> Self {
        Self {
            path: path.to_string(),
            kind: kind.to_string(),
            ok: true,
            errors: Vec::new(),
            warnings: Vec::new(),
            info: json!({}),
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
    match lunco_assets::engine_asset_local_path(reference) {
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
/// file (and, for `.usda`, its referenced layers) and nothing else.
pub fn validate_asset(reference: &str) -> ValidationReport {
    let path = match resolve(reference) {
        Ok(p) => p,
        Err(e) => return ValidationReport::new(reference, "unknown").error(e),
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            return ValidationReport::new(reference, "unknown")
                .error(format!("cannot read {}: {e}", path.display()))
        }
    };
    match ext.as_str() {
        "mo" => validate_modelica(reference, &path, &text),
        "usda" => validate_usda(reference, &path, &text),
        "wgsl" => validate_wgsl(reference, &text),
        "rhai" => validate_rhai(reference, &text),
        other => ValidationReport::new(reference, "unknown").error(format!(
            "unsupported extension `.{other}` — supported: .mo, .usda, .wgsl, .rhai"
        )),
    }
}

// ─── .mo ────────────────────────────────────────────────────────────────────

/// Rumoca PARSE phase only — the same call + extraction the USD-cosim
/// dispatcher makes (`dispatch_loaded_modelica_sources`), then the
/// branch-free lint. No compile.
fn validate_modelica(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "modelica");

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("validate.mo");
    let syntax = rumoca_phase_parse::parse_to_syntax(text, file_name);
    if syntax.has_errors() {
        match syntax.parse_error() {
            Some(joined) => report
                .errors
                .extend(joined.lines().map(|l| l.to_string())),
            None => report
                .errors
                .push("parse failed (no diagnostic text from rumoca)".to_string()),
        }
    }

    // Lenient parsing still yields usable name/parameter/input snapshots —
    // same recovery semantics the cosim dispatcher relies on.
    let ast = syntax.best_effort();
    let model_name = lunco_modelica::extract_model_name_from_ast(ast);
    let parameters: std::collections::BTreeMap<String, f64> =
        lunco_modelica::extract_parameters_from_ast(ast)
            .into_iter()
            .collect();
    let inputs: std::collections::BTreeMap<String, f64> =
        lunco_modelica::extract_inputs_with_defaults_from_ast(ast)
            .into_iter()
            .collect();

    report.errors.extend(branch_lint(text));

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

/// `word` present in `line` with non-identifier characters on both sides.
fn has_word(line: &str, word: &str) -> bool {
    let bytes = line.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut from = 0;
    while let Some(pos) = line[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let left_ok = start == 0 || !is_ident(bytes[start - 1]);
        let right_ok = end == bytes.len() || !is_ident(bytes[end]);
        if left_ok && right_ok {
            return true;
        }
        from = end;
    }
    false
}

/// Flag `if`/`when` equation constructs — rumoca's solver path is branch-free,
/// so a model using them parses but will not simulate. Text-based (comments
/// stripped, equation/algorithm sections tracked) so it also fires on source
/// the recovering parser mangled.
fn branch_lint(text: &str) -> Vec<String> {
    const HINT: &str =
        "rumoca's solver path is branch-free — rewrite as der(x) = expr using max()/min() clamps";
    let mut errors = Vec::new();
    let mut in_block_comment = false;
    let mut in_equations = false;
    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        // Strip comments (line-granular approximation of /* */).
        let mut line = raw.to_string();
        if in_block_comment {
            match line.find("*/") {
                Some(p) => {
                    line = line[p + 2..].to_string();
                    in_block_comment = false;
                }
                None => continue,
            }
        }
        if let Some(p) = line.find("/*") {
            in_block_comment = !line[p..].contains("*/");
            line.truncate(p);
        }
        if let Some(p) = line.find("//") {
            line.truncate(p);
        }
        let trimmed = line.trim();

        if trimmed == "equation"
            || trimmed == "initial equation"
            || trimmed == "algorithm"
            || trimmed == "initial algorithm"
        {
            in_equations = true;
            continue;
        }
        if trimmed.starts_with("end ") || trimmed == "end" {
            in_equations = false;
            continue;
        }

        // `when` is only legal inside equation/algorithm sections, so flag it
        // anywhere; `if` is also a valid *expression* in bindings, so only
        // flag it inside the sections the solver walks.
        if has_word(trimmed, "when") || has_word(trimmed, "elsewhen") {
            errors.push(format!(
                "line {n}: `when` equation — {HINT}"
            ));
        } else if in_equations && has_word(trimmed, "if") {
            errors.push(format!(
                "line {n}: `if` in an equation — {HINT}"
            ));
        }
    }
    errors
}

// ─── .usda ──────────────────────────────────────────────────────────────────

/// Parse the layer, compose the file, then run the spawner's own
/// `WheelParams::read` over every `PhysxVehicleWheelAPI` prim.
fn validate_usda(reference: &str, path: &Path, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "usd");

    // The layer's own syntax first: a compose error on a referenced layer
    // should not mask a typo in THIS file.
    if let Err(e) = lunco_usd_bevy::author::usda_to_data(text) {
        return report.error(format!("usda parse: {e}"));
    }

    let stage = match lunco_usd_bevy::compose_file_to_stage(path) {
        Ok(s) => s,
        Err(e) => return report.error(format!("compose: {e}")),
    };
    let stage = CanonicalStage::from_stage(stage, path.to_string_lossy().to_string());
    let view = stage.view();

    // Every composed wheel must satisfy the ONE reader both wheel kinds spawn
    // through — `Err(missing)` here is exactly the refusal the spawner logs.
    let mut wheel_prims = Vec::new();
    for prim in view.prim_paths() {
        if !view.has_api_schema(&prim, "PhysxVehicleWheelAPI") {
            continue;
        }
        match lunco_usd_sim::wheel_params::WheelParams::read(&view, &prim, None, None) {
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
    report.info = json!({ "wheel_prims": wheel_prims });
    report.finish()
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
    match rhai::Engine::new().compile(text) {
        Ok(_) => report.finish(),
        // rhai's Display includes "line N, position M".
        Err(e) => report.error(format!("rhai compile: {e}")),
    }
}

// ─── CLI ────────────────────────────────────────────────────────────────────

/// One-shot CLI leg (`sandbox --validate <path>…`): print each report
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
    if failed {
        1
    } else {
        0
    }
}

// ─── API registration ───────────────────────────────────────────────────────

/// `ValidateAsset { path }` → [`ValidationReport`].
struct ValidateAssetProvider;

impl ApiQueryProvider for ValidateAssetProvider {
    fn name(&self) -> &'static str {
        "ValidateAsset"
    }

    fn execute(&self, _world: &mut World, params: &serde_json::Value) -> ApiResponse {
        let Some(path) = params.get("path").and_then(|p| p.as_str()) else {
            return ApiResponse::error(
                ApiErrorCode::DeserializationError,
                "ValidateAsset requires params.path (string): a lunco:// or filesystem path",
            );
        };
        let report = validate_asset(path);
        match serde_json::to_value(&report) {
            Ok(v) => ApiResponse::ok(v),
            Err(e) => ApiResponse::error(ApiErrorCode::InternalError, e.to_string()),
        }
    }
}

/// Register the provider. Called by [`crate::commands::SpawnCommandPlugin`],
/// so any binary with the scene verbs answers `ValidateAsset` too — the
/// headless server included.
pub fn register(app: &mut App) {
    app.init_resource::<ApiQueryRegistry>();
    app.world_mut()
        .resource_mut::<ApiQueryRegistry>()
        .register(ValidateAssetProvider);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_lint_flags_when_and_equation_if() {
        let src = "model M\n  Real x;\nequation\n  when x > 1 then\n    reinit(x, 0);\n  end when;\nend M;\n";
        let errs = branch_lint(src);
        assert!(errs.iter().any(|e| e.starts_with("line 4:")), "{errs:?}");
    }

    #[test]
    fn branch_lint_ignores_comments_and_bindings() {
        let src = "model M\n  // if this then that\n  parameter Real k = 2;\nequation\n  der(x) = max(0.0, k);\nend M;\n";
        assert!(branch_lint(src).is_empty());
    }

    #[test]
    fn unknown_extension_lists_supported() {
        let report = validate_asset("no/such/file.xyz");
        assert!(!report.ok);
    }
}
