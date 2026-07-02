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

use lunco_hooks::{HookError, HookResult, HookValue, RegisteredHook, ScriptHook};
use rhai::{Dynamic, Engine, Scope, AST};

/// A hook implemented by a rhai function.
///
/// Holds its own `Engine` + compiled `AST` and the initial `Scope` produced by
/// running the script's top-level once (so `const` tables the function reads are
/// available). Each [`invoke`](ScriptHook::invoke) runs with a **fresh clone** of
/// that initial scope — no state carries across calls, which is what makes a hook
/// safe to mark `deterministic` for convergent use (merge).
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
        let engine = Engine::new();
        let ast = engine.compile(source).map_err(|e| e.to_string())?;
        // Run top-level statements once to populate consts into the base scope.
        let mut scope = Scope::new();
        engine
            .run_ast_with_scope(&mut scope, &ast)
            .map_err(|e| e.to_string())?;
        Ok(Self { engine, ast, scope, entry: entry.into() })
    }
}

impl ScriptHook for RhaiHook {
    fn invoke(&self, args: &[HookValue]) -> HookResult {
        let dyn_args: Vec<Dynamic> = args.iter().map(hook_to_dynamic).collect();
        // Fresh scope clone per call → no cross-call state (determinism).
        let mut scope = self.scope.clone();
        let options = rhai::CallFnOptions::new().eval_ast(false).rewind_scope(true);
        let result: Dynamic = self
            .engine
            .call_fn_with_options(options, &mut scope, &self.ast, &self.entry, dyn_args)
            .map_err(|e| HookError(e.to_string()))?;
        Ok(dynamic_to_hook(&result))
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

/// Convert a rhai [`Dynamic`] back into a neutral [`HookValue`]. Unknown/opaque
/// types degrade to their debug string (matching the reflect-walker's fallback).
fn dynamic_to_hook(d: &Dynamic) -> HookValue {
    if d.is_unit() {
        HookValue::Unit
    } else if d.is_int() {
        HookValue::Int(d.as_int().unwrap_or(0))
    } else if d.is_float() {
        HookValue::Float(d.as_float().unwrap_or(0.0))
    } else if d.is_bool() {
        HookValue::Bool(d.as_bool().unwrap_or(false))
    } else if d.is_string() {
        HookValue::Str(d.clone().into_string().unwrap_or_default())
    } else if d.is_array() {
        let arr = d.clone().cast::<rhai::Array>();
        HookValue::Array(arr.iter().map(dynamic_to_hook).collect())
    } else if d.is_map() {
        let map = d.clone().cast::<rhai::Map>();
        HookValue::Map(
            map.iter()
                .map(|(k, v)| (k.to_string(), dynamic_to_hook(v)))
                .collect(),
        )
    } else {
        HookValue::Str(format!("{d:?}"))
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
}
