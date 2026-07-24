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
//!   Control bindings are checked against the same authority the loader uses
//!   (`lunco_core::parse_user_intent`): `ControlBinding` load is deliberately
//!   TOLERANT — an unknown intent warns and is skipped — so a typo silently
//!   costs one control at runtime. This is where that becomes a hard error, on
//!   purpose: tolerant load, strict pre-flight.
//! - `.wgsl` — reflect the `Material` param schema (`ParamSchema::parse`).
//!   Full naga module validation is deliberately absent: naga is not a direct
//!   dependency of this crate and the light path adds none.
//! - `.rhai` — `rhai::Engine::compile` only; nothing is executed.
//! - `.btxml` / `.xml` — the same BehaviorTree.CPP v4 parser and semantic
//!   validation used by the runtime asset loader; nothing is executed.
//!
//! Registered as an [`ApiQueryProvider`] (it returns data, like
//! [`crate::usd_prim_query`]), so one implementation answers rhai `query()`,
//! Python, raw HTTP and MCP:
//! `{"command":"ValidateAsset","params":{"path":"lunco://models/X.mo"}}`.

use bevy::prelude::*;
use lunco_api::queries::{ApiQueryProvider, ApiQueryRegistry};
use lunco_api::schema::{ApiErrorCode, ApiResponse};
use lunco_hooks::HookValue as H;
use lunco_usd_bevy::{CanonicalStage, UsdRead};
use serde_json::json;
use std::path::{Path, PathBuf};

/// The structured verdict on one asset file. Serialized verbatim as the
/// query's `data` payload.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidationReport {
    /// The path as the caller gave it.
    pub path: String,
    /// Asset kind, from the extension: `modelica` | `usd` | `wgsl` | `rhai` |
    /// `behavior_tree`.
    pub kind: String,
    /// True iff `errors` is empty — the file would survive the load path.
    pub ok: bool,
    /// Human-readable, with `line N:` prefixes where the parser gives them.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Kind-specific extras: `model`/`params`/`inputs` (.mo),
    /// `wheel_prims` (.usda), `shader_params` (.wgsl).
    pub info: serde_json::Value,
    /// Domain-shaped facts handed to the authored lint rules for this kind
    /// (`apply_lint_policy`). Not part of the API payload — it is the linter's
    /// input, not the caller's answer — so it is skipped on serialization and
    /// taken (not cloned) when the rules run.
    #[serde(skip)]
    pub(crate) lint_facts: Option<H>,
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
            lint_facts: None,
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
                .error(format!("cannot read {}: {e}", path.display()));
        }
    };
    let report = match ext.as_str() {
        "mo" => validate_modelica(reference, &path, &text),
        "usda" => validate_usda(reference, &path, &text),
        "wgsl" => validate_wgsl(reference, &text),
        "rhai" => validate_rhai(reference, &text),
        "btxml" | "xml" => validate_behavior_tree(reference, &text),
        other => ValidationReport::new(reference, "unknown").error(format!(
            "unsupported extension `.{other}` — supported: .mo, .usda, .wgsl, .rhai, .btxml, .xml"
        )),
    };
    apply_lint_policy(report, &text)
}

