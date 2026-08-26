//! rhai binding for the runtime-agnostic [`lunco_hooks`] registry.
//!
//! The whole point of the hook substrate is that a *language* is bound **once**
//! and then services **every** hook point. This crate is that one binding for
//! rhai: [`RhaiHook`] implements [`ScriptHook`] by calling a named function in a
//! compiled rhai script, marshalling [`HookValue`] ↔ [`rhai::Dynamic`] at the
//! boundary. [`register_rhai_hook`] compiles a snippet and drops the resulting
//! hook into the global registry under a `HookId`.
//!
//! Mirrors the `lunco-tools` / `lunco-tools-rhai` split (mechanism vs binding)
//! and stays wasm-clean — rhai only, no Bevy — so it can back hooks that fire deep
//! in dependency-free crates (the journal's merge, `lunco-core`'s authorize gate).

/// Shared bounded-resource policy for every Rhai engine in the workspace.
/// Re-exported as `lunco_scripting::rhai_limits` for the world-bound plane.
pub mod rhai_limits;

use lunco_hooks::{HookError, HookResult, HookValue, RegisteredHook, ScriptHook};
use rhai::{Dynamic, Engine, Scope, AST};

/// A hook implemented by a rhai function.
///
/// Holds its own `Engine` + compiled `AST` with literal top-level constants
/// propagated into callable functions, and the initial `Scope` produced by
/// running the script's top-level once. Each [`invoke`](ScriptHook::invoke) runs
/// with a **fresh clone** of that initial scope — no state carries across calls,
/// which is what makes a hook safe to mark `deterministic` for convergent use
/// (merge).
pub struct RhaiHook {
    engine: Engine,
    ast: AST,
    scope: Scope<'static>,
    entry: String,
}

impl RhaiHook {
    /// Compile `source` and target its function `entry`. `source` may define
    /// helper functions and top-level `const`s; `entry` is the function invoked
    /// per hook call, receiving the marshalled args positionally.
    pub fn compile(source: &str, entry: impl Into<String>) -> Result<Self, String> {
        let mut engine = Engine::new();

        // Close the file-import hole BEFORE compiling anything. `Engine::new()`
        // installs rhai's `FileModuleResolver`, which reads arbitrary files
        // relative to the process CWD — so a hook source (a peer-supplied merge
        // policy, an authored `assets/scripting/policy/*.rhai`) could
        // `import "../../../etc/passwd"`. Hook sources are self-contained
        // snippets: no shipped policy or lint script uses `import`, and this
        // crate has no asset layer to resolve one against, so an EMPTY static
        // resolver is the correct fail-closed choice. If hooks ever need
        // libraries, they must resolve through `lunco-assets`' script registry —
        // never off the filesystem.
        engine.set_module_resolver(rhai::module_resolvers::StaticModuleResolver::new());

        // NOT applying `rhai_limits` here — deliberate, not an oversight.
        //
        // The caps (op budget, call/expression depth, string/array size) are a
        // MULTIPLAYER/untrusted-source concern: they bound what a peer-supplied
        // policy can spend. Single-user is the current target, and the depth cap
        // in particular is a known authoring trap — a legitimately nested
        // expression in an authored policy fails to PARSE, which reads as "my
        // script is broken" rather than "you hit a limit".
        //
        // The resolver above stays: file access is a different question from
        // resource budget, and closing it costs authored content nothing (no
        // shipped hook uses `import`).
        //
        // `rhai_limits` lives in this crate and is re-exported as
        // `lunco_scripting::rhai_limits`, so re-enabling is one line here.
        // Hook sources are local authored inputs in this path. A remote source
        // must enter through the authenticated session boundary before it can
        // be compiled or registered as a hook.
        let ast = compile_with_script_consts(&engine, source).map_err(|e| e.to_string())?;
        // Run top-level statements once to populate runtime state into the
        // base scope; literal constants are propagated at compile time above.
        let mut scope = Scope::new();
        engine
            .run_ast_with_scope(&mut scope, &ast)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            engine,
            ast,
            scope,
            entry: entry.into(),
        })
    }
}

/// Compile a hook so its literal top-level `const`s are visible inside its
/// functions. Rhai functions do not close over script scope, and hook calls
/// intentionally run with `eval_ast(false)`, so executing the top level alone
/// cannot make those names resolvable. Recompile with the constants in the
/// compile scope; this is generic hook machinery, not policy-specific logic.
fn compile_with_script_consts(engine: &Engine, source: &str) -> Result<AST, rhai::ParseError> {
    let first = engine.compile(source)?;
    let mut constants = Scope::new();
    for (name, is_const, value) in first.iter_literal_variables(true, false) {
        if is_const {
            constants.push_constant_dynamic(name.to_string(), value);
        }
    }
    if constants.is_empty() {
        return Ok(first);
    }
    engine.compile_with_scope(&constants, source)
}

impl ScriptHook for RhaiHook {
    fn invoke(&self, args: &[HookValue]) -> HookResult {
        let dyn_args: Vec<Dynamic> = args.iter().map(hook_to_dynamic).collect();
        // Fresh scope clone per call → no cross-call state (determinism).
        let mut scope = self.scope.clone();
        let options = rhai::CallFnOptions::new()
            .eval_ast(false)
            .rewind_scope(true);
        let result: Dynamic = self
            .engine
            .call_fn_with_options(options, &mut scope, &self.ast, &self.entry, dyn_args)
            .map_err(|e| HookError(e.to_string()))?;
        dynamic_to_hook(&result)
    }
}

/// Compile `source` and register its `entry` function as the hook `id`.
///
/// `deterministic` declares whether the hook is safe for convergent/replicated use
/// (see the [`lunco_hooks`] determinism contract) — set it `true` ONLY for a pure
/// merge-ordering policy that every peer runs identically. Returns the compile
/// error (unregistered) on failure.
pub fn register_rhai_hook(
    id: impl Into<String>,
    entry: impl Into<String>,
    source: &str,
    deterministic: bool,
) -> Result<String, String> {
    let hook = RhaiHook::compile(source, entry)?;
    Ok(lunco_hooks::register(RegisteredHook {
        id: id.into(),
        backend: "rhai".into(),
        deterministic,
        hook: std::sync::Arc::new(hook),
    }))
}

// ── HookValue ↔ Dynamic marshalling ──────────────────────────────────────────

/// Convert a neutral [`HookValue`] into a rhai [`Dynamic`].
fn hook_to_dynamic(v: &HookValue) -> Dynamic {
    match v {
        HookValue::Unit => Dynamic::UNIT,
        HookValue::Int(i) => Dynamic::from_int(*i),
        HookValue::Float(f) => Dynamic::from_float(*f),
        HookValue::Bool(b) => Dynamic::from_bool(*b),
        HookValue::Str(s) => s.clone().into(),
        HookValue::Array(a) => {
            let arr: rhai::Array = a.iter().map(hook_to_dynamic).collect();
            Dynamic::from_array(arr)
        }
        HookValue::Map(m) => {
            let mut map = rhai::Map::new();
            for (k, val) in m {
                map.insert(k.as_str().into(), hook_to_dynamic(val));
            }
            Dynamic::from_map(map)
        }
    }
}