/// Consult the DOMAIN's authored lint rules and fold their findings into the
/// report.
///
/// The checks above are what the LOADER itself would refuse — compiled, because
/// they are the loader's own code paths. This is the other half: rules about what
/// is merely WRONG, authored in `assets/scripting/policy/lint_<domain>.rhai` and
/// replaceable on a running sim (`register_hook("lint.usd", …)`). One linter per
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

    for f in lunco_lint::run_lint(&report.kind, H::Map(facts)) {
        match f.severity {
            lunco_lint::LintSeverity::Error => report.errors.push(f.line()),
            _ => report.warnings.push(f.line()),
        }
    }
    report.finish()
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
            Some(joined) => report.errors.extend(joined.lines().map(|l| l.to_string())),
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
    let mut in_string = false;
    let mut in_equations = false;
    for (idx, raw) in text.lines().enumerate() {
        let n = idx + 1;
        // Strip strings and comments in source order. A quote inside a comment
        // is inert, and comment markers inside a description string are text.
        let mut line = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        let mut escaped = false;
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if in_string {
                if ch == '"' && !escaped {
                    in_string = false;
                }
                escaped = ch == '\\' && !escaped;
                continue;
            }
            if ch == '/' {
                match chars.peek() {
                    Some('/') => break,
                    Some('*') => {
                        chars.next();
                        in_block_comment = true;
                        continue;
                    }
                    _ => {}
                }
            }
            if ch == '"' {
                in_string = true;
                escaped = false;
            } else {
                line.push(ch);
            }
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
            errors.push(format!("line {n}: `when` equation — {HINT}"));
        } else if in_equations && has_word(trimmed, "if") {
            errors.push(format!("line {n}: `if` in an equation — {HINT}"));
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

    let engine_assets = engine_assets_root();
    let stage = match lunco_usd_bevy::compose_file_to_stage_with_assets(
        path,
        Some(engine_assets.as_path()),
    ) {
        Ok(s) => s,
        Err(e) => return report.error(format!("compose: {e}")),
    };
    let stage = CanonicalStage::from_stage(stage, path.to_string_lossy().to_string());
    let view = stage.view();

    // The physics projection the `lint.usd` rules read — the SAME facts the live
    // loader hands them (`lunco_usd_avian::physics_facts`), so a rule cannot pass
    // here and fire at load, or the reverse.
    report.lint_facts = Some(lunco_usd_avian::physics_facts(&view));

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
    // Every control binding's intent must be one `parse_user_intent` knows.
    let mut control_bindings = Vec::new();
    for prim in view.prim_paths() {
        if !is_controls_scope(&view, &prim) {
            continue;
        }
        for bind in view.children(&prim) {
            let Some(name) = bind.name() else { continue };
            if lunco_core::parse_user_intent(name).is_some() {
                control_bindings.push(json!({
                    "prim": bind.as_str(),
                    "intent": name,
                    // `text` not `scalar::<String>`: a port may be authored as a
                    // `string` or a `token` and those are distinct sdf values.
                    "port": view.text(&bind, "lunco:port"),
                }));
                continue;
            }
            let hint = match closest_intent(name) {
                Some(s) => format!(" — did you mean `{s}`?"),
                None => String::new(),
            };
            report.errors.push(format!(
                "control binding {} names `{name}`, which is not a control intent — \
                 the loader skips it, so this binding never actuates anything{hint}",
                bind.as_str()
            ));
            control_bindings.push(json!({
                "prim": bind.as_str(),
                "intent": name,
                "ok": false,
            }));
        }
    }

    report.info = json!({
        "wheel_prims": wheel_prims,
        "control_bindings": control_bindings,
    });
    report.finish()
}

/// The shipped `lunco://` root for a parse-only tool.
///
/// A deployed binary runs from the workspace/application directory, which is
/// the same CWD-based root the AssetServer uses. Cargo tests instead run from
/// the crate directory, so use the compile-time workspace layout only when the
/// runtime root is absent.
fn engine_assets_root() -> PathBuf {
    let runtime_root = lunco_assets::assets_dir_abs();
    if runtime_root.is_dir() {
        return runtime_root;
    }
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// A prim whose children carry intent→port bindings. Named `Controls` is the
/// form a vessel composes (`lunco-usd-bevy` matches exactly that name), but the
/// shared profiles in `vessels/control_profiles.usda` are `RoverControls` /
/// `LanderControls` — validating THAT file is the point, since a typo authored
/// once there reaches every vessel referencing it. So the shape decides, not the
/// name: a scope is any prim with a child that authors `lunco:port`.
fn is_controls_scope(view: &impl UsdRead, prim: &openusd::sdf::Path) -> bool {
    view.children(prim)
        .iter()
        .any(|c| c.name().is_some() && view.attr_names(c).iter().any(|a| a == "lunco:port"))
}

/// Spellings [`lunco_core::parse_user_intent`] accepts. Used ONLY to suggest a
/// correction: the check itself calls `parse_user_intent`, never this list, so a
/// stale entry can never accept or reject a binding — only make a hint worse.
/// `intent_spellings_all_parse` pins every entry against the real parser.
const INTENT_SPELLINGS: &[&str] = &[
    "forward",
    "backward",
    "left",
    "right",
    "up",
    "down",
    "yaw_left",
    "yaw_right",
    "roll_left",
    "roll_right",
    "pitch_up",
    "pitch_down",
    "action",
    "brake",
    "release",
    "detach",
    "switch_mode",
    "pause",
    "cancel",
    "unpossess",
];

/// Nearest accepted spelling, when the name is close enough to be a typo rather
/// than a different word. `None` suppresses the hint.
fn closest_intent(name: &str) -> Option<&'static str> {
    let lower = name.trim().to_ascii_lowercase();
    let (best, dist) = INTENT_SPELLINGS
        .iter()
        .map(|c| (*c, edit_distance(&lower, c)))
        .min_by_key(|(_, d)| *d)?;
    (dist <= 2 && dist < lower.len()).then_some(best)
}

/// Levenshtein distance, two-row DP.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
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

// ─── .btxml / .xml ─────────────────────────────────────────────────────────