/// Convert a rhai [`Dynamic`] back into a neutral [`HookValue`]. The hook ABI is
/// deliberately closed: an opaque Rhai value is a policy error, never a debug
/// string that can accidentally satisfy a downstream schema.
fn dynamic_to_hook(d: &Dynamic) -> HookResult {
    if d.is_unit() {
        Ok(HookValue::Unit)
    } else if d.is_int() {
        d.as_int()
            .map(HookValue::Int)
            .map_err(|error| HookError(format!("failed to read Rhai integer: {error}")))
    } else if d.is_float() {
        d.as_float()
            .map(HookValue::Float)
            .map_err(|error| HookError(format!("failed to read Rhai float: {error}")))
    } else if d.is_bool() {
        d.as_bool()
            .map(HookValue::Bool)
            .map_err(|error| HookError(format!("failed to read Rhai bool: {error}")))
    } else if d.is_string() {
        d.clone()
            .into_string()
            .map(HookValue::Str)
            .map_err(|error| HookError(format!("failed to read Rhai string: {error}")))
    } else if d.is_array() {
        let arr = d.clone().cast::<rhai::Array>();
        arr.iter()
            .map(dynamic_to_hook)
            .collect::<Result<Vec<_>, _>>()
            .map(HookValue::Array)
    } else if d.is_map() {
        let map = d.clone().cast::<rhai::Map>();
        map.iter()
            .map(|(k, v)| dynamic_to_hook(v).map(|value| (k.to_string(), value)))
            .collect::<Result<Vec<_>, _>>()
            .map(HookValue::Map)
    } else {
        Err(HookError(format!(
            "unsupported Rhai hook return type `{}`",
            d.type_name()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_invoke_a_rhai_hook() {
        // A merge-ordering-shaped hook: given two entry maps, order by lamport
        // then author. Returns -1 / 0 / 1 (the ScriptedMergePolicy contract).
        let src = r#"
            fn cmp(a, b) {
                if a.lamport != b.lamport { return a.lamport - b.lamport; }
                if a.author < b.author { return -1; }
                if a.author > b.author { return 1; }
                return 0;
            }
        "#;
        let hook = RhaiHook::compile(src, "cmp").unwrap();
        let a = HookValue::map([
            ("lamport", HookValue::Int(3)),
            ("author", HookValue::str("peer-1")),
        ]);
        let b = HookValue::map([
            ("lamport", HookValue::Int(5)),
            ("author", HookValue::str("peer-2")),
        ]);
        let out = hook.invoke(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(out.as_i64(), Some(-2), "lamport 3 sorts before 5");
        // Symmetric.
        let out = hook.invoke(&[b, a]).unwrap();
        assert_eq!(out.as_i64(), Some(2));
    }

    #[test]
    fn function_reads_a_top_level_constant() {
        let hook = RhaiHook::compile(
            "const SCALE = 3; fn scale(value) { value * SCALE }",
            "scale",
        )
        .expect("literal policy constants must be available inside hook functions");
        let out = hook
            .invoke(&[HookValue::Int(4)])
            .expect("the hook call must resolve its constant");
        assert_eq!(out, HookValue::Int(12));
    }

    #[test]
    fn prefixed_top_level_constants_are_collected() {
        let engine = Engine::new();
        let ast = engine
            .compile(
                "const WRENCH_ALLOCATION_ITERATIONS = 64; \
                 const WRENCH_ICON_BOX_HALF_WIDTH = 92;",
            )
            .unwrap();
        let names: Vec<_> = ast
            .iter_literal_variables(true, false)
            .map(|(name, _, _)| name.to_string())
            .collect();
        assert_eq!(
            names,
            ["WRENCH_ALLOCATION_ITERATIONS", "WRENCH_ICON_BOX_HALF_WIDTH"]
        );
    }

    #[test]
    fn helper_function_reads_a_top_level_constant() {
        let hook = RhaiHook::compile(
            "const RADIUS = 10; fn helper() { RADIUS * 1 } fn entry() { helper() }",
            "entry",
        )
        .expect("helper functions must receive the same constant propagation");
        let out = hook.invoke(&[]).expect("entry must call helper");
        assert_eq!(out, HookValue::Int(10));
    }

    #[test]
    fn register_places_hook_in_registry() {
        register_rhai_hook("test.rhai_id", "pick", "fn pick(a, b) { a + b }", true).unwrap();
        let got = lunco_hooks::invoke("test.rhai_id", &[HookValue::Int(1), HookValue::Int(2)]);
        assert_eq!(got.unwrap().unwrap(), HookValue::Int(3));
        lunco_hooks::unregister("test.rhai_id");
    }

    #[test]
    fn compile_error_is_reported_not_registered() {
        let err = register_rhai_hook("test.bad", "f", "fn f( { oops", false);
        assert!(err.is_err());
        assert!(lunco_hooks::get("test.bad").is_none());
    }

    #[test]
    fn opaque_rhai_values_are_rejected_at_the_hook_boundary() {
        let hook = RhaiHook::compile("fn emit() { #{value: [1, 2]}[\"value\"] }", "emit")
            .expect("policy compiles");
        assert_eq!(
            hook.invoke(&[]).expect("hook invocation succeeds"),
            HookValue::Array(vec![HookValue::Int(1), HookValue::Int(2),])
        );

        let opaque = RhaiHook::compile("fn emit() { 1..3 }", "emit").expect("policy compiles");
        let error = opaque
            .invoke(&[])
            .expect_err("ranges are not part of the hook ABI");
        assert!(error.0.contains("unsupported Rhai hook return type"));
    }
}