/// Parse BehaviorTree.CPP through the runtime's authoritative codec.
///
/// The codec validates XML structure, entry-tree selection, composite arity,
/// subtree references and recursion. Calling it here keeps preflight and load
/// semantics identical instead of maintaining a second XML walk.
fn validate_behavior_tree(reference: &str, text: &str) -> ValidationReport {
    let mut report = ValidationReport::new(reference, "behavior_tree");
    match lunco_autopilot::btcpp_xml::xml_to_value(text) {
        Ok(tree) => report.info = json!({ "tree": tree }),
        Err(error) => report
            .errors
            .push(format!("BehaviorTree.CPP parse: {error}")),
    }
    report.finish()
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
    fn branch_lint_ignores_multiline_model_descriptions() {
        let src = "model M\n  \"Active when commanded;\n   otherwise idle\"\n  Real x;\nequation\n  der(x) = 0;\nend M;\n";
        assert!(branch_lint(src).is_empty());
    }

    #[test]
    fn branch_lint_ignores_quotes_and_keywords_in_comments() {
        let src = "model M\n  // \"if this comment never closes\n  Real x;\nequation\n  /* when false then */ der(x) = 0;\nend M;\n";
        assert!(branch_lint(src).is_empty());
    }

    /// Write a `.usda` under the temp dir and hand back its path — the control
    /// checks run on a COMPOSED stage, so they can only be exercised through the
    /// real `validate_asset` entry point.
    fn temp_usda(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("lunco-validate-controls");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write temp usda");
        path
    }

    /// A control scope whose children are named `intent`, one per line.
    fn controls_usda(intents: &[&str]) -> String {
        let mut s = String::from("#usda 1.0\n\ndef \"RoverControls\"\n{\n");
        for i in intents {
            s.push_str(&format!(
                "    def \"{i}\"\n    {{\n        uniform string lunco:port = \"throttle\"\n        uniform double lunco:scale = 1\n    }}\n"
            ));
        }
        s.push_str("}\n");
        s
    }

    #[test]
    fn valid_control_profile_produces_no_diagnostics() {
        let path = temp_usda(
            "valid_profile.usda",
            &controls_usda(&["forward", "backward", "left", "right", "action"]),
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        // NOT vacuous: the scope must actually have been walked. Without this the
        // test would still pass if `is_controls_scope` never matched anything.
        let found = report.info["control_bindings"]
            .as_array()
            .expect("control_bindings in info")
            .len();
        assert_eq!(found, 5, "{:?}", report.info);
    }

    #[test]
    fn external_twin_scene_resolves_lunco_references_for_preflight() {
        let path = temp_usda(
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
    fn misspelled_intent_is_reported_once_with_a_suggestion() {
        let path = temp_usda(
            "typo_profile.usda",
            &controls_usda(&["forwrad", "backward", "left", "right", "action"]),
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(!report.ok, "a misspelled intent must fail validation");
        let hits: Vec<&String> = report
            .errors
            .iter()
            .filter(|e| e.contains("not a control intent"))
            .collect();
        assert_eq!(hits.len(), 1, "{:?}", report.errors);
        assert!(hits[0].contains("forwrad"), "{}", hits[0]);
        assert!(hits[0].contains("did you mean `forward`"), "{}", hits[0]);
    }

    /// The suggestion table is a MIRROR of `parse_user_intent`, never an
    /// authority. If an entry stops parsing, the mirror has drifted.
    #[test]
    fn intent_spellings_all_parse() {
        for s in INTENT_SPELLINGS {
            assert!(
                lunco_core::parse_user_intent(s).is_some(),
                "`{s}` is no longer an accepted intent spelling"
            );
        }
    }

    #[test]
    fn unknown_extension_lists_supported() {
        let report = validate_asset("no/such/file.xyz");
        assert!(!report.ok);
    }

    fn temp_text(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("lunco-validate-text");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        std::fs::write(&path, body).expect("write temp text asset");
        path
    }

    #[test]
    fn validates_canonical_btxml_through_runtime_codec() {
        let path = temp_text(
            "route.btxml",
            r#"<root BTCPP_format="4" main_tree_to_execute="MainTree">
  <BehaviorTree ID="MainTree">
    <Sequence>
      <Action ID="drive_to" target="/Route/W1"/>
      <Action ID="drive_to" target="/Route/W2"/>
    </Sequence>
  </BehaviorTree>
</root>"#,
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(report.ok, "{:?}", report.errors);
        assert_eq!(report.kind, "behavior_tree");
        assert!(
            report.info["tree"].is_object(),
            "the parser must return a real runtime tree: {}",
            report.info
        );
    }

    #[test]
    fn rejects_semantically_invalid_btxml() {
        let path = temp_text(
            "missing-entry.btxml",
            r#"<root BTCPP_format="4" main_tree_to_execute="Missing">
  <BehaviorTree ID="MainTree"><Action ID="hold"/></BehaviorTree>
</root>"#,
        );
        let report = validate_asset(path.to_str().unwrap());
        assert!(!report.ok);
        assert!(
            report
                .errors
                .iter()
                .any(|error| error.contains("names no <BehaviorTree>")),
            "{:?}",
            report.errors
        );
    }
}
